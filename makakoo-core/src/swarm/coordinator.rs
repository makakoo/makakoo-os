//! Subagent lifecycle manager.
//!
//! Every subagent dispatch is a `tokio::spawn` holding a `JoinHandle`
//! plus metadata. The coordinator is a typed registry over those handles
//! keyed by subagent id, with status updates flowing through an
//! `Arc<Mutex<SubagentStatus>>` each running task owns alongside the
//! coordinator.
//!
//! Decoupled from LLM specifics: [`AgentCoordinator::spawn`] accepts a
//! runner closure, so tests can plug a canned-response stub and the real
//! `SwarmGateway` can pass an LLM-driven closure.
//!
//! Port notes vs. the Python `core/orchestration/agent_coordinator.py`:
//! the Python version is a DAG-executor glue layer over in-process
//! subagent objects; Rust takes the cleaner "task spawner" approach
//! because tokio gives us real cancellation and join semantics the
//! Python side fakes with BoundedSemaphores.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{MakakooError, Result};

/// Lifecycle state of a single spawned subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubagentStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

impl SubagentStatus {
    pub fn label(&self) -> &'static str {
        match self {
            SubagentStatus::Pending => "pending",
            SubagentStatus::Running => "running",
            SubagentStatus::Completed => "completed",
            SubagentStatus::Failed(_) => "failed",
            SubagentStatus::Cancelled => "cancelled",
        }
    }
}

/// A request to spawn a subagent — the human-readable task, the concrete
/// prompt, plus any shared context payload the runner wants visible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSpec {
    /// Stable name / role, e.g. `"brain-researcher"`, `"olibia"`.
    pub name: String,
    /// Short human description.
    pub task: String,
    /// Full prompt handed to the LLM.
    pub prompt: String,
    /// Any extra JSON the runner should see.
    #[serde(default)]
    pub context: Value,
}

/// Live handle for a spawned subagent.
pub struct SubagentHandle {
    pub id: String,
    pub spec: SubagentSpec,
    pub status: Arc<Mutex<SubagentStatus>>,
    pub join: Option<tokio::task::JoinHandle<Result<Value>>>,
    pub started_at: DateTime<Utc>,
}

impl SubagentHandle {
    pub fn snapshot_status(&self) -> SubagentStatus {
        self.status
            .lock()
            .map(|g| g.clone())
            .unwrap_or(SubagentStatus::Failed("status mutex poisoned".to_string()))
    }
}

/// Registry of live subagents. Cheap to clone — the inner map is shared.
#[derive(Clone, Default)]
pub struct AgentCoordinator {
    active: Arc<Mutex<HashMap<String, SubagentHandle>>>,
}

impl AgentCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, SubagentHandle>>> {
        self.active
            .lock()
            .map_err(|_| MakakooError::internal("agent coordinator mutex poisoned"))
    }

    /// Spawn a subagent. The `runner` closure is called exactly once on a
    /// tokio task; its `Result<Value>` becomes the subagent's final status
    /// (Completed on Ok, Failed on Err). Returns the newly-minted subagent
    /// id.
    pub fn spawn<F, Fut>(&self, spec: SubagentSpec, runner: F) -> Result<String>
    where
        F: FnOnce(SubagentSpec) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        let id = format!("sub-{}", Uuid::new_v4().simple());
        let status = Arc::new(Mutex::new(SubagentStatus::Running));

        // Clone what the task needs. The spec clone is the easy path;
        // the runner takes ownership of the clone.
        let spec_for_runner = spec.clone();
        let status_for_task = Arc::clone(&status);
        let id_for_log = id.clone();
        let join: tokio::task::JoinHandle<Result<Value>> = tokio::spawn(async move {
            let res = runner(spec_for_runner).await;
            let mut guard = status_for_task
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // If the task was cancelled between spawn and here, the status
            // will have been set to Cancelled by cancel(); leave it alone.
            if matches!(*guard, SubagentStatus::Running) {
                *guard = match &res {
                    Ok(_) => SubagentStatus::Completed,
                    Err(e) => SubagentStatus::Failed(e.to_string()),
                };
            }
            tracing::debug!(target: "makakoo.swarm", id = %id_for_log, "subagent task finished");
            res
        });

        let handle = SubagentHandle {
            id: id.clone(),
            spec,
            status,
            join: Some(join),
            started_at: Utc::now(),
        };
        self.lock()?.insert(id.clone(), handle);
        Ok(id)
    }

    /// Await the named subagent, returning its result value.
    pub async fn wait(&self, id: &str) -> Result<Value> {
        // Pop the JoinHandle out of the map so we don't hold the mutex
        // across the await. The rest of the SubagentHandle stays in place.
        let join = {
            let mut map = self.lock()?;
            let entry = map
                .get_mut(id)
                .ok_or_else(|| MakakooError::NotFound(format!("subagent: {id}")))?;
            entry
                .join
                .take()
                .ok_or_else(|| MakakooError::internal("subagent already awaited"))?
        };

        match join.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            Err(join_err) => Err(MakakooError::internal(format!(
                "subagent task panicked: {join_err}"
            ))),
        }
    }

    /// Current status snapshot for `id`, or `None` if unknown.
    pub fn status(&self, id: &str) -> Option<SubagentStatus> {
        let map = self.active.lock().ok()?;
        map.get(id).map(|h| h.snapshot_status())
    }

    /// Snapshot every live subagent as `(id, status)`.
    pub fn list(&self) -> Vec<(String, SubagentStatus)> {
        let map = match self.active.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        map.iter()
            .map(|(k, v)| (k.clone(), v.snapshot_status()))
            .collect()
    }

    /// Cancel a running subagent. Aborts the tokio task and marks the
    /// status as `Cancelled`. No-op if the id is unknown.
    pub fn cancel(&self, id: &str) -> Result<()> {
        let mut map = self.lock()?;
        let entry = map
            .get_mut(id)
            .ok_or_else(|| MakakooError::NotFound(format!("subagent: {id}")))?;
        if let Some(join) = entry.join.as_ref() {
            join.abort();
        }
        if let Ok(mut g) = entry.status.lock() {
            *g = SubagentStatus::Cancelled;
        }
        Ok(())
    }

    /// Total number of registered subagents (living or finished).
    pub fn len(&self) -> usize {
        self.active.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the spec for a given subagent id, or `None`.
    pub fn spec(&self, id: &str) -> Option<SubagentSpec> {
        self.active
            .lock()
            .ok()
            .and_then(|m| m.get(id).map(|h| h.spec.clone()))
    }
}

