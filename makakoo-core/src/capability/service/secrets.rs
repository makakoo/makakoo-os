//! `SecretHandler` — serve secret reads over the capability socket.
//!
//! Spec §1.7 and §5: secret values flow out of the kernel on demand,
//! never via env-var injection. Plugins declare `secrets/read:KEY_NAME`
//! in their manifest; the socket layer enforces that they only ask for
//! keys they've declared. Plugins also get a context-manager-shaped
//! API on the client side so secret values auto-zero on drop.
//!
//! **Backend pluggability.** The real production backend is the OS
//! keyring (Keychain / Secret Service / Credential Manager) but wiring
//! keyring into `makakoo-core` would add a platform-specific dep that
//! complicates cross-compile on Redox. Instead, `SecretHandler` takes
//! any `SecretBackend` trait object. The binary crate (`makakoo`) wires
//! the real keyring-backed backend at daemon startup; tests use
//! `InMemorySecretBackend`.
//!
//! **Scope enforcement.** The socket-level grant check already rejects
//! requests for keys outside the plugin's `secrets/read:…` scope
//! before calling the handler — so the handler sees only allowed
//! requests. It returns an error on lookup failure and never panics.
//!
//! **Method served:**
//! - `secrets.read` params `{ name }` → `{ value }`

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use serde::Deserialize;
use thiserror::Error;

use crate::capability::socket::{
    CapabilityError, CapabilityHandler, CapabilityRequest,
};

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("secret {name:?} not found")]
    NotFound { name: String },
    #[error("backend error: {msg}")]
    Backend { msg: String },
}

/// Pluggable lookup for secret values. Implementations must be
/// thread-safe and should avoid blocking for long on lookups — the
/// capability socket awaits this call so a slow backend stalls the
/// plugin's response.
pub trait SecretBackend: Send + Sync {
    fn get(&self, name: &str) -> Result<String, SecretError>;
}

/// In-memory backend. Primarily for tests and the daemon's initial
/// bootstrap phase before the real keyring is wired. Safe to share
/// across threads via `Arc`.
pub struct InMemorySecretBackend {
    inner: RwLock<HashMap<String, String>>,
}

impl InMemorySecretBackend {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    pub fn with(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.insert(name, value);
        self
    }

    pub fn insert(&self, name: impl Into<String>, value: impl Into<String>) {
        self.inner
            .write()
            .expect("secret backend rwlock poisoned")
            .insert(name.into(), value.into());
    }
}

impl Default for InMemorySecretBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretBackend for InMemorySecretBackend {
    fn get(&self, name: &str) -> Result<String, SecretError> {
        self.inner
            .read()
            .expect("secret backend rwlock poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| SecretError::NotFound {
                name: name.to_string(),
            })
    }
}

/// Backend that reads secrets from environment variables. Used in
/// production — plugins get whatever the kernel process has in its env.
/// Scope enforcement happens at the grant layer, not here.
pub struct EnvSecretBackend;

impl SecretBackend for EnvSecretBackend {
    fn get(&self, name: &str) -> Result<String, SecretError> {
        std::env::var(name).map_err(|_| SecretError::NotFound {
            name: name.to_string(),
        })
    }
}

pub struct SecretHandler {
    backend: std::sync::Arc<dyn SecretBackend>,
}

impl SecretHandler {
    pub fn new(backend: std::sync::Arc<dyn SecretBackend>) -> Self {
        Self { backend }
    }
}

#[derive(Debug, Deserialize)]
struct ReadParams {
    name: String,
}

#[async_trait]
impl CapabilityHandler for SecretHandler {
    async fn handle(
        &self,
        request: &CapabilityRequest,
        _scope: Option<&str>,
    ) -> Result<serde_json::Value, CapabilityError> {
        match request.method.as_str() {
            "secrets.read" => {
                let p: ReadParams = serde_json::from_value(request.params.clone())
                    .map_err(|e| {
                        CapabilityError::bad_request(format!("bad params: {e}"))
                    })?;
                // Belt-and-suspenders: the caller's scope must match the
                // key name they're requesting. The GrantTable already
                // enforced this at dispatch time, but double-check so a
                // direct `SecretHandler::handle` call (e.g. from a test)
                // still honours the contract.
                if request.scope != p.name {
                    return Err(CapabilityError::bad_request(format!(
                        "scope {:?} does not match requested name {:?}",
                        request.scope, p.name
                    )));
                }
                let value = self.backend.get(&p.name).map_err(|e| match e {
                    SecretError::NotFound { .. } => {
                        CapabilityError::handler(e.to_string())
                    }
                    SecretError::Backend { msg } => {
                        CapabilityError::handler(msg)
                    }
                })?;
                Ok(serde_json::json!({ "value": value }))
            }
            other => Err(CapabilityError::handler(format!(
                "unknown secrets method {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn req(method: &str, params: serde_json::Value, scope: &str) -> CapabilityRequest {
        CapabilityRequest {
            id: json!(1),
            method: method.to_string(),
            params,
            verb: "secrets/read".into(),
            scope: scope.to_string(),
            correlation_id: None,
        }
    }

    #[tokio::test]
    async fn reads_known_secret() {
        let backend = Arc::new(
            InMemorySecretBackend::new().with("AIL_API_KEY", "sk-abc123"),
        );
        let h = SecretHandler::new(backend);
        let r = h
            .handle(
                &req(
                    "secrets.read",
                    json!({ "name": "AIL_API_KEY" }),
                    "AIL_API_KEY",
                ),
                None,
            )
            .await
            .unwrap();
        assert_eq!(r["value"], "sk-abc123");
    }

    #[tokio::test]
    async fn missing_secret_errors() {
        let backend = Arc::new(InMemorySecretBackend::new());
        let h = SecretHandler::new(backend);
        let err = h
            .handle(
                &req(
                    "secrets.read",
                    json!({ "name": "MISSING" }),
                    "MISSING",
                ),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"));
    }

    #[tokio::test]
    async fn scope_must_match_requested_name() {
        let backend = Arc::new(
            InMemorySecretBackend::new().with("AIL_API_KEY", "sk-abc123"),
        );
        let h = SecretHandler::new(backend);
        // Scope grants AIL_API_KEY but the plugin asked for NOTION_TOKEN.
        // The handler rejects even if the backend would have the key —
        // the GrantTable caller is responsible for scope matching but
        // we double-check as a safety net.
        let err = h
            .handle(
                &req(
                    "secrets.read",
                    json!({ "name": "NOTION_TOKEN" }),
                    "AIL_API_KEY",
                ),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("scope"));
    }

    #[tokio::test]
    async fn unknown_method_errors() {
        let backend = Arc::new(InMemorySecretBackend::new());
        let h = SecretHandler::new(backend);
        let err = h
            .handle(&req("secrets.write", json!({}), ""), None)
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown secrets method"));
    }
}
