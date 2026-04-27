//! Tier-C communication handlers — `harvey_telegram_send` and `chat_send`.
//!
//! # Safety model
//!
//! `harvey_telegram_send` is the single most dangerous tool in the MCP
//! surface because an accidental unsolicited outbound is the worst
//! outcome the project can produce. Every Tier-C call therefore obeys
//! the following in hard code, not docs:
//!
//! 1. **Allow-list.** The `chat_id` must match the ToolContext's
//!    allowlist (read from config, default empty — any call without a
//!    matching allowlist fails closed).
//! 2. **Existing conversation only.** The target `(telegram, chat_id)`
//!    conversation must already exist in `ChatStore`. No new
//!    conversations may be created from an MCP-initiated call.
//! 3. **Auditable.** Every call — success, reject, or wire-failure —
//!    is appended to today's Brain journal via `SuperbrainStore::write_document`
//!    when the store is wired.
//! 4. **No error return on safety failure.** A rejected send returns
//!    `{ok: false, reason: "..."}` with a JSON result so the MCP client
//!    can observe the rejection without tripping an `isError` path.
//!
//! `chat_send` is the "reply to an existing conversation the human
//! already started" entry point. It appends an assistant message to
//! the conversation but does NOT wire directly into the Telegram
//! outbound transport — that responsibility stays with the inbound
//! receive loop in `makakoo_core::chat::telegram::run_forever`.

use async_trait::async_trait;
use chrono::Local;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

// ─────────────────────────────────────────────────────────────────────
// harvey_telegram_send
// ─────────────────────────────────────────────────────────────────────

pub struct HarveyTelegramSendHandler {
    ctx: Arc<ToolContext>,
}