// Simple uuid shim — avoids pulling in the full `uuid` crate for one
// call. Uses a weak PRNG seeded from the system clock; good enough for
// an in-process id where collisions only matter within one makakoo-mcp
// process.
mod uuid {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static CTR: AtomicU64 = AtomicU64::new(0);

    pub struct Uuid(u128);

    impl Uuid {
        pub fn new_v4() -> Self {
            // 64 bits of unix-nanos time + 64 bits of per-process counter.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let ctr = CTR.fetch_add(1, Ordering::Relaxed);
            Uuid(((now as u128) << 64) | (ctr as u128))
        }

        pub fn simple(&self) -> String {
            format!("{:032x}", self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn spec(name: &str) -> SubagentSpec {
        SubagentSpec {
            name: name.to_string(),
            task: "test task".to_string(),
            prompt: "say hello".to_string(),
            context: Value::Null,
        }
    }

    #[tokio::test]
    async fn spawn_and_wait_completes() {
        let co = AgentCoordinator::new();
        let id = co
            .spawn(spec("echo"), |s| async move {
                Ok(json!({"name": s.name, "reply": "hi"}))
            })
            .unwrap();
        let v = co.wait(&id).await.unwrap();
        assert_eq!(v["name"], "echo");
        assert_eq!(v["reply"], "hi");
        matches!(co.status(&id).unwrap(), SubagentStatus::Completed);
    }

    #[tokio::test]
    async fn failed_runner_is_recorded() {
        let co = AgentCoordinator::new();
        let id = co
            .spawn(spec("bad"), |_s| async move {
                Err(MakakooError::internal("boom"))
            })
            .unwrap();
        let err = co.wait(&id).await.unwrap_err();
        assert!(format!("{err}").contains("boom"));
        match co.status(&id).unwrap() {
            SubagentStatus::Failed(msg) => assert!(msg.contains("boom")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_shows_spawned_subagents() {
        let co = AgentCoordinator::new();
        for i in 0..3 {
            co.spawn(spec(&format!("a{i}")), |_s| async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(json!({}))
            })
            .unwrap();
        }
        assert_eq!(co.list().len(), 3);
        assert_eq!(co.len(), 3);
    }

    #[tokio::test]
    async fn cancel_running_subagent() {
        let co = AgentCoordinator::new();
        let id = co
            .spawn(spec("slow"), |_s| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(json!({}))
            })
            .unwrap();
        // Let the task enter its sleep.
        tokio::time::sleep(Duration::from_millis(20)).await;
        co.cancel(&id).unwrap();
        match co.status(&id).unwrap() {
            SubagentStatus::Cancelled => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_id_not_found() {
        let co = AgentCoordinator::new();
        assert!(co.status("sub-does-not-exist").is_none());
        let err = co.cancel("sub-does-not-exist").unwrap_err();
        assert!(matches!(err, MakakooError::NotFound(_)));
    }
}
