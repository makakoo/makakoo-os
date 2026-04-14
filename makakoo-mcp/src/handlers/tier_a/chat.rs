//! Tier-A chat handlers: read-only stats + recent message history over
//! the `ChatStore`.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

// ─────────────────────────────────────────────────────────────────────
// chat_status — identical to chat_stats + a latest-session snapshot
// ─────────────────────────────────────────────────────────────────────

pub struct ChatStatusHandler {
    ctx: Arc<ToolContext>,
}

impl ChatStatusHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChatStatusHandler {
    fn name(&self) -> &str {
        "chat_status"
    }
    fn description(&self) -> &str {
        "High-level chat-store health: total conversations, messages, \
         active-today count, and the most recent conversation."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _params: Value) -> Result<Value, RpcError> {
        let chat = self
            .ctx
            .chat_store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: chat_store"))?;
        let stats = chat
            .stats()
            .map_err(|e| RpcError::internal(format!("chat_status: {e}")))?;
        let latest = chat
            .list_conversations(None, 1)
            .map_err(|e| RpcError::internal(format!("chat_status: {e}")))?;
        Ok(json!({
            "stats": stats,
            "latest": latest.first(),
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// chat_history — recent messages in a conversation
// ─────────────────────────────────────────────────────────────────────

pub struct ChatHistoryHandler {
    ctx: Arc<ToolContext>,
}

impl ChatHistoryHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChatHistoryHandler {
    fn name(&self) -> &str {
        "chat_history"
    }
    fn description(&self) -> &str {
        "Return the most recent messages for a conversation, chronological \
         (oldest first). If no conversation_id is supplied, falls back to \
         the most recent conversation in the store."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "conversation_id": { "type": "integer" },
                "limit": { "type": "integer", "default": 20 }
            }
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
        let chat = self
            .ctx
            .chat_store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: chat_store"))?;

        let conv_id: i64 = match params.get("conversation_id").and_then(Value::as_i64) {
            Some(id) => id,
            None => {
                let latest = chat
                    .list_conversations(None, 1)
                    .map_err(|e| RpcError::internal(format!("chat_history: {e}")))?;
                match latest.first() {
                    Some(c) => c.id,
                    None => return Ok(json!([])),
                }
            }
        };

        let messages = chat
            .recent_messages(conv_id, limit)
            .map_err(|e| RpcError::internal(format!("chat_history: {e}")))?;
        Ok(json!({
            "conversation_id": conv_id,
            "messages": messages,
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// chat_stats — aggregate counters only
// ─────────────────────────────────────────────────────────────────────

pub struct ChatStatsHandler {
    ctx: Arc<ToolContext>,
}

impl ChatStatsHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChatStatsHandler {
    fn name(&self) -> &str {
        "chat_stats"
    }
    fn description(&self) -> &str {
        "Aggregate chat-store counters: conversations, messages, active_today."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _params: Value) -> Result<Value, RpcError> {
        let chat = self
            .ctx
            .chat_store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: chat_store"))?;
        let stats = chat
            .stats()
            .map_err(|e| RpcError::internal(format!("chat_stats: {e}")))?;
        Ok(json!(stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::chat::ChatStore;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn empty_ctx() -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(PathBuf::from("/tmp/makakoo-t13-test")))
    }

    fn chat_ctx() -> (tempfile::TempDir, Arc<ToolContext>) {
        let tmp = tempdir().unwrap();
        let db = tmp.path().join("chat.db");
        let store = ChatStore::open(&db).unwrap();
        let ctx =
            ToolContext::empty(tmp.path().to_path_buf()).with_chat(Arc::new(store));
        (tmp, Arc::new(ctx))
    }

    #[tokio::test]
    async fn status_missing_subsystem_is_internal() {
        let h = ChatStatsHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn status_empty_store_returns_zero_counts() {
        let (_tmp, ctx) = chat_ctx();
        let h = ChatStatsHandler::new(ctx);
        let out = h.call(json!({})).await.unwrap();
        assert_eq!(out["conversations"], 0);
        assert_eq!(out["messages"], 0);
    }

    #[tokio::test]
    async fn history_empty_store_returns_empty_array() {
        let (_tmp, ctx) = chat_ctx();
        let h = ChatHistoryHandler::new(ctx);
        let out = h.call(json!({})).await.unwrap();
        assert!(out.is_array());
        assert_eq!(out.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn history_returns_messages_after_append() {
        let (_tmp, ctx) = chat_ctx();
        let chat = ctx.chat_store.as_ref().unwrap().clone();
        let conv = chat
            .get_or_create_conversation("test", "u1", "the user")
            .unwrap();
        chat.append_message(conv.id, "user", "hello", None).unwrap();
        chat.append_message(conv.id, "assistant", "hi", None)
            .unwrap();

        let h = ChatHistoryHandler::new(ctx);
        let out = h
            .call(json!({"conversation_id": conv.id, "limit": 10}))
            .await
            .unwrap();
        assert_eq!(out["conversation_id"], conv.id);
        assert_eq!(out["messages"].as_array().unwrap().len(), 2);
    }
}
