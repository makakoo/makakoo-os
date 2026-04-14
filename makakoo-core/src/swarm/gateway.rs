//! SwarmGateway — public dispatch surface for the swarm subsystem.
//!
//! Wraps [`AgentCoordinator`] (lifecycle) + [`ArtifactStore`] (persistent
//! trace) + [`LlmClient`] (actual inference) + [`PersistentEventBus`]
//! (cross-process notifications) into a single dispatch API the MCP
//! tool handlers and the CLI can both drive.
//!
//! One dispatch → one `run_id` → one subagent, which:
//!
//! 1. Calls `llm.chat(model, [user: prompt])`.
//! 2. Writes a `Result` artifact with the raw LLM output.
//! 3. Publishes `swarm.dispatch.complete` on the event bus.
//!
//! Errors from the LLM call become `Failed` subagent status AND a `Log`
//! artifact AND a `swarm.dispatch.failed` event.
//!
//! Global init is available via [`SwarmGateway::install`] /
//! [`SwarmGateway::global`] using `OnceCell`.

use std::sync::Arc;

use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{MakakooError, Result};
use crate::event_bus::PersistentEventBus;
use crate::llm::{ChatMessage as LlmMessage, LlmClient};

use super::artifacts::{Artifact, ArtifactKind, ArtifactStore};
use super::coordinator::{AgentCoordinator, SubagentSpec, SubagentStatus};

/// Default chat-completion model for dispatched subagents. Matches the
/// rest of makakoo-core.
pub const DEFAULT_DISPATCH_MODEL: &str = "ail-compound";

/// A dispatch request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchRequest {
    pub name: String,
    pub task: String,
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub parent_run_id: Option<String>,
}

/// Immediate dispatch acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchResponse {
    pub run_id: String,
    pub subagent_id: String,
    pub accepted: bool,
}

/// Aggregated run status returned by `run_status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmRunStatus {
    pub run_id: String,
    pub subagent_id: String,
    pub status: String,
    pub artifact_count: usize,
    pub latest_result: Option<Value>,
}

static GLOBAL_GATEWAY: OnceCell<Arc<SwarmGateway>> = OnceCell::new();

/// Public dispatch surface over coordinator + artifact + llm + bus.
pub struct SwarmGateway {
    coordinator: Arc<AgentCoordinator>,
    artifacts: Arc<ArtifactStore>,
    llm: Arc<LlmClient>,
    bus: Arc<PersistentEventBus>,
}

impl SwarmGateway {
    pub fn new(
        coordinator: Arc<AgentCoordinator>,
        artifacts: Arc<ArtifactStore>,
        llm: Arc<LlmClient>,
        bus: Arc<PersistentEventBus>,
    ) -> Self {
        Self {
            coordinator,
            artifacts,
            llm,
            bus,
        }
    }

    /// Install the process-global gateway. First caller wins.
    pub fn install(gateway: Arc<SwarmGateway>) -> Result<Arc<SwarmGateway>> {
        GLOBAL_GATEWAY
            .set(gateway)
            .map_err(|_| MakakooError::internal("SwarmGateway already installed"))?;
        Ok(Arc::clone(GLOBAL_GATEWAY.get().expect("just set")))
    }

    /// Process-global gateway, if installed.
    pub fn global() -> Option<Arc<SwarmGateway>> {
        GLOBAL_GATEWAY.get().map(Arc::clone)
    }

    /// Kick off a dispatch. Spawns a subagent via the coordinator using
    /// an LLM-driven runner that writes artifacts and publishes events.
    /// Returns the freshly minted `run_id` + `subagent_id`.
    pub async fn dispatch(&self, req: DispatchRequest) -> Result<DispatchResponse> {
        let run_id = match req.parent_run_id.as_deref() {
            Some(parent) => format!("{parent}::{}", mint_run_suffix()),
            None => format!("swarm-run-{}", mint_run_suffix()),
        };
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_DISPATCH_MODEL.to_string());
        let agent_name = req.name.clone();
        let prompt = req.prompt.clone();

        // Shared handles cloned into the runner closure.
        let artifacts = Arc::clone(&self.artifacts);
        let llm = Arc::clone(&self.llm);
        let bus = Arc::clone(&self.bus);
        let run_id_for_runner = run_id.clone();
        let agent_for_runner = agent_name.clone();
        let model_for_runner = model.clone();

