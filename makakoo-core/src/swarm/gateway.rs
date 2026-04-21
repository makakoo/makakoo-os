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
use super::team::TeamRoster;

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
    /// When set, this dispatch bypasses the internal LlmClient and runs
    /// against a registered adapter via the universal-bridge pipeline.
    /// The Result artifact carries the verdict's rationale as content.
    #[serde(default)]
    pub adapter: Option<String>,
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

/// Request to dispatch an entire team roster as a coordinated run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamDispatchRequest {
    /// Roster to run (see `TeamComposition::available_names`).
    pub team: String,
    /// User-level prompt — applied to every role via `input_template`.
    pub prompt: String,
    /// Optional parallelism override (research_team only).
    #[serde(default)]
    pub parallelism: Option<usize>,
    /// Optional model override for every subagent.
    #[serde(default)]
    pub model: Option<String>,
}

/// Response from a team dispatch — one `run_id` covers every member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamDispatchResponse {
    pub run_id: String,
    pub team: String,
    pub subagent_ids: Vec<String>,
    pub total_members: usize,
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
                "adapter": req.adapter.clone(),
            }),
        };

        let adapter_for_runner = req.adapter.clone();

        let subagent_id = self.coordinator.spawn(spec, move |s| {
            let artifacts = Arc::clone(&artifacts);
            let llm = Arc::clone(&llm);
            let bus = Arc::clone(&bus);
            let run_id = run_id_for_runner.clone();
            let agent = agent_for_runner.clone();
            let model = model_for_runner.clone();
            let adapter = adapter_for_runner.clone();
            async move {
                // Adapter-backed dispatch path — runs against a registered
                // universal-bridge adapter instead of the LlmClient. The
                // Result artifact content is the verdict's rationale so
                // downstream consumers (run_status, tests) read one stable
                // shape regardless of which path produced it.
                if let Some(adapter_name) = adapter {
                    use crate::adapter::{call_adapter, AdapterRegistry, CallContext};
                    let registry = match AdapterRegistry::load_default() {
                        Ok(r) => r,
                        Err(e) => {
                            return Err(crate::error::MakakooError::internal(format!(
                                "adapter registry unavailable: {e}"
                            )));
                        }
                    };
                    let manifest = match registry.get(&adapter_name) {
                        Some(r) => r.manifest.clone(),
                        None => {
                            return Err(crate::error::MakakooError::internal(format!(
                                "adapter `{adapter_name}` not registered"
                            )));
                        }
                    };
                    let ctx = CallContext::default().with_timeout(120);
                    let result = call_adapter(&manifest, &s.prompt, ctx).await;
                    let content = result.verdict.rationale.clone();
                    let status = result.verdict.status.as_str().to_string();
                    let art = Artifact {
                        id: 0,
                        kind: ArtifactKind::Result,
                        run_id: run_id.clone(),
                        parent_id: None,
                        agent: agent.clone(),
                        content: content.clone(),
                        metadata: json!({
                            "adapter": adapter_name,
                            "status": status,
                            "confidence": result.verdict.confidence,
                        }),
                        created_at: chrono::Utc::now(),
                    };
                    if let Err(e) = artifacts.write(art) {
                        tracing::warn!(
                            target: "makakoo.swarm",
                            "adapter dispatch artifact write failed: {e}"
                        );
                    }
                    let _ = bus.publish(
                        "swarm.dispatch.complete",
                        "swarm-gateway",
                        json!({
                            "run_id": run_id,
                            "agent": agent,
                            "adapter": adapter_name,
                            "status": status,
                        }),
                    );
                    return Ok(json!({
                        "run_id": run_id,
                        "adapter": adapter_name,
                        "status": status,
                        "content": content,
                    }));
                }

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

    /// Dispatch an entire team roster under one `run_id`. Every member
    /// becomes a subagent; dependencies are expressed via `depends_on_roles`
    /// and must be respected by the caller when awaiting results. This
    /// method fires all subagents concurrently and returns immediately —
    /// ordering/join is the caller's responsibility.
    pub async fn dispatch_team(
        &self,
        roster: &TeamRoster,
        req: TeamDispatchRequest,
    ) -> Result<TeamDispatchResponse> {
        let run_id = format!("swarm-team-{}-{}", roster.name, mint_run_suffix());
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_DISPATCH_MODEL.to_string());

        // Team plan artifact — single upfront record for the whole team.
        let plan = Artifact {
            id: 0,
            kind: ArtifactKind::Plan,
            run_id: run_id.clone(),
            parent_id: None,
            agent: format!("team::{}", roster.name),
            content: req.prompt.clone(),
            metadata: json!({
                "team": roster.name,
                "members": roster.members.len(),
                "total_steps": roster.total_steps(),
                "model": model,
            }),
            created_at: chrono::Utc::now(),
        };
        let _ = self.artifacts.write(plan)?;

        let _ = self.bus.publish(
            "swarm.team.start",
            "swarm-gateway",
            json!({
                "run_id": run_id,
                "team": roster.name,
                "members": roster.members.len(),
            }),
        );

        let mut subagent_ids: Vec<String> = Vec::new();
        for member in &roster.members {
            for instance_ix in 0..member.count {
                let name = if member.count > 1 {
                    format!("{}#{instance_ix}", member.role)
                } else {
                    member.role.clone()
                };
                let task = format!("{} :: {}", member.agent, member.action);
                let prompt = compose_member_prompt(member, &req.prompt);
                let child = DispatchRequest {
                    name,
                    task,
                    prompt,
                    model: Some(model.clone()),
                    parent_run_id: Some(run_id.clone()),
                    adapter: None,
                };
                let resp = self.dispatch(child).await?;
                subagent_ids.push(resp.subagent_id);
            }
        }

        Ok(TeamDispatchResponse {
            run_id,
            team: roster.name.clone(),
            subagent_ids: subagent_ids.clone(),
            total_members: subagent_ids.len(),
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

fn compose_member_prompt(member: &super::team::TeamMember, user_prompt: &str) -> String {
    // The Python agent_team builds a formatted dict via `input_template`;
    // in the Rust path we just append the user prompt plus role context
    // so the subagent's LLM has enough to act on.
    format!(
        "Role: {role} (agent={agent}, action={action}).\n\n{user}",
        role = if member.role.is_empty() {
            &member.agent
        } else {
            &member.role
        },
        agent = member.agent,
        action = member.action,
        user = user_prompt,
    )
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
                adapter: None,
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
                adapter: None,
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
                adapter: None,
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

    #[tokio::test]
    async fn dispatch_team_spawns_every_member() {
        use crate::swarm::team::TeamComposition;
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "ack"}}]
            })))
            .mount(&mock)
            .await;
        let (_dir, gw) = build_gateway(&mock).await;
        let roster = TeamComposition::research_team(3);
        let resp = gw
            .dispatch_team(
                &roster,
                TeamDispatchRequest {
                    team: roster.name.clone(),
                    prompt: "find me signals".into(),
                    parallelism: None,
                    model: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.team, "research_team");
        // research_team(3) = 3 researchers + 1 synthesizer + 1 storage = 5.
        assert_eq!(resp.total_members, 5);
        assert_eq!(resp.subagent_ids.len(), 5);
        for sid in &resp.subagent_ids {
            let _ = gw.coordinator.wait(sid).await;
        }
        let arts = gw.artifacts.by_run(&resp.run_id).unwrap();
        assert!(
            arts.iter().any(|a| a.kind == ArtifactKind::Plan
                && a.agent == "team::research_team"),
            "missing team-level Plan artifact",
        );
    }

    #[tokio::test]
    async fn dispatch_team_minimal_single_member() {
        use crate::swarm::team::TeamComposition;
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "ok"}}]
            })))
            .mount(&mock)
            .await;
        let (_dir, gw) = build_gateway(&mock).await;
        let roster = TeamComposition::minimal_team();
        let resp = gw
            .dispatch_team(
                &roster,
                TeamDispatchRequest {
                    team: roster.name.clone(),
                    prompt: "ping".into(),
                    parallelism: None,
                    model: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.total_members, 1);
        let _ = gw.coordinator.wait(&resp.subagent_ids[0]).await;
    }
}
