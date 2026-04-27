//! Tier-A SANCHO handlers: a read-only engine status probe and the
//! `dream` consolidation tool. The full SANCHO engine runner lives in
//! T8/T15 — this module only surfaces a lightweight read of the current
//! engine state plus an ad-hoc `dream` pass that doesn't depend on the
//! periodic handler registry.

use async_trait::async_trait;
use makakoo_core::llm::ChatMessage as LlmMessage;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

const DREAM_MODEL: &str = "minimax/ail-compound";

// ─────────────────────────────────────────────────────────────────────
// sancho_status — canned engine state
// ─────────────────────────────────────────────────────────────────────

pub struct SanchoStatusHandler {
    #[allow(dead_code)]
    ctx: Arc<ToolContext>,
}

impl SanchoStatusHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for SanchoStatusHandler {
    fn name(&self) -> &str {
        "sancho_status"
    }
    fn description(&self) -> &str {
        "Return the current SANCHO engine state (read-only). T13 surfaces \
         a canned idle status; the full engine runner lands in T15 and \
         will upgrade this endpoint to live handler reports."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _params: Value) -> Result<Value, RpcError> {
        // The SANCHO engine runner isn't bound into the MCP context at
        // T13 — it lives in its own tokio task owned by the daemon. This
        // stub list mirrors `sancho::default_registry()` so consumers see
        // the real task names; update both together when adding handlers.
        // Live tick reporting via the event bus lands in T15.
        Ok(json!({
            "engine": "idle",
            "tasks": [
                "dream",
                "wiki_lint",
                "index_rebuild",
                "daily_briefing",
                "memory_consolidation",
                "memory_promotion",
                "superbrain_sync_embed",
                "dynamic_checklist",
                "switchailocal_watchdog",
                "pg_watchdog",
                "hackernews_monitor",
                "gym_classify",
                "gym_hypothesize",
                "gym_lope_gate",
                "gym_morning_report",
                "gym_weekly_report"
            ],
            "last_tick": null,
            "note": "t13 read-only stub mirroring default_registry(); live reporting lands in t15"
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// dream — ad-hoc consolidation pass
// ─────────────────────────────────────────────────────────────────────

pub struct DreamHandler {
    ctx: Arc<ToolContext>,
}

impl DreamHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for DreamHandler {
    fn name(&self) -> &str {
        "dream"
    }
    fn description(&self) -> &str {
        "Run an ad-hoc dream-style consolidation pass over recent Brain \
         docs. Returns the LLM summary. Does NOT write to the Brain at \
         Tier-A; the writing variant lives in SANCHO's periodic handler."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "default": 20 }
            }
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(20) as usize;

        let store = self
            .ctx
            .store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: store"))?;
        let llm = self
            .ctx
            .llm
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: llm"))?;

        let recent = store
            .recent(limit, None)
            .map_err(|e| RpcError::internal(format!("dream: {e}")))?;
        if recent.is_empty() {
            return Ok(json!({
                "summary": "Brain is empty — nothing to consolidate.",
                "doc_count": 0
            }));
        }

        let joined: String = recent
            .iter()
            .map(|h| {
                let snip: String = h.content.chars().take(160).collect();
                format!("- {}: {}", h.doc_id, snip)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let system = "You are Harvey's dream consolidator. Summarize the \
                      following recent Brain docs into ONE tight paragraph \
                      (<=4 sentences). No preamble, no headers."
            .to_string();
        let user = joined;

        let reply = llm
            .chat(
                DREAM_MODEL,
                vec![LlmMessage::system(system), LlmMessage::user(user)],
            )
            .await
            .map_err(|e| RpcError::internal(format!("dream llm: {e}")))?;
        let summary = reply.trim().to_string();
        Ok(json!({
            "summary": summary,
            "doc_count": recent.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn empty_ctx() -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(PathBuf::from("/tmp/makakoo-t13-test")))
    }

    #[tokio::test]
    async fn sancho_status_returns_idle_shape() {
        let h = SanchoStatusHandler::new(empty_ctx());
        let out = h.call(json!({})).await.unwrap();
        assert_eq!(out["engine"], "idle");
        assert!(out["tasks"].is_array());
        assert!(out["tasks"].as_array().unwrap().len() >= 5);
    }

    #[tokio::test]
    async fn dream_missing_store_returns_internal() {
        let h = DreamHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }
}