        // Record the upfront "plan" artifact — the prompt itself.
        let plan = Artifact {
            id: 0,
            kind: ArtifactKind::Plan,
            run_id: run_id.clone(),
            parent_id: None,
            agent: agent_name.clone(),
            content: prompt.clone(),
            metadata: json!({
                "task": req.task,
                "model": model,
            }),
            created_at: chrono::Utc::now(),
        };
        let _ = artifacts.write(plan)?;

        let _ = bus.publish(
            "swarm.dispatch.start",
            "swarm-gateway",
            json!({
                "run_id": run_id,
                "agent": agent_name,
                "model": model,
            }),
        );

        let spec = SubagentSpec {
            name: agent_name.clone(),
            task: req.task.clone(),
            prompt: prompt.clone(),
            context: json!({
                "run_id": run_id,
                "model": model,
            }),
        };

        let subagent_id = self.coordinator.spawn(spec, move |s| {
            let artifacts = Arc::clone(&artifacts);
            let llm = Arc::clone(&llm);
            let bus = Arc::clone(&bus);
            let run_id = run_id_for_runner.clone();
            let agent = agent_for_runner.clone();
            let model = model_for_runner.clone();
            async move {
                let messages = vec![LlmMessage::user(s.prompt.clone())];
                match llm.chat(&model, messages).await {
                    Ok(content) => {
                        // Persist the result as a Result artifact.
                        let art = Artifact {
                            id: 0,
                            kind: ArtifactKind::Result,
                            run_id: run_id.clone(),
                            parent_id: None,
                            agent: agent.clone(),
                            content: content.clone(),
                            metadata: json!({"model": model}),
                            created_at: chrono::Utc::now(),
                        };
                        if let Err(e) = artifacts.write(art) {
                            tracing::warn!(
                                target: "makakoo.swarm",
                                "dispatch artifact write failed: {e}"
                            );
                        }
                        let _ = bus.publish(
                            "swarm.dispatch.complete",
                            "swarm-gateway",
                            json!({
                                "run_id": run_id,
                                "agent": agent,
                                "content_len": content.len(),
                            }),
                        );
                        Ok(json!({
                            "run_id": run_id,
                            "content": content,
                        }))
                    }
                    Err(e) => {
                        let err_msg = format!("{e}");
                        // Log the failure as a Log artifact for audit.
                        let _ = artifacts.write(Artifact {
                            id: 0,
                            kind: ArtifactKind::Log,
                            run_id: run_id.clone(),
                            parent_id: None,
                            agent: agent.clone(),
                            content: err_msg.clone(),
                            metadata: json!({"level": "error", "model": model}),
                            created_at: chrono::Utc::now(),
                        });
                        let _ = bus.publish(
                            "swarm.dispatch.failed",
                            "swarm-gateway",
                            json!({
                                "run_id": run_id,
                                "agent": agent,
                                "error": err_msg,
                            }),
                        );
                        Err(e)
                    }
                }
            }
        })?;

        Ok(DispatchResponse {
            run_id,
            subagent_id,
            accepted: true,
        })
    }

    /// Aggregated view of a run — combines coordinator status (most
    /// recent spawn with that run_id in its context) with artifact
    /// history from the artifact store.
    pub async fn run_status(&self, run_id: &str) -> Result<SwarmRunStatus> {
        let artifacts = self.artifacts.by_run(run_id)?;
        if artifacts.is_empty() {
            return Err(MakakooError::NotFound(format!(
                "swarm run not found: {run_id}"
            )));
        }
        // Find the most recent subagent id whose context's run_id matches.
        let mut subagent_id = String::new();
        let mut status_label = "unknown".to_string();
        for (sid, status) in self.coordinator.list() {
            if let Some(spec) = self.coordinator.spec(&sid) {
                if spec
                    .context
                    .get("run_id")
                    .and_then(Value::as_str)
                    .map(|s| s == run_id)
                    .unwrap_or(false)
                {
                    subagent_id = sid;
                    status_label = status_str(&status);
                    break;
                }
            }
        }
        let latest_result = self
            .artifacts
            .latest(run_id, ArtifactKind::Result)?
            .map(|a| {
                json!({
                    "id": a.id,
                    "agent": a.agent,
                    "content": a.content,
                    "metadata": a.metadata,
                })
            });
        Ok(SwarmRunStatus {
            run_id: run_id.to_string(),
            subagent_id,
            status: status_label,
            artifact_count: artifacts.len(),
            latest_result,
        })
    }

    /// Access the underlying coordinator.
    pub fn coordinator(&self) -> &Arc<AgentCoordinator> {
        &self.coordinator
    }

    /// Access the underlying artifact store.
    pub fn artifacts(&self) -> &Arc<ArtifactStore> {
        &self.artifacts
    }
}

