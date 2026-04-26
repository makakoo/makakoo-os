//! Secrets precedence adapter (Q11, locked).
//!
//! Resolution order (highest → lowest):
//!   1. **Environment variable** (`secret_env`, `app_token_env`,
//!      `bot_token_env`).
//!   2. **`makakoo secret` keyring store** (`secret_ref`,
//!      `app_token_ref`, `bot_token_ref`).
//!   3. **TOML inline** (`inline_secret_dev`). Logs WARNING. Refuses
//!      to load in non-dev mode (enforced at config load).
//!
//! In TOML this looks like:
//!     [[transport]]
//!     id = "telegram-main"
//!     kind = "telegram"
//!     secret_ref       = "agent/secretary/telegram-main/bot_token"
//!     secret_env       = "SECRETARY_TELEGRAM_MAIN_TOKEN"
//!     inline_secret_dev = ""

use serde::{Deserialize, Serialize};

use crate::{MakakooError, Result};

/// One secret slot — collects the env var name, keyring entry name,
/// and inline literal that may resolve to its value.  At least one
/// of the three MUST be populated for the adapter to start.
///
/// All three fields are optional in TOML so the entry can be entered
/// flat at the `[[transport]]` level (e.g. `secret_ref = "…"` on its
/// own line).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretRef {
    /// Environment variable name.
    pub env: Option<String>,
    /// `makakoo secret` keyring entry name (no `secret:` prefix).
    pub keyring_ref: Option<String>,
    /// TOML inline literal — dev-only fallback.
    pub inline: Option<String>,
}

impl SecretRef {
    /// Build from the flat TOML triple at the `[[transport]]` level.
    /// Empty strings count as "absent" — TOML linters often emit
    /// `inline_secret_dev = ""` even when the field isn't set.
    pub fn from_flat(env: Option<String>, keyring_ref: Option<String>, inline: Option<String>) -> Self {
        fn norm(s: Option<String>) -> Option<String> {
            s.and_then(|v| if v.is_empty() { None } else { Some(v) })
        }
        Self {
            env: norm(env),
            keyring_ref: norm(keyring_ref),
            inline: norm(inline),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.env.is_none() && self.keyring_ref.is_none() && self.inline.is_none()
    }
}

/// A resolved secret value plus the source it came from (for
/// auditing / log lines).
#[derive(Debug, Clone)]
pub struct ResolvedSecret {
    pub value: String,
    pub source: SecretSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretSource {
    Env,
    Keyring,
    Inline,
}

/// Resolver trait — keyring-backed in production, in-memory in tests.
pub trait SecretsAdapter: Send + Sync {
    /// Look up a `secret:<namespace>/<key>` value. Returns
    /// `Ok(None)` if the key is missing (callers may fall back to
    /// the next precedence layer).
    fn lookup_keyring(&self, secret_ref: &str) -> Result<Option<String>>;

