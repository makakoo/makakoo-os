//! `LlmHandler` — chat + embed over the capability socket.
//!
//! Spec: `spec/CAPABILITIES.md §1.2`. Plugins call `llm/chat:<model>`
//! and `llm/embed`; the kernel enforces model-scope matching at the
//! grant layer, then routes the call through the shared `LlmClient`
//! / `EmbeddingClient` so every plugin goes through the same token
//! audit trail and the same SwitchAILocal gateway.
//!
//! **Methods served:**
//! - `llm.chat`  params `{ model, messages: [{role, content}] }` → `{ content }`
//! - `llm.embed` params `{ text }` → `{ embedding: [f32], dim }`
//!
//! Omni methods (`llm.describe_image` / `audio` / `video`) are a next
//! slice — they need the `source` payload handling (URL vs b64 blob)
//! which isn't load-bearing for Gate 5.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::capability::socket::{
    CapabilityError, CapabilityHandler, CapabilityRequest,
};
use crate::capability::verb::scope_matches;
use crate::embeddings::EmbeddingClient;
use crate::llm::{ChatMessage, LlmClient};

pub struct LlmHandler {
    llm: Arc<LlmClient>,
    emb: Arc<EmbeddingClient>,
}

impl LlmHandler {
    pub fn new(llm: Arc<LlmClient>, emb: Arc<EmbeddingClient>) -> Self {
        Self { llm, emb }
    }
}

#[derive(Debug, Deserialize)]
struct IncomingMessage {
    role: String,
    content: String,
}

impl IncomingMessage {
    fn to_chat(&self) -> Result<ChatMessage, CapabilityError> {
        match self.role.as_str() {
            "user" => Ok(ChatMessage::user(&self.content)),
            "assistant" => Ok(ChatMessage::assistant(&self.content)),
            "system" => Ok(ChatMessage::system(&self.content)),
            other => Err(CapabilityError::bad_request(format!(
                "unknown chat role {other:?} (expected user/assistant/system)"
            ))),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatParams {
    model: String,
    messages: Vec<IncomingMessage>,
}

#[derive(Debug, Deserialize)]
struct EmbedParams {
    text: String,
}

fn bad_params(e: serde_json::Error) -> CapabilityError {
    CapabilityError::bad_request(format!("bad params: {e}"))
}

#[async_trait]
impl CapabilityHandler for LlmHandler {
    async fn handle(
        &self,
        request: &CapabilityRequest,
        matched_scope: Option<&str>,
    ) -> Result<serde_json::Value, CapabilityError> {
        match request.method.as_str() {
            "llm.chat" => {
                let p: ChatParams = serde_json::from_value(request.params.clone())
                    .map_err(bad_params)?;

                // Extra check: even with a matched scope at the grant
                // layer, double-verify the requested model actually
                // satisfies the granted scope. Belt-and-suspenders for
                // when a handler is invoked directly from tests or
                // from a future daemon path that bypasses the socket.
                if let Some(scope) = matched_scope {
                    if !scope.is_empty() && !scope_matches(scope, &p.model) {
                        return Err(CapabilityError::denied(
                            "llm/chat",
                            &p.model,
                            "granted scope does not match requested model",
                        ));
                    }
                }

                let msgs: Result<Vec<ChatMessage>, _> =
                    p.messages.iter().map(IncomingMessage::to_chat).collect();
                let msgs = msgs?;

                let content = self
                    .llm
                    .chat(&p.model, msgs)
                    .await
                    .map_err(|e| CapabilityError::handler(format!("chat: {e}")))?;
                Ok(json!({ "content": content, "model": p.model }))
            }
            "llm.embed" => {
                let p: EmbedParams = serde_json::from_value(request.params.clone())
                    .map_err(bad_params)?;
                let vec = self
                    .emb
                    .embed(&p.text)
                    .await
                    .map_err(|e| CapabilityError::handler(format!("embed: {e}")))?;
                let dim = vec.len();
                Ok(json!({ "embedding": vec, "dim": dim }))
            }
            other => Err(CapabilityError::handler(format!(
                "unknown llm method {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req(method: &str, params: serde_json::Value, scope: &str) -> CapabilityRequest {
        CapabilityRequest {
            id: json!(1),
            method: method.into(),
            params,
            verb: if method.starts_with("llm.chat") {
                "llm/chat".into()
            } else {
                "llm/embed".into()
            },
            scope: scope.into(),
            correlation_id: None,
        }
    }

    #[tokio::test]
    async fn chat_routes_to_llm_client() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "hi back" } }]
            })))
            .mount(&mock)
            .await;

        let llm = Arc::new(LlmClient::with_base_url(mock.uri()));
        let emb = Arc::new(EmbeddingClient::with_base_url(mock.uri()));
        let h = LlmHandler::new(llm, emb);

        let r = h
            .handle(
                &req(
                    "llm.chat",
                    json!({
                        "model": "ail-compound",
                        "messages": [{ "role": "user", "content": "hi" }],
                    }),
                    "",
                ),
                None,
            )
            .await
            .unwrap();
        assert_eq!(r["content"], "hi back");
        assert_eq!(r["model"], "ail-compound");
    }

    #[tokio::test]
    async fn embed_returns_vector_and_dim() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "embedding": [0.1f32, 0.2, 0.3] }]
            })))
            .mount(&mock)
            .await;

        let llm = Arc::new(LlmClient::with_base_url(mock.uri()));
        let emb = Arc::new(EmbeddingClient::with_base_url(mock.uri()));
        let h = LlmHandler::new(llm, emb);

        let r = h
            .handle(
                &req("llm.embed", json!({ "text": "hello world" }), ""),
                None,
            )
            .await
            .unwrap();
        let vec: Vec<f32> = serde_json::from_value(r["embedding"].clone()).unwrap();
        assert_eq!(vec.len(), 3);
        assert_eq!(r["dim"], 3);
    }

    #[tokio::test]
    async fn scope_mismatch_is_rejected_at_handler() {
        let mock = MockServer::start().await;
        let llm = Arc::new(LlmClient::with_base_url(mock.uri()));
        let emb = Arc::new(EmbeddingClient::with_base_url(mock.uri()));
        let h = LlmHandler::new(llm, emb);

        // Grant is "minimax/*", but plugin asked for "anthropic/claude"
        let err = h
            .handle(
                &req(
                    "llm.chat",
                    json!({
                        "model": "anthropic/claude",
                        "messages": [{ "role": "user", "content": "hi" }],
                    }),
                    "",
                ),
                Some("minimax/*"),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, -32001);
        assert!(err.message.contains("llm/chat"));
    }

    #[tokio::test]
    async fn bad_role_rejected() {
        let mock = MockServer::start().await;
        let llm = Arc::new(LlmClient::with_base_url(mock.uri()));
        let emb = Arc::new(EmbeddingClient::with_base_url(mock.uri()));
        let h = LlmHandler::new(llm, emb);

        let err = h
            .handle(
                &req(
                    "llm.chat",
                    json!({
                        "model": "ail-compound",
                        "messages": [{ "role": "system-monitor", "content": "hi" }],
                    }),
                    "",
                ),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown chat role"));
    }

    #[tokio::test]
    async fn unknown_method_errors() {
        let mock = MockServer::start().await;
        let llm = Arc::new(LlmClient::with_base_url(mock.uri()));
        let emb = Arc::new(EmbeddingClient::with_base_url(mock.uri()));
        let h = LlmHandler::new(llm, emb);

        let err = h
            .handle(&req("llm.teleport", json!({}), ""), None)
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown llm method"));
    }
}
