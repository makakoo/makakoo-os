//! Tier-B `outbound_draft` handler.
//!
//! HARD RULE: this handler is the only MCP-surfaced path that touches
//! the outbound queue, and it ONLY creates drafts in `pending` state.
//! There is no MCP tool for `approve` or `mark_sent` — those stay on
//! the CLI (and require the user's keypress) so a runaway model can
//! never autoreply-bomb somebody's inbox. See
//! `makakoo_core::outbound::OutboundQueue` for the three-state
//! pending → approved → sent machine and the SQL-level guard that
//! refuses to `mark_sent` anything not already `approved`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

pub struct OutboundDraftHandler {
    ctx: Arc<ToolContext>,
}

impl OutboundDraftHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for OutboundDraftHandler {
    fn name(&self) -> &str {
        "outbound_draft"
    }
    fn description(&self) -> &str {
        "Draft an outbound message (email / linkedin / telegram). ALWAYS \
         creates the draft in 'pending' state — this handler NEVER \
         auto-sends. Approval requires a separate explicit CLI action by \
         the user."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "enum": ["email", "linkedin", "telegram"]
                },
                "recipient": { "type": "string" },
                "subject": { "type": "string" },
                "body": { "type": "string" }
            },
            "required": ["channel", "recipient", "body"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let channel = params
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'channel'"))?;
        if !matches!(channel, "email" | "linkedin" | "telegram") {
            return Err(RpcError::invalid_params(format!(
                "channel '{channel}' not one of email|linkedin|telegram"
            )));
        }
        let recipient = params
            .get("recipient")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'recipient'"))?;
        let subject = params.get("subject").and_then(|v| v.as_str());
        let body = params
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'body'"))?;

        let outbound = self
            .ctx
            .outbound
            .as_ref()
            .ok_or_else(|| RpcError::internal("outbound queue not wired"))?;

        let draft_id = outbound
            .draft(channel, recipient, subject, body)
            .map_err(|e| RpcError::internal(format!("outbound_draft: {e}")))?;

        // Always pending — the queue enforces this, but we restate it
        // in the response payload so callers cannot mistake the draft
        // for a sent message.
        Ok(json!({
            "draft_id": draft_id,
            "status": "pending"
        }))
    }
}

// ═════════════════════════════════════════════════════════════════════
// Tests — including the auto-send rule
// ═════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use makakoo_core::db::{open_db, run_migrations};
    use makakoo_core::outbound::OutboundQueue;

    fn ctx_with_outbound() -> (tempfile::TempDir, Arc<ToolContext>) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = open_db(&db_path).unwrap();
        run_migrations(&conn).unwrap();
        let shared = Arc::new(Mutex::new(conn));
        let q = Arc::new(OutboundQueue::open(shared).unwrap());

        let ctx = ToolContext::empty(dir.path().to_path_buf()).with_outbound(q);
        (dir, Arc::new(ctx))
    }

    #[tokio::test]
    async fn draft_returns_pending_status_never_sent() {
        let (_d, ctx) = ctx_with_outbound();
        let h = OutboundDraftHandler::new(ctx);
        let out = h
            .call(json!({
                "channel": "email",
                "recipient": "alice@example.com",
                "subject": "hi",
                "body": "hello there"
            }))
            .await
            .unwrap();
        // HARD RULE: status is pending, never sent.
        assert_eq!(
            out["status"], json!("pending"),
            "outbound_draft must NEVER auto-send"
        );
        assert_ne!(out["status"], json!("sent"));
        assert!(out["draft_id"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn missing_channel_is_invalid_params() {
        let (_d, ctx) = ctx_with_outbound();
        let h = OutboundDraftHandler::new(ctx);
        let err = h
            .call(json!({ "recipient": "x", "body": "y" }))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn unknown_channel_is_rejected() {
        let (_d, ctx) = ctx_with_outbound();
        let h = OutboundDraftHandler::new(ctx);
        let err = h
            .call(json!({
                "channel": "smoke-signal",
                "recipient": "x",
                "body": "y"
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn missing_outbound_subsystem_returns_internal_error() {
        let ctx = Arc::new(ToolContext::empty(std::env::temp_dir()));
        let h = OutboundDraftHandler::new(ctx);
        let err = h
            .call(json!({
                "channel": "email",
                "recipient": "x",
                "body": "y"
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
        assert!(err.message.contains("not wired"));
    }
}
