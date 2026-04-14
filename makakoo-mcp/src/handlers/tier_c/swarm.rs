//! Tier-C swarm handlers — dispatch + status + legacy alias.
//!
//! Three tools share one backing subsystem (`makakoo_core::swarm::SwarmGateway`):
//!
//! * `harvey_swarm_run` — dispatch a new subagent, return `{run_id, subagent_id}`.
//! * `harvey_swarm_status` — aggregate status for a given `run_id`.
//! * `swarm` — legacy alias for `harvey_swarm_run`, kept for Python-parity
//!   MCP clients that still call the unprefixed name.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use makakoo_core::swarm::DispatchRequest;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

fn require_str(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RpcError::invalid_params(format!("missing or empty '{key}'")))
}

fn optional_string(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ─────────────────────────────────────────────────────────────────────
// harvey_swarm_run
// ─────────────────────────────────────────────────────────────────────

pub struct HarveySwarmRunHandler {
    ctx: Arc<ToolContext>,
}

impl HarveySwarmRunHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }

    fn input_schema_shared() -> Value {
        json!({
            "type": "object",
            "properties": {
                "task":   { "type": "string", "description": "Human description of what the subagent should do" },
                "prompt": { "type": "string", "description": "Full prompt handed to the LLM" },
                "name":   { "type": "string", "description": "Optional subagent role/name" },
                "model":  { "type": "string", "description": "Optional model override" },
                "parent_run_id": { "type": "string", "description": "Nest this run under a parent run id" }
            },
            "required": ["task", "prompt"]
        })
    }

    async fn run_dispatch(&self, params: Value) -> Result<Value, RpcError> {
        let state = self
            .ctx
            .swarm_state
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: swarm"))?;
        let task = require_str(&params, "task")?;
        let prompt = require_str(&params, "prompt")?;
        let name = optional_string(&params, "name").unwrap_or_else(|| "subagent".to_string());
        let model = optional_string(&params, "model");
        let parent_run_id = optional_string(&params, "parent_run_id");

        let resp = state
            .gateway
            .dispatch(DispatchRequest {
                name,
                task,
                prompt,
                model,
                parent_run_id,
            })
            .await
            .map_err(|e| RpcError::internal(format!("swarm dispatch failed: {e}")))?;
        Ok(json!({
            "run_id": resp.run_id,
            "subagent_id": resp.subagent_id,
            "accepted": resp.accepted,
        }))
    }
}

#[async_trait]
impl ToolHandler for HarveySwarmRunHandler {
    fn name(&self) -> &str {
        "harvey_swarm_run"
    }
    fn description(&self) -> &str {
        "Dispatch a subagent into the swarm. Returns the run id and subagent \
         id immediately; callers poll harvey_swarm_status for progress."
    }
    fn input_schema(&self) -> Value {
        Self::input_schema_shared()
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        self.run_dispatch(params).await
    }
}

// ─────────────────────────────────────────────────────────────────────
// swarm — legacy alias for harvey_swarm_run
// ─────────────────────────────────────────────────────────────────────

pub struct SwarmLegacyHandler {
    inner: HarveySwarmRunHandler,
}

impl SwarmLegacyHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self {
            inner: HarveySwarmRunHandler::new(ctx),
        }
    }
}

#[async_trait]
impl ToolHandler for SwarmLegacyHandler {
    fn name(&self) -> &str {
        "swarm"
    }
    fn description(&self) -> &str {
        "Legacy alias for harvey_swarm_run. Same semantics; kept for \
         Python-parity MCP clients that still call the unprefixed name."
    }
    fn input_schema(&self) -> Value {
        HarveySwarmRunHandler::input_schema_shared()
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        self.inner.run_dispatch(params).await
    }
}

// ─────────────────────────────────────────────────────────────────────
// harvey_swarm_status
// ─────────────────────────────────────────────────────────────────────

pub struct HarveySwarmStatusHandler {
    ctx: Arc<ToolContext>,
}

