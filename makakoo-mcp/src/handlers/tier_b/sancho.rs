//! Tier-B `sancho_tick` handler — static response until the Rust SANCHO
//! engine is daemonized and wired into the ToolContext.
//!
//! The Python `sancho_tick` runs one scheduling round of the background
//! nudge engine and returns a list of fired reports. On the Rust side
//! T8 shipped the engine + 8 handlers but the MCP spine does not yet
//! carry an `Arc<SanchoEngine>` field on `ToolContext` — that lands in
//! T17 alongside the launchd daemon install. Until then this handler
//! answers with the contract-safe shape `{ ok, reports: [], note }` so
//! downstream clients can poll without error and the test surface stays
//! deterministic.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

pub struct SanchoTickHandler {
    #[allow(dead_code)]
    ctx: Arc<ToolContext>,
}

impl SanchoTickHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for SanchoTickHandler {
    fn name(&self) -> &str {
        "sancho_tick"
    }
    fn description(&self) -> &str {
        "Run one SANCHO scheduling round and return fired task reports. \
         On the Rust MCP server this is a no-op stub until the engine is \
         daemonized (T17) — it returns { ok: true, reports: [] }."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }
    async fn call(&self, _params: Value) -> Result<Value, RpcError> {
        Ok(json!({
            "ok": true,
            "reports": [],
            "note": "sancho engine not yet daemonized in rust mcp server"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sancho_tick_returns_empty_reports_array() {
        let ctx = Arc::new(ToolContext::empty(std::env::temp_dir()));
        let h = SanchoTickHandler::new(ctx);
        let out = h.call(json!({})).await.unwrap();
        assert_eq!(out["ok"], json!(true));
        assert!(out["reports"].is_array());
        assert_eq!(out["reports"].as_array().unwrap().len(), 0);
    }
}
