//! `CompositeHandler` — route method calls by method-name prefix.
//!
//! A plugin socket exposes many operations (state.read, state.write,
//! secrets.read, brain.write, llm.chat, …). Each domain is implemented
//! by its own `CapabilityHandler`. `CompositeHandler` owns a map of
//! method-prefix → boxed handler and forwards each request to the
//! matching child based on the method's first dotted segment.
//!
//! `state.read` → the handler registered under `"state"`.
//! `brain.write` → the handler registered under `"brain"`.
//!
//! Unknown prefixes → `CapabilityError::handler("unknown method …")`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::capability::socket::{
    CapabilityError, CapabilityHandler, CapabilityRequest,
};

/// Demux requests to a per-prefix `CapabilityHandler`. The prefix is
/// the substring of `method` before the first `.`.
pub struct CompositeHandler {
    children: HashMap<String, Arc<dyn CapabilityHandler>>,
}

impl CompositeHandler {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
        }
    }

    /// Register a handler under a method prefix. `prefix` should not
    /// contain a dot — it's the full name before the first `.`.
    /// Overwrites any existing entry.
    pub fn register(
        mut self,
        prefix: impl Into<String>,
        handler: Arc<dyn CapabilityHandler>,
    ) -> Self {
        self.children.insert(prefix.into(), handler);
        self
    }

    pub fn handlers(&self) -> impl Iterator<Item = &str> {
        self.children.keys().map(|s| s.as_str())
    }
}

impl Default for CompositeHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CapabilityHandler for CompositeHandler {
    async fn handle(
        &self,
        request: &CapabilityRequest,
        matched_scope: Option<&str>,
    ) -> Result<serde_json::Value, CapabilityError> {
        let prefix = request
            .method
            .split_once('.')
            .map(|(p, _)| p)
            .unwrap_or(request.method.as_str());
        let child = self.children.get(prefix).ok_or_else(|| {
            CapabilityError::handler(format!(
                "unknown method {:?} (registered prefixes: {})",
                request.method,
                self.children.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;
        child.handle(request, matched_scope).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::socket::{CapabilityError, CapabilityHandler};
    use serde_json::json;

    struct NamedHandler(&'static str);

    #[async_trait]
    impl CapabilityHandler for NamedHandler {
        async fn handle(
            &self,
            request: &CapabilityRequest,
            _scope: Option<&str>,
        ) -> Result<serde_json::Value, CapabilityError> {
            Ok(json!({ "handled_by": self.0, "method": request.method }))
        }
    }

    fn req(method: &str) -> CapabilityRequest {
        CapabilityRequest {
            id: json!(1),
            method: method.to_string(),
            params: json!({}),
            verb: "brain/read".into(),
            scope: String::new(),
            correlation_id: None,
        }
    }

    #[tokio::test]
    async fn dispatches_by_prefix() {
        let c = CompositeHandler::new()
            .register("state", Arc::new(NamedHandler("state")))
            .register("secrets", Arc::new(NamedHandler("secrets")));

        let r = c.handle(&req("state.read"), None).await.unwrap();
        assert_eq!(r["handled_by"], "state");

        let r = c.handle(&req("secrets.read"), None).await.unwrap();
        assert_eq!(r["handled_by"], "secrets");
    }

    #[tokio::test]
    async fn unknown_prefix_errors() {
        let c = CompositeHandler::new()
            .register("state", Arc::new(NamedHandler("state")));
        let err = c.handle(&req("brain.read"), None).await.unwrap_err();
        assert!(err.message.contains("unknown method"));
    }

    #[tokio::test]
    async fn method_without_dot_uses_whole_string() {
        let c = CompositeHandler::new()
            .register("ping", Arc::new(NamedHandler("ping")));
        let r = c.handle(&req("ping"), None).await.unwrap();
        assert_eq!(r["handled_by"], "ping");
    }

    #[tokio::test]
    async fn register_overwrites() {
        let c = CompositeHandler::new()
            .register("state", Arc::new(NamedHandler("first")))
            .register("state", Arc::new(NamedHandler("second")));
        let r = c.handle(&req("state.read"), None).await.unwrap();
        assert_eq!(r["handled_by"], "second");
    }
}
