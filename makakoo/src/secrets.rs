//! Secrets — keyring-backed secret storage with env-var fallback.
//!
//! Wraps the `keyring` crate so the rest of makakoo can store and retrieve
//! API tokens (AIL_API_KEY, ANTHROPIC_API_KEY, etc.) without ever touching
//! plaintext config files. The "service" namespace is `makakoo` on every
//! platform — macOS Keychain, Secret Service on Linux, Credential Manager
//! on Windows.
//!
//! Fallback: [`SecretsStore::resolve`] checks the keyring first and then
//! drops to the supplied environment variable. This keeps existing scripts
//! that export `AIL_API_KEY=...` working while new code prefers the
//! keyring path.

// Public API surface — `resolve` and the canonical `keys::*` constants are
// exported for agents / daemons that haven't landed yet. Allow dead_code
// so the strict workspace clippy gate stays green until those callers wire
// in over the next wave.
#![allow(dead_code)]

use anyhow::{bail, Result};
use keyring::Entry;

/// Service name used for every keyring entry written by makakoo.
pub const SERVICE: &str = "makakoo";

/// Escape hatch for automation that explicitly accepts an OS keychain prompt.
pub const ALLOW_KEYCHAIN_PROMPT_ENV: &str = "MAKAKOO_SECRET_ALLOW_KEYCHAIN_PROMPT";

/// Thin facade around the `keyring` crate — all methods are static and
/// the struct exists purely as a namespace. `SecretsStore::set` /
/// `::get` / `::delete` round-trip exactly one entry; `::resolve` adds
/// an env-var fallback for legacy callers.
pub struct SecretsStore;

impl SecretsStore {
    /// Store a secret under `key` in the OS keyring. Overwrites any prior value.
    pub fn set(key: &str, value: &str) -> Result<()> {
        let entry = Entry::new(SERVICE, key)?;
        entry.set_password(value)?;
        Ok(())
    }

    /// Retrieve a secret from the OS keyring. Returns an error if the entry
    /// does not exist — use [`SecretsStore::resolve`] for best-effort lookup
    /// with env fallback.
    pub fn get(key: &str) -> Result<String> {
        let entry = Entry::new(SERVICE, key)?;
        Ok(entry.get_password()?)
    }

    /// Return a non-empty environment value for the same name as `key`.
    ///
    /// `makakoo secret get AIL_API_KEY` is commonly used inside shell
    /// automation. If that automation already exported `AIL_API_KEY`, do not
    /// touch the OS keyring at all. This avoids macOS Keychain prompt storms
    /// from background agents.
    pub fn env_value(key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }

    /// CLI-safe getter for `makakoo secret get`.
    ///
    /// Reads the matching environment variable first. If no env value exists,
    /// only touches the OS keyring when the caller is allowed to show an OS GUI
    /// prompt. Background agent shells must fail fast instead of spawning
    /// unbounded macOS Keychain dialogs.
    pub fn get_cli(key: &str, allow_keychain_prompt: bool) -> Result<String> {
        if let Some(v) = Self::env_value(key) {
            return Ok(v);
        }

        if !allow_keychain_prompt {
            bail!(
                "refusing to read keyring entry '{key}' from a non-interactive process because it may open an OS keychain prompt; export {key}=... or set {ALLOW_KEYCHAIN_PROMPT_ENV}=1 to allow the prompt explicitly"
            );
        }

        Self::get(key)
    }

