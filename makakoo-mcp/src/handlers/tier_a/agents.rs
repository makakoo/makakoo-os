//! Tier-A agent_list + agent_info — read-only queries over the agent
//! scaffold directory.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

// ─────────────────────────────────────────────────────────────────────
// agent_list
// ─────────────────────────────────────────────────────────────────────

pub struct AgentListHandler {
    ctx: Arc<ToolContext>,
}

impl AgentListHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for AgentListHandler {
    fn name(&self) -> &str {
        "agent_list"
    }
    fn description(&self) -> &str {
        "List every scaffolded agent directory parseable from \
         $MAKAKOO_HOME/agents/<name>/agent.toml."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _params: Value) -> Result<Value, RpcError> {
        let scaffold = self
            .ctx
            .agents
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: agents"))?;
        let list = scaffold
            .list()
            .map_err(|e| RpcError::internal(format!("agent_list: {e}")))?;
        Ok(json!(list))
    }
}

// ─────────────────────────────────────────────────────────────────────
// agent_info
// ─────────────────────────────────────────────────────────────────────

pub struct AgentInfoHandler {
    ctx: Arc<ToolContext>,
}

impl AgentInfoHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for AgentInfoHandler {
    fn name(&self) -> &str {
        "agent_info"
    }
    fn description(&self) -> &str {
        "Return the full AgentSpec for a single scaffolded agent, or null \
         if no such agent exists."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("missing 'name'"))?;
        let scaffold = self
            .ctx
            .agents
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: agents"))?;
        let info = scaffold
            .info(name)
            .map_err(|e| RpcError::internal(format!("agent_info: {e}")))?;
        Ok(json!(info))
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
    async fn list_without_scaffold_is_internal() {
        let h = AgentListHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn info_requires_name() {
        let h = AgentInfoHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }
}
