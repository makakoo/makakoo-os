//! Secrets precedence adapter (Q11).
//!
//! Resolution order (highest → lowest):
//!   1. `secret_ref` → makakoo keyring lookup via `keyring` crate
//!   2. `env`        → process environment variable
//!   3. `inline`     → TOML literal (dev-only; logs WARNING)
//!
//! Encoded in TOML as one of:
//!     token.secret_ref = "secret:telegram/secretary-bot-token"
//!     token.env        = "SECRETARY_TELEGRAM_MAIN_TOKEN"
//!     token            = "literal-bot-token-value"

use serde::{Deserialize, Serialize};

use crate::{MakakooError, Result};

/// A reference to a secret value, resolved at adapter-startup time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SecretRef {
    Structured(StructuredSecret),
    Inline(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredSecret {
    /// `"secret:<namespace>/<key>"` lookup in the makakoo keyring.
    pub secret_ref: Option<String>,
    /// Environment-variable name to read.
    pub env: Option<String>,
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
    Keyring,
    Env,
    Inline,
}

/// Resolver trait — keyring-backed in production, in-memory in tests.
pub trait SecretsAdapter: Send + Sync {
    /// Look up a `secret:<namespace>/<key>` value. Returns
    /// `Ok(None)` if the key is missing (callers may fall back to
    /// the next precedence layer).
    fn lookup_keyring(&self, secret_ref: &str) -> Result<Option<String>>;

    fn resolve(&self, reference: &SecretRef) -> Result<ResolvedSecret> {
        match reference {
            SecretRef::Structured(s) => {
                if let Some(key) = &s.secret_ref {
                    if let Some(value) = self.lookup_keyring(key)? {
                        return Ok(ResolvedSecret {
                            value,
                            source: SecretSource::Keyring,
                        });
                    }
                    return Err(MakakooError::Config(format!(
                        "secret_ref '{}' not found in keyring",
                        key
                    )));
                }
                if let Some(env_name) = &s.env {
                    return std::env::var(env_name)
                        .map(|value| ResolvedSecret {
                            value,
                            source: SecretSource::Env,
                        })
                        .map_err(|_| {
                            MakakooError::Config(format!(
                                "environment variable '{}' is not set",
                                env_name
                            ))
                        });
                }
                Err(MakakooError::Config(
                    "structured SecretRef must have either `secret_ref` or `env`".into(),
                ))
            }
            SecretRef::Inline(value) => {
                tracing::warn!(
                    target: "makakoo_core::transport::secrets",
                    "inline secret value used (dev-only fallback) — move to makakoo secret store before production"
                );
                Ok(ResolvedSecret {
                    value: value.clone(),
                    source: SecretSource::Inline,
                })
            }
        }
    }
}

/// Production resolver — backed by the OS keyring through the
/// `keyring` crate. Service name is `"makakoo"` to match the
/// existing `SecretsStore` (see makakoo/src/secrets.rs).
///
/// `secret_ref` is the keyring entry name; the `secret:` prefix is
/// stripped before lookup so the actual entry can be addressed
/// either with or without the namespace prefix in the keyring CLI.
pub struct KeyringSecrets;

impl SecretsAdapter for KeyringSecrets {
    fn lookup_keyring(&self, secret_ref: &str) -> Result<Option<String>> {
        let key = secret_ref.strip_prefix("secret:").unwrap_or(secret_ref);
        // The `keyring` crate is sync — wrap in a blocking call.
        // In v1 this runs once at adapter startup, not per message.
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
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory test resolver.
    pub struct MemSecrets {
        pub map: Mutex<HashMap<String, String>>,
    }

    impl MemSecrets {
        fn with(items: &[(&str, &str)]) -> Self {
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
    fn inline_resolves() {
        let secrets = MemSecrets::with(&[]);
        let r = secrets
            .resolve(&SecretRef::Inline("hello".into()))
            .unwrap();
        assert_eq!(r.value, "hello");
        assert_eq!(r.source, SecretSource::Inline);
    }

    #[test]
    fn keyring_takes_precedence_when_present() {
        let secrets = MemSecrets::with(&[("secret:tg/main", "kr-value")]);
        let r = secrets
            .resolve(&SecretRef::Structured(StructuredSecret {
                secret_ref: Some("secret:tg/main".into()),
                env: None,
            }))
            .unwrap();
        assert_eq!(r.value, "kr-value");
        assert_eq!(r.source, SecretSource::Keyring);
    }

    #[test]
    fn missing_keyring_secret_errors() {
        let secrets = MemSecrets::with(&[]);
        let err = secrets
            .resolve(&SecretRef::Structured(StructuredSecret {
                secret_ref: Some("secret:missing/x".into()),
                env: None,
            }))
            .unwrap_err();
        assert!(format!("{err}").contains("not found in keyring"));
    }

    #[test]
    fn env_var_resolves() {
        let key = "MAKAKOO_TEST_SECRETS_ENV_VAR_X";
        std::env::set_var(key, "envval");
        let secrets = MemSecrets::with(&[]);
        let r = secrets
            .resolve(&SecretRef::Structured(StructuredSecret {
                secret_ref: None,
                env: Some(key.into()),
            }))
            .unwrap();
        assert_eq!(r.value, "envval");
        assert_eq!(r.source, SecretSource::Env);
        std::env::remove_var(key);
    }

    #[test]
    fn missing_env_errors() {
        let secrets = MemSecrets::with(&[]);
        let err = secrets
            .resolve(&SecretRef::Structured(StructuredSecret {
                secret_ref: None,
                env: Some("MAKAKOO_TEST_NEVER_SET_VAR".into()),
            }))
            .unwrap_err();
        assert!(format!("{err}").contains("not set"));
    }
}