    /// Whether automation explicitly opted into OS keychain prompts.
    pub fn keychain_prompt_explicitly_allowed() -> bool {
        std::env::var(ALLOW_KEYCHAIN_PROMPT_ENV)
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    /// Delete a secret from the OS keyring. Returns Ok(()) if the entry was
    /// removed or did not exist in the first place.
    pub fn delete(key: &str) -> Result<()> {
        let entry = Entry::new(SERVICE, key)?;
        match entry.delete_password() {
            Ok(()) => Ok(()),
            // Treat "no entry" as success to keep delete idempotent.
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Store a JSON-serialisable value under `key`.
    pub fn set_json<T: serde::Serialize>(key: &str, value: &T) -> Result<()> {
        let body = serde_json::to_string(value)?;
        Self::set(key, &body)
    }

    /// Retrieve a JSON-deserialisable value under `key`.
    pub fn get_json<T: serde::de::DeserializeOwned>(key: &str) -> Result<T> {
        let raw = Self::get(key)?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// Resolve a secret with an env-var fallback. Returns:
    ///   1. keyring value for `key` if present, else
    ///   2. value of `env_name` if set, else
    ///   3. None.
    pub fn resolve(key: &str, env_name: &str) -> Option<String> {
        if let Ok(v) = Self::get(key) {
            return Some(v);
        }
        std::env::var(env_name).ok()
    }
}

/// Canonical secret keys used across makakoo. Keeping them in one place
/// makes it easy to audit which secrets the runtime expects.
pub mod keys {
    pub const AIL_API_KEY: &str = "AIL_API_KEY";
    pub const ANTHROPIC_API_KEY: &str = "ANTHROPIC_API_KEY";
    pub const OPENAI_API_KEY: &str = "OPENAI_API_KEY";
    pub const TELEGRAM_BOT_TOKEN: &str = "TELEGRAM_BOT_TOKEN";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end round-trip against the real OS keyring. Only runs on
    /// macOS during local development because CI Linux boxes typically
    /// have no unlocked Secret Service daemon.
    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "touches the real macOS Keychain; run explicitly with --ignored"]
    fn roundtrip_macos_keychain() {
        let key = "MAKAKOO_TEST_ROUNDTRIP_KEY";
        SecretsStore::set(key, "hunter2").unwrap();
        assert_eq!(SecretsStore::get(key).unwrap(), "hunter2");
        SecretsStore::delete(key).unwrap();
        assert!(SecretsStore::get(key).is_err());
    }

    #[test]
    fn resolve_prefers_env_when_keyring_missing() {
        let env_name = "MAKAKOO_SECRET_TEST_ENV_VAR";
        // Ensure keyring is empty for this key.
        let _ = SecretsStore::delete("NONEXISTENT_KEY_FOR_TEST");
        std::env::set_var(env_name, "from-env");
        let v = SecretsStore::resolve("NONEXISTENT_KEY_FOR_TEST", env_name);
        std::env::remove_var(env_name);
        assert_eq!(v, Some("from-env".to_string()));
    }

    #[test]
    fn resolve_returns_none_when_both_missing() {
        let env_name = "MAKAKOO_DEFINITELY_NOT_SET_9f8a7b";
        std::env::remove_var(env_name);
        let v = SecretsStore::resolve("ALSO_NOT_IN_KEYRING_9f8a7b", env_name);
        assert_eq!(v, None);
    }

    #[test]
    fn get_cli_prefers_env_without_keyring_prompt() {
        let key = "MAKAKOO_SECRET_GET_CLI_ENV_TEST";
        std::env::set_var(key, "from-env");
        let v = SecretsStore::get_cli(key, false).unwrap();
        std::env::remove_var(key);
        assert_eq!(v, "from-env");
    }

    #[test]
    fn get_cli_blocks_noninteractive_keyring_prompt() {
        let key = "MAKAKOO_SECRET_GET_CLI_BLOCKED_TEST";
        std::env::remove_var(key);
        let err = SecretsStore::get_cli(key, false).unwrap_err().to_string();
        assert!(err.contains("refusing to read keyring entry"));
        assert!(err.contains("non-interactive process"));
    }

    #[test]
    fn keychain_prompt_allow_env_accepts_true_values() {
        std::env::set_var(ALLOW_KEYCHAIN_PROMPT_ENV, "yes");
        assert!(SecretsStore::keychain_prompt_explicitly_allowed());
        std::env::set_var(ALLOW_KEYCHAIN_PROMPT_ENV, "0");
        assert!(!SecretsStore::keychain_prompt_explicitly_allowed());
        std::env::remove_var(ALLOW_KEYCHAIN_PROMPT_ENV);
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "macOS CI runners have no default keychain — keyring returns \
                  PlatformFailure instead of NoEntry on first delete. Run \
                  locally with --ignored to validate against a real keychain."
    )]
    fn delete_is_idempotent() {
        // Deleting a missing key must not error.
        SecretsStore::delete("MAKAKOO_DELETE_IDEMPOTENT_TEST").unwrap();
        SecretsStore::delete("MAKAKOO_DELETE_IDEMPOTENT_TEST").unwrap();
    }
}