fn status_str(s: &SubagentStatus) -> String {
    match s {
        SubagentStatus::Pending => "pending".to_string(),
        SubagentStatus::Running => "running".to_string(),
        SubagentStatus::Completed => "completed".to_string(),
        SubagentStatus::Failed(msg) => format!("failed: {msg}"),
        SubagentStatus::Cancelled => "cancelled".to_string(),
    }
}

fn mint_run_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    format!("{now:x}-{c:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn build_gateway(
        mock: &MockServer,
    ) -> (tempfile::TempDir, Arc<SwarmGateway>) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("swarm.db");
        let conn = open_db(&db_path).unwrap();
        run_migrations(&conn).unwrap();
        let conn_arc = Arc::new(Mutex::new(conn));
        let artifacts = Arc::new(ArtifactStore::open(Arc::clone(&conn_arc)).unwrap());
        let coordinator = Arc::new(AgentCoordinator::new());
        let llm = Arc::new(LlmClient::with_base_url(format!("{}/v1", mock.uri())));
        let bus = PersistentEventBus::open(&dir.path().join("bus.db")).unwrap();
        let gw = Arc::new(SwarmGateway::new(coordinator, artifacts, llm, bus));
        (dir, gw)
    }

    #[tokio::test]
    async fn dispatch_writes_plan_and_result_artifacts() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "okay"}}]
            })))
            .mount(&mock)
            .await;
        let (_dir, gw) = build_gateway(&mock).await;
        let resp = gw
            .dispatch(DispatchRequest {
                name: "tester".into(),
                task: "say hi".into(),
                prompt: "please say hi".into(),
                model: None,
                parent_run_id: None,
            })
            .await
            .unwrap();
        // Wait for the runner task to finish.
        let _ = gw.coordinator.wait(&resp.subagent_id).await.unwrap();
        let run_artifacts = gw.artifacts.by_run(&resp.run_id).unwrap();
        assert!(
            run_artifacts.iter().any(|a| a.kind == ArtifactKind::Plan),
            "missing Plan artifact"
        );
        assert!(
            run_artifacts
                .iter()
                .any(|a| a.kind == ArtifactKind::Result && a.content == "okay"),
            "missing Result artifact"
        );
    }

    #[tokio::test]
    async fn dispatch_failure_writes_log_artifact() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock)
            .await;
        let (_dir, gw) = build_gateway(&mock).await;
        let resp = gw
            .dispatch(DispatchRequest {
                name: "fail".into(),
                task: "boom".into(),
                prompt: "crash please".into(),
                model: None,
                parent_run_id: None,
            })
            .await
            .unwrap();
        let _ = gw.coordinator.wait(&resp.subagent_id).await;
        let run_artifacts = gw.artifacts.by_run(&resp.run_id).unwrap();
        assert!(run_artifacts.iter().any(|a| a.kind == ArtifactKind::Plan));
        assert!(run_artifacts.iter().any(|a| a.kind == ArtifactKind::Log));
        assert!(
            !run_artifacts.iter().any(|a| a.kind == ArtifactKind::Result),
            "result artifact should not be written on failure"
        );
    }

    #[tokio::test]
    async fn run_status_surfaces_latest_result() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "final answer"}}]
            })))
            .mount(&mock)
            .await;
        let (_dir, gw) = build_gateway(&mock).await;
        let resp = gw
            .dispatch(DispatchRequest {
                name: "tester".into(),
                task: "do it".into(),
                prompt: "go".into(),
                model: None,
                parent_run_id: None,
            })
            .await
            .unwrap();
        let _ = gw.coordinator.wait(&resp.subagent_id).await.unwrap();
        let status = gw.run_status(&resp.run_id).await.unwrap();
        assert_eq!(status.run_id, resp.run_id);
        assert!(status.artifact_count >= 2);
        let latest = status.latest_result.unwrap();
        assert_eq!(latest["content"].as_str().unwrap(), "final answer");
    }

    #[tokio::test]
    async fn run_status_unknown_is_error() {
        let mock = MockServer::start().await;
        let (_dir, gw) = build_gateway(&mock).await;
        let err = gw.run_status("not-a-run").await.unwrap_err();
        assert!(matches!(err, MakakooError::NotFound(_)));
    }
}