impl HarveySwarmStatusHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for HarveySwarmStatusHandler {
    fn name(&self) -> &str {
        "harvey_swarm_status"
    }
    fn description(&self) -> &str {
        "Aggregate status for a swarm run id — current subagent state, \
         artifact count, and the latest result artifact if any."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" }
            },
            "required": ["run_id"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let state = self
            .ctx
            .swarm_state
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: swarm"))?;
        let run_id = require_str(&params, "run_id")?;
        let status = state
            .gateway
            .run_status(&run_id)
            .await
            .map_err(|e| RpcError::internal(format!("swarm status failed: {e}")))?;
        Ok(json!({
            "run_id": status.run_id,
            "subagent_id": status.subagent_id,
            "status": status.status,
            "artifact_count": status.artifact_count,
            "latest_result": status.latest_result,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::ToolContext;
    use makakoo_core::db::{open_db, run_migrations};
    use makakoo_core::event_bus::PersistentEventBus;
    use makakoo_core::llm::LlmClient;
    use makakoo_core::swarm::{AgentCoordinator, ArtifactStore, SwarmGateway, SwarmState};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn build_ctx_with_swarm() -> (tempfile::TempDir, Arc<ToolContext>, MockServer) {
        let dir = tempfile::tempdir().unwrap();
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "hello from swarm"}}]
            })))
            .mount(&mock)
            .await;
        let db_path = dir.path().join("main.db");
        let conn = open_db(&db_path).unwrap();
        run_migrations(&conn).unwrap();
        let conn_arc = Arc::new(Mutex::new(conn));
        let artifacts = Arc::new(ArtifactStore::open(Arc::clone(&conn_arc)).unwrap());
        let coordinator = Arc::new(AgentCoordinator::new());
        let llm = Arc::new(LlmClient::with_base_url(format!("{}/v1", mock.uri())));
        let bus = PersistentEventBus::open(&dir.path().join("bus.db")).unwrap();
        let gateway = Arc::new(SwarmGateway::new(
            Arc::clone(&coordinator),
            Arc::clone(&artifacts),
            llm.clone(),
            bus,
        ));
        let state = Arc::new(SwarmState {
            gateway,
            coordinator,
            artifacts,
        });
        let ctx = Arc::new(
            ToolContext::empty(PathBuf::from("/tmp/makakoo-tier-c"))
                .with_llm(llm)
                .with_swarm_state(state),
        );
        (dir, ctx, mock)
    }

    #[tokio::test]
    async fn swarm_run_then_status_round_trip() {
        let (_dir, ctx, _mock) = build_ctx_with_swarm().await;
        let run = HarveySwarmRunHandler::new(Arc::clone(&ctx));
        let dispatched = run
            .call(json!({
                "task": "greet",
                "prompt": "say hi",
                "name": "greeter"
            }))
            .await
            .unwrap();
        let run_id = dispatched["run_id"].as_str().unwrap().to_string();
        let subagent_id = dispatched["subagent_id"].as_str().unwrap().to_string();
        // Wait for the subagent to finish so the Result artifact lands.
        let state = ctx.swarm_state.as_ref().unwrap();
        let _ = state.coordinator.wait(&subagent_id).await.unwrap();
        let status_handler = HarveySwarmStatusHandler::new(Arc::clone(&ctx));
        let status = status_handler
            .call(json!({"run_id": run_id.clone()}))
            .await
            .unwrap();
        assert_eq!(status["run_id"].as_str().unwrap(), run_id);
        assert!(status["artifact_count"].as_u64().unwrap() >= 2);
    }

    #[tokio::test]
    async fn swarm_run_missing_params() {
        let (_dir, ctx, _mock) = build_ctx_with_swarm().await;
        let run = HarveySwarmRunHandler::new(ctx);
        let err = run.call(json!({"task": "no prompt"})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn swarm_legacy_alias_dispatches() {
        let (_dir, ctx, _mock) = build_ctx_with_swarm().await;
        let legacy = SwarmLegacyHandler::new(Arc::clone(&ctx));
        let out = legacy
            .call(json!({"task": "a", "prompt": "b"}))
            .await
            .unwrap();
        assert!(out["accepted"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn swarm_status_returns_not_found() {
        let (_dir, ctx, _mock) = build_ctx_with_swarm().await;
        let status_handler = HarveySwarmStatusHandler::new(ctx);
        let err = status_handler
            .call(json!({"run_id": "nope"}))
            .await
            .unwrap_err();
        assert!(err.message.contains("swarm status failed"));
    }

    #[tokio::test]
    async fn swarm_unwired_returns_internal() {
        let ctx = Arc::new(ToolContext::empty(PathBuf::from("/tmp/mkk")));
        let run = HarveySwarmRunHandler::new(ctx);
        let err = run
            .call(json!({"task": "a", "prompt": "b"}))
            .await
            .unwrap_err();
        assert!(err.message.contains("not wired"));
    }
}