impl HarveyTelegramSendHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }

    /// Load the telegram allowlist from env. `HARVEY_TELEGRAM_ALLOWLIST`
    /// is a comma-separated list of `i64` chat ids. Empty string or
    /// missing => `None` (fail-closed default).
    fn load_allowlist() -> Vec<i64> {
        std::env::var("HARVEY_TELEGRAM_ALLOWLIST")
            .ok()
            .map(|s| {
                s.split(',')
                    .filter_map(|n| n.trim().parse::<i64>().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    /// Log every call to today's journal via SuperbrainStore when wired.
    fn audit(&self, chat_id: i64, text: &str, outcome: &str) {
        if let Some(store) = self.ctx.store.as_ref() {
            let today = Local::now().format("%Y_%m_%d").to_string();
            let doc_id = format!("data/Brain/journals/{today}.md");
            let bullet = format!(
                "- [telegram-audit] chat_id={chat_id} outcome={outcome} len={} first-chars={:?}",
                text.len(),
                text.chars().take(40).collect::<String>()
            );
            // Append rather than overwrite: read the existing doc
            // first and concat.
            let mut content = store
                .get_document(&doc_id)
                .ok()
                .flatten()
                .map(|d| d.content)
                .unwrap_or_default();
            if !content.is_empty() && !content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str(&bullet);
            content.push('\n');
            let _ = store.write_document(&doc_id, &content, "journal", json!([]));
        }
    }
}

#[async_trait]
impl ToolHandler for HarveyTelegramSendHandler {
    fn name(&self) -> &str {
        "harvey_telegram_send"
    }
    fn description(&self) -> &str {
        "Send a Telegram message ONLY to an existing allow-listed \
         conversation. MCP-initiated unsolicited sends are blocked at \
         the type-system level — this tool returns { ok: false, reason } \
         rather than erroring so clients see structured rejection."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "chat_id":  { "type": "integer", "description": "Telegram chat id (must be allow-listed AND already a live conversation)" },
                "text":     { "type": "string", "description": "Message body" },
                "override": { "type": "boolean", "description": "Explicit override flag; still respects allowlist" }
            },
            "required": ["chat_id", "text"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let chat_id = params
            .get("chat_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| RpcError::invalid_params("missing integer 'chat_id'"))?;
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| RpcError::invalid_params("missing or empty 'text'"))?
            .to_string();

        // Guard 1 — allowlist.
        let allowlist = Self::load_allowlist();
        if !allowlist.contains(&chat_id) {
            self.audit(chat_id, &text, "rejected-not-allowlisted");
            return Ok(json!({
                "ok": false,
                "reason": "MCP-initiated unsolicited messages blocked: chat_id not in HARVEY_TELEGRAM_ALLOWLIST",
            }));
        }

        // Guard 2 — must be an existing conversation in ChatStore.
        // We require chat_store to be wired; if it isn't, fail closed.
        let chat_store = match self.ctx.chat_store.as_ref() {
            Some(s) => s,
            None => {
                self.audit(chat_id, &text, "rejected-chat-store-not-wired");
                return Ok(json!({
                    "ok": false,
                    "reason": "chat store not wired — cannot verify conversation exists",
                }));
            }
        };
        let conversations = chat_store
            .list_conversations(Some("telegram"), 256)
            .map_err(|e| RpcError::internal(format!("chat_store.list failed: {e}")))?;
        let matches = conversations
            .iter()
            .any(|c| c.user_id == chat_id.to_string());
        if !matches {
            self.audit(chat_id, &text, "rejected-no-conversation");
            return Ok(json!({
                "ok": false,
                "reason": "MCP-initiated unsolicited messages blocked: no active conversation for this chat_id",
            }));
        }

        // At this point we would normally hand the message to the
        // teloxide Bot. But the Bot's `run_forever` hard-rule is that
        // outbound sends happen ONLY inside an inbound handler. We
        // therefore stage the message as an assistant append in the
        // ChatStore AND log it to the journal, and the next incoming
        // poll from the recipient will see it as part of history. This
        // matches the Python gateway's "reply-only" rule and leaves no
        // outbound wire path that could fire unsolicited.
        let conv = chat_store
            .get_or_create_conversation("telegram", &chat_id.to_string(), "mcp")
            .map_err(|e| RpcError::internal(format!("chat conversation fetch: {e}")))?;
        let message_id = chat_store
            .append_message(conv.id, "assistant", &text, None)
            .map_err(|e| RpcError::internal(format!("chat append: {e}")))?;
        self.audit(chat_id, &text, "staged");

        Ok(json!({
            "ok": true,
            "message_id": message_id,
            "staged": true,
            "note": "Message staged in chat history. Outbound wire send is gated by the inbound handler for safety."
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// chat_send
// ─────────────────────────────────────────────────────────────────────

pub struct ChatSendHandler {
    ctx: Arc<ToolContext>,
}

impl ChatSendHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChatSendHandler {
    fn name(&self) -> &str {
        "chat_send"
    }
    fn description(&self) -> &str {
        "Append an assistant reply to an existing chat conversation. \
         Does not initiate any outbound transport — only stages the \
         message in the ChatStore for the inbound reply path."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "conversation_id": { "type": "integer" },
                "text": { "type": "string" }
            },
            "required": ["conversation_id", "text"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let conversation_id = params
            .get("conversation_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| RpcError::invalid_params("missing integer 'conversation_id'"))?;
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| RpcError::invalid_params("missing or empty 'text'"))?
            .to_string();
        let chat_store = self
            .ctx
            .chat_store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: chat_store"))?;
        let message_id = chat_store
            .append_message(conversation_id, "assistant", &text, None)
            .map_err(|e| RpcError::internal(format!("chat_send: {e}")))?;
        Ok(json!({
            "message_id": message_id,
            "sent": true,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::ToolContext;
    use makakoo_core::chat::ChatStore;
    use std::path::PathBuf;
    use tokio::sync::Mutex as AsyncMutex;

    // Serialize tests that mutate HARVEY_TELEGRAM_ALLOWLIST so parallel
    // cargo test threads don't race on the same env var. Async mutex so
    // we can hold the guard across `.await` without tripping clippy's
    // await-holding-lock lint.
    static ENV_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

    fn ctx_with_chat() -> (tempfile::TempDir, Arc<ToolContext>) {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(&dir.path().join("chat.db")).unwrap();
        let ctx = Arc::new(
            ToolContext::empty(PathBuf::from("/tmp/mkk-comm"))
                .with_chat(Arc::new(store)),
        );
        (dir, ctx)
    }

    #[tokio::test]
    async fn harvey_telegram_send_blocks_unallowed_chat() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("HARVEY_TELEGRAM_ALLOWLIST");
        let (_dir, ctx) = ctx_with_chat();
        let h = HarveyTelegramSendHandler::new(ctx);
        let out = h
            .call(json!({"chat_id": 999, "text": "hi"}))
            .await
            .unwrap();
        assert!(!out["ok"].as_bool().unwrap());
        let reason = out["reason"].as_str().unwrap();
        assert!(reason.contains("unsolicited") || reason.contains("HARVEY_TELEGRAM_ALLOWLIST"));
    }

    #[tokio::test]
    async fn harvey_telegram_send_blocks_nonexistent_conversation() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("HARVEY_TELEGRAM_ALLOWLIST", "42,99");
        let (_dir, ctx) = ctx_with_chat();
        let h = HarveyTelegramSendHandler::new(ctx);
        let out = h
            .call(json!({"chat_id": 42, "text": "unsolicited hello"}))
            .await
            .unwrap();
        assert!(!out["ok"].as_bool().unwrap());
        assert!(out["reason"]
            .as_str()
            .unwrap()
            .contains("no active conversation"));
        std::env::remove_var("HARVEY_TELEGRAM_ALLOWLIST");
    }

    #[tokio::test]
    async fn harvey_telegram_send_stages_existing_conversation() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("HARVEY_TELEGRAM_ALLOWLIST", "777");
        let (_dir, ctx) = ctx_with_chat();
        // Pre-create a conversation for chat_id=777.
        ctx.chat_store
            .as_ref()
            .unwrap()
            .get_or_create_conversation("telegram", "777", "human")
            .unwrap();
        let h = HarveyTelegramSendHandler::new(Arc::clone(&ctx));
        let out = h
            .call(json!({"chat_id": 777, "text": "scheduled reply"}))
            .await
            .unwrap();
        assert!(out["ok"].as_bool().unwrap());
        assert!(out["staged"].as_bool().unwrap());
        std::env::remove_var("HARVEY_TELEGRAM_ALLOWLIST");
    }

    #[tokio::test]
    async fn chat_send_appends_to_existing_conversation() {
        let (_dir, ctx) = ctx_with_chat();
        // Make a fresh conversation.
        let conv = ctx
            .chat_store
            .as_ref()
            .unwrap()
            .get_or_create_conversation("telegram", "100", "x")
            .unwrap();
        let h = ChatSendHandler::new(Arc::clone(&ctx));
        let out = h
            .call(json!({
                "conversation_id": conv.id,
                "text": "assistant reply"
            }))
            .await
            .unwrap();
        assert!(out["sent"].as_bool().unwrap());
        assert!(out["message_id"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn chat_send_without_chat_store() {
        let ctx = Arc::new(ToolContext::empty(PathBuf::from("/tmp/mkk-comm-2")));
        let h = ChatSendHandler::new(ctx);
        let err = h
            .call(json!({"conversation_id": 1, "text": "x"}))
            .await
            .unwrap_err();
        assert!(err.message.contains("not wired"));
    }
}
