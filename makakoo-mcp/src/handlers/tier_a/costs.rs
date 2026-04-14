//! Tier-A costs_summary — roll up switchAILocal spend across a window.
//! Routes through `CostTracker::summary`.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

pub struct CostsSummaryHandler {
    ctx: Arc<ToolContext>,
}

impl CostsSummaryHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for CostsSummaryHandler {
    fn name(&self) -> &str {
        "costs_summary"
    }
    fn description(&self) -> &str {
        "Roll up LLM / embedding costs for a time window. Windows: \
         'today', '7d', '30d', 'all'."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "string",
                    "enum": ["today", "7d", "30d", "all"],
                    "default": "7d"
                }
            }
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let window = params
            .get("window")
            .and_then(Value::as_str)
            .unwrap_or("7d")
            .to_string();
        if !["today", "7d", "30d", "all"].contains(&window.as_str()) {
            return Err(RpcError::invalid_params(format!(
                "invalid window '{window}'; expected one of today|7d|30d|all"
            )));
        }
        let costs = self
            .ctx
            .costs
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: costs"))?;
        let summary = costs
            .summary(&window)
            .map_err(|e| RpcError::internal(format!("costs_summary: {e}")))?;
        Ok(json!(summary))
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
    async fn rejects_invalid_window() {
        let h = CostsSummaryHandler::new(empty_ctx());
        let err = h.call(json!({"window": "quarter"})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn missing_costs_is_internal() {
        let h = CostsSummaryHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }

    #[test]
    fn schema_default_is_7d() {
        let h = CostsSummaryHandler::new(empty_ctx());
        let schema = h.input_schema();
        assert_eq!(schema["properties"]["window"]["default"], "7d");
    }
}
