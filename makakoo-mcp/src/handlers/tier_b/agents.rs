//! Tier-B agent-scaffold handlers: install, uninstall, create.
//!
//! Thin MCP wrappers over `makakoo_core::agents::AgentScaffold`. Each
//! operation is filesystem-bound (`{home}/agents/<name>/`) and rejects
//! invalid names + duplicates at the scaffold layer. `uninstall` also
//! refuses to delete a directory whose files hold an exclusive fs2
//! lock — i.e. a running agent — so you can't yank the rug out from
//! under a live process via an MCP call.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

use makakoo_core::agents::AgentSpec;

fn spec_to_json(spec: &AgentSpec) -> Value {
    json!({
        "name": spec.name,
        "kind": spec.kind,
        "entry": spec.entry,
        "description": spec.description,
        "version": spec.version,
        "created_at": spec.created_at.to_rfc3339(),
        "maintainer": spec.maintainer,
        "patrol_interval_min": spec.patrol_interval_min,
    })
}

// ─────────────────────────────────────────────────────────────────────
// agent_install
// ─────────────────────────────────────────────────────────────────────

pub struct AgentInstallHandler {
    ctx: Arc<ToolContext>,
}

impl AgentInstallHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for AgentInstallHandler {
    fn name(&self) -> &str {
        "agent_install"
    }
    fn description(&self) -> &str {
        "Install an agent from a source directory containing agent.toml. \
         Rejects duplicates and invalid names."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "src_dir": { "type": "string" }
            },
            "required": ["src_dir"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let src_dir = params
            .get("src_dir")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'src_dir'"))?;
        let scaffold = self
            .ctx
            .agents
            .as_ref()
            .ok_or_else(|| RpcError::internal("agent scaffold not wired"))?;
        let spec = scaffold
            .install(&PathBuf::from(src_dir))
            .map_err(|e| RpcError::internal(format!("agent_install: {e}")))?;
        Ok(spec_to_json(&spec))
    }
}

// ─────────────────────────────────────────────────────────────────────
// agent_uninstall
// ─────────────────────────────────────────────────────────────────────

pub struct AgentUninstallHandler {
    ctx: Arc<ToolContext>,
}

impl AgentUninstallHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for AgentUninstallHandler {
    fn name(&self) -> &str {
        "agent_uninstall"
    }
    fn description(&self) -> &str {
        "Remove an installed agent directory. Refuses to delete an agent \
         whose files are currently held under exclusive fs2 lock \
         (meaning a running process)."
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
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'name'"))?;
        let scaffold = self
            .ctx
            .agents
            .as_ref()
            .ok_or_else(|| RpcError::internal("agent scaffold not wired"))?;
        scaffold
            .uninstall(name)
            .map_err(|e| RpcError::internal(format!("agent_uninstall: {e}")))?;
        Ok(json!({ "ok": true }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// agent_create
// ─────────────────────────────────────────────────────────────────────

pub struct AgentCreateHandler {
    ctx: Arc<ToolContext>,
}

impl AgentCreateHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for AgentCreateHandler {
    fn name(&self) -> &str {
        "agent_create"
    }
    fn description(&self) -> &str {
        "Scaffold a new agent directory under {home}/agents/<name>/ with \
         agent.toml, README.md, and a stub entry file for the given kind \
         (python | rust | shell)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "kind": { "type": "string", "enum": ["python", "rust", "shell"] },
                "description": { "type": "string" }
            },
            "required": ["name", "kind", "description"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'name'"))?;
        let kind = params
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'kind'"))?;
        let description = params
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'description'"))?;

        let scaffold = self
            .ctx
            .agents
            .as_ref()
            .ok_or_else(|| RpcError::internal("agent scaffold not wired"))?;
        let spec = scaffold
            .create(name, kind, description)
            .map_err(|e| RpcError::internal(format!("agent_create: {e}")))?;
        Ok(spec_to_json(&spec))
    }
}

// ═════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::agents::AgentScaffold;

    fn ctx() -> (tempfile::TempDir, Arc<ToolContext>) {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let scaffold = Arc::new(AgentScaffold::new(agents));
        let c = ToolContext::empty(dir.path().to_path_buf()).with_agents(scaffold);
        (dir, Arc::new(c))
    }

    #[tokio::test]
    async fn agent_create_scaffolds_python_dir() {
        let (_d, ctx) = ctx();
        let h = AgentCreateHandler::new(ctx.clone());
        let out = h
            .call(json!({
                "name": "weather-bot",
                "kind": "python",
                "description": "weather alerts"
            }))
            .await
            .unwrap();
        assert_eq!(out["name"], json!("weather-bot"));
        assert_eq!(out["kind"], json!("python"));
    }

    #[tokio::test]
    async fn agent_create_rejects_duplicate() {
        let (_d, ctx) = ctx();
        let h = AgentCreateHandler::new(ctx.clone());
        h.call(json!({
            "name": "dup",
            "kind": "shell",
            "description": ""
        }))
        .await
        .unwrap();
        let err = h
            .call(json!({
                "name": "dup",
                "kind": "shell",
                "description": ""
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
        assert!(err.message.contains("already exists"));
    }

    #[tokio::test]
    async fn agent_uninstall_removes_created_agent() {
        let (_d, ctx) = ctx();
        let create = AgentCreateHandler::new(ctx.clone());
        create
            .call(json!({
                "name": "temp",
                "kind": "python",
                "description": ""
            }))
            .await
            .unwrap();
        let uninstall = AgentUninstallHandler::new(ctx.clone());
        let out = uninstall.call(json!({ "name": "temp" })).await.unwrap();
        assert_eq!(out["ok"], json!(true));
    }
}
