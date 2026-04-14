//! Tier-A nursery handlers: read-only snapshot of the mascot registry
//! and the buddy tracker.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

// ─────────────────────────────────────────────────────────────────────
// nursery_status — all mascots + the active buddy name
// ─────────────────────────────────────────────────────────────────────

pub struct NurseryStatusHandler {
    ctx: Arc<ToolContext>,
}

impl NurseryStatusHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for NurseryStatusHandler {
    fn name(&self) -> &str {
        "nursery_status"
    }
    fn description(&self) -> &str {
        "Snapshot every mascot in the nursery plus the currently-active buddy."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _params: Value) -> Result<Value, RpcError> {
        let nursery = self
            .ctx
            .nursery
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: nursery"))?;
        let mascots = nursery.all();
        let active_buddy = self
            .ctx
            .buddy
            .as_ref()
            .and_then(|b| b.active().map(|m| m.name));
        Ok(json!({
            "mascots": mascots,
            "active_buddy": active_buddy,
            "count": mascots.len(),
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// buddy_status — active mascot + mood/energy/frame
// ─────────────────────────────────────────────────────────────────────

pub struct BuddyStatusHandler {
    ctx: Arc<ToolContext>,
}

impl BuddyStatusHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for BuddyStatusHandler {
    fn name(&self) -> &str {
        "buddy_status"
    }
    fn description(&self) -> &str {
        "Return the currently-active mascot's name, mood, energy, and its \
         rendered ASCII display frame."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _params: Value) -> Result<Value, RpcError> {
        let buddy = self
            .ctx
            .buddy
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: buddy"))?;
        let state = buddy.state();
        let frame = buddy.display_frame();
        Ok(json!({
            "active": state.active,
            "mood": state.mood,
            "energy": state.energy,
            "last_interaction": state.last_interaction.to_rfc3339(),
            "frame": frame,
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
    async fn nursery_missing_subsystem_is_internal() {
        let h = NurseryStatusHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn buddy_missing_subsystem_is_internal() {
        let h = BuddyStatusHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }
}