    /// Resolve a `SecretRef` honoring env > keyring > inline.
    fn resolve(&self, reference: &SecretRef) -> Result<ResolvedSecret> {
        if let Some(env_name) = &reference.env {
            if let Ok(value) = std::env::var(env_name) {
                if !value.is_empty() {
                    return Ok(ResolvedSecret {
                        value,
                        source: SecretSource::Env,
                    });
                }
            }
        }
        if let Some(key) = &reference.keyring_ref {
            if let Some(value) = self.lookup_keyring(key)? {
                return Ok(ResolvedSecret {
                    value,
                    source: SecretSource::Keyring,
                });
            }
        }
        if let Some(value) = &reference.inline {
            tracing::warn!(
                target: "makakoo_core::transport::secrets",
                "inline secret value used (dev-only fallback) — move to env var or makakoo secret store before production"
            );
            return Ok(ResolvedSecret {
                value: value.clone(),
                source: SecretSource::Inline,
            });
        }
        Err(MakakooError::Config(
            "no secret source resolved (env unset, keyring miss, no inline) — adapter cannot start".into(),
        ))
    }
}

/// Production resolver — backed by the OS keyring through the
/// `keyring` crate. Service name is `"makakoo"` to match the
/// existing `SecretsStore` (see makakoo/src/secrets.rs).
pub struct KeyringSecrets;

impl SecretsAdapter for KeyringSecrets {
    fn lookup_keyring(&self, secret_ref: &str) -> Result<Option<String>> {
        let key = secret_ref.strip_prefix("secret:").unwrap_or(secret_ref);
        let entry = keyring::Entry::new("makakoo", key)
            .map_err(|e| MakakooError::Config(format!("keyring entry '{}': {}", key, e)))?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(MakakooError::Config(format!(
                "keyring lookup '{}' failed: {}",
                key, e
            ))),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory test resolver.
    pub struct MemSecrets {
        pub map: Mutex<HashMap<String, String>>,
    }

    impl MemSecrets {
        pub fn with(items: &[(&str, &str)]) -> Self {
            let mut map = HashMap::new();
            for (k, v) in items {
                map.insert((*k).to_string(), (*v).to_string());
            }
            Self {
                map: Mutex::new(map),
            }
        }
    }

    impl SecretsAdapter for MemSecrets {
        fn lookup_keyring(&self, secret_ref: &str) -> Result<Option<String>> {
            Ok(self.map.lock().unwrap().get(secret_ref).cloned())
        }
    }

    #[test]
    fn from_flat_normalises_empty() {
        let r = SecretRef::from_flat(
            Some("".into()),
            Some("agent/secretary/tg/bot".into()),
            Some("".into()),
        );
        assert!(r.env.is_none());
        assert!(r.keyring_ref.is_some());
        assert!(r.inline.is_none());
    }

    #[test]
    fn empty_secret_ref_fails_to_resolve() {
        let secrets = MemSecrets::with(&[]);
        let err = secrets.resolve(&SecretRef::default()).unwrap_err();
        assert!(format!("{err}").contains("no secret source resolved"));
    }

    #[test]
    fn env_takes_precedence_over_keyring() {
        let key = "MAKAKOO_TEST_SECRETS_PRECEDENCE_VAR";
        std::env::set_var(key, "env-value");
        let secrets = MemSecrets::with(&[("agent/x", "kr-value")]);
        let r = secrets
            .resolve(&SecretRef {
                env: Some(key.into()),
                keyring_ref: Some("agent/x".into()),
                inline: Some("inline-value".into()),
            })
            .unwrap();
        assert_eq!(r.value, "env-value");
        assert_eq!(r.source, SecretSource::Env);
        std::env::remove_var(key);
    }

    #[test]
    fn keyring_takes_precedence_over_inline() {
        let secrets = MemSecrets::with(&[("agent/y", "kr-value")]);
        let r = secrets
            .resolve(&SecretRef {
                env: None,
                keyring_ref: Some("agent/y".into()),
                inline: Some("inline-value".into()),
            })
            .unwrap();
        assert_eq!(r.value, "kr-value");
        assert_eq!(r.source, SecretSource::Keyring);
    }

    #[test]
    fn inline_used_when_others_empty() {
        let secrets = MemSecrets::with(&[]);
        let r = secrets
            .resolve(&SecretRef {
                env: None,
                keyring_ref: None,
                inline: Some("inline-value".into()),
            })
            .unwrap();
        assert_eq!(r.value, "inline-value");
        assert_eq!(r.source, SecretSource::Inline);
    }

    #[test]
    fn empty_env_var_falls_through_to_keyring() {
        let key = "MAKAKOO_TEST_EMPTY_ENV_FALLTHROUGH";
        std::env::set_var(key, "");
        let secrets = MemSecrets::with(&[("agent/z", "kr-value")]);
        let r = secrets
            .resolve(&SecretRef {
                env: Some(key.into()),
                keyring_ref: Some("agent/z".into()),
                inline: None,
            })
            .unwrap();
        assert_eq!(r.value, "kr-value");
        assert_eq!(r.source, SecretSource::Keyring);
        std::env::remove_var(key);
    }
}
