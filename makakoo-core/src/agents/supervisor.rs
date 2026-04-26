//! Per-slot supervisor — Phase 1 of v2-mega.
//!
//! ONE supervisor per agent slot. Spawns the Python LLM gateway as a
//! single child process, hosts the transport adapters in-process,
//! holds `TransportStatusHandle` clones, and writes
//! `~/MAKAKOO/run/agents/<slot>/status.json` every 5s.
//!
//! Distinct from `agents::lifecycle::AgentSupervisor` (legacy plugin
//! lifecycle for `kind=agent` plugins). The new `SlotSupervisor`
//! owns multi-bot subagent slots specifically.
//!
//! Key behaviours:
//!
//! * **Restart budget** — gateway crashes within a sliding 60-second
//!   window are tracked. Up to 5 crashes/minute trigger exponential-
//!   backoff respawn (500ms→30s, jittered). The 6th crash within
//!   60s opens the circuit breaker: state flips to `Crashed` and no
//!   further respawns happen until reset.
//!
//! * **Status writer** — JSON snapshot of supervisor + gateway PIDs +
//!   transport status flushed every 5 seconds (configurable via
//!   `STATUS_WRITE_INTERVAL`). The CLI's `agent status` reads this
//!   file rather than RPC-ing the supervisor; a hung supervisor
//!   shows stale `last_frame` but the file still parses.
//!
//! * **Graceful shutdown** — SIGTERM to the supervisor sends SIGTERM
//!   to the gateway child, waits up to `SHUTDOWN_GRACE`, then
//!   SIGKILLs. Status file is rewritten with `gateway: dead` before
//!   exit so the next `agent status` is honest.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agents::status::{GatewayStatus, TransportStatusHandle};
use crate::error::{MakakooError, Result};

/// How often the status writer flushes status.json.
pub const STATUS_WRITE_INTERVAL: Duration = Duration::from_secs(5);

/// Grace period given to the gateway child to exit on SIGTERM
/// before the supervisor escalates to SIGKILL.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Sliding window for restart budget tracking.
pub const RESTART_BUDGET_WINDOW: Duration = Duration::from_secs(60);

/// Crashes-per-window beyond which the circuit breaker trips.
pub const RESTART_BUDGET_LIMIT: usize = 5;

/// Backoff bounds for the exponential schedule.
pub const BACKOFF_MIN: Duration = Duration::from_millis(500);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Supervisor lifecycle state. Persisted to status.json so the CLI
/// can render it without an RPC round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorState {
    /// Bootstrapping — supervisor up, gateway not yet spawned.
    Starting,
    /// Gateway running and responsive.
    Running,
    /// Gateway crashed; backoff in progress before next respawn.
    Restarting,
    /// Restart budget exhausted; circuit broken. Manual `agent
    /// restart` required.
    Crashed,
    /// Operator-initiated shutdown in progress or complete.
    Stopped,
}

/// Where the supervisor writes its status.json + PID file.
pub fn run_dir(makakoo_home: &Path, slot_id: &str) -> PathBuf {
    makakoo_home.join("run/agents").join(slot_id)
}

/// status.json is the lingua franca between supervisor and `agent
/// status` CLI. Schema is intentionally narrow — adding fields
/// requires a coordinated CLI release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorStatusFile {
    pub slot_id: String,
    pub state: SupervisorState,
    pub supervisor_pid: u32,
    pub gateway: GatewayStatus,
    pub transports: Vec<crate::agents::status::TransportStatusSnapshot>,
    pub restart_count: u32,
    pub circuit_break_until: Option<DateTime<Utc>>,
    pub written_at: DateTime<Utc>,
}

impl SupervisorStatusFile {
    /// Serialize and atomically write to `status.json` in the slot's
    /// run dir. Atomic = write to `.tmp`, rename onto target. Avoids
    /// partial-read races with the CLI.
    pub fn write_atomic(&self, run_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(run_dir).map_err(|e| {
            MakakooError::Internal(format!("create run_dir: {e}"))
        })?;
        let target = run_dir.join("status.json");
        let tmp = run_dir.join("status.json.tmp");
        let body = serde_json::to_vec_pretty(self)
            .map_err(|e| MakakooError::Internal(format!("serialize status: {e}")))?;
        std::fs::write(&tmp, body)
            .map_err(|e| MakakooError::Internal(format!("write status.tmp: {e}")))?;
        std::fs::rename(&tmp, &target)
            .map_err(|e| MakakooError::Internal(format!("rename status: {e}")))?;
        Ok(())
    }

    /// Read status.json from a slot's run dir. Returns Ok(None) if
    /// the file is missing (slot not supervised).
    pub fn read(run_dir: &Path) -> Result<Option<Self>> {
        let path = run_dir.join("status.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                let parsed: Self = serde_json::from_slice(&bytes)
                    .map_err(|e| MakakooError::Internal(format!("parse status: {e}")))?;
                Ok(Some(parsed))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(MakakooError::Internal(format!("read status: {e}"))),
        }
    }
}

/// Tracks crash timestamps in a sliding window and decides whether
/// to respawn or trip the circuit breaker.
#[derive(Debug, Default)]
pub struct RestartBudget {
    crashes: VecDeque<DateTime<Utc>>,
    /// When the breaker tripped, if it has.
    tripped_at: Option<DateTime<Utc>>,
}

impl RestartBudget {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one crash. Returns the decision the supervisor should
    /// take.
    pub fn record_crash(&mut self, now: DateTime<Utc>) -> RestartDecision {
        self.trim(now);
        self.crashes.push_back(now);
        if self.crashes.len() > RESTART_BUDGET_LIMIT {
            self.tripped_at = Some(now);
            RestartDecision::CircuitBreak
        } else {
            let attempt = (self.crashes.len() - 1) as u32;
            RestartDecision::Backoff(backoff_for_attempt(attempt))
        }
    }

    pub fn is_tripped(&self) -> bool {
        self.tripped_at.is_some()
    }

    pub fn tripped_at(&self) -> Option<DateTime<Utc>> {
        self.tripped_at
    }

    pub fn count(&self) -> usize {
        self.crashes.len()
    }

    pub fn reset(&mut self) {
        self.crashes.clear();
        self.tripped_at = None;
    }

    fn trim(&mut self, now: DateTime<Utc>) {
        let cutoff = now - chrono::Duration::from_std(RESTART_BUDGET_WINDOW).unwrap();
        while let Some(t) = self.crashes.front() {
            if *t < cutoff {
                self.crashes.pop_front();
            } else {
                break;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Sleep this long, then respawn.
    Backoff(Duration),
    /// Budget exhausted. Trip circuit breaker; do not respawn.
    CircuitBreak,
}

/// Exponential backoff jittered into [BACKOFF_MIN, BACKOFF_MAX].
/// `attempt = 0` returns BACKOFF_MIN.
pub fn backoff_for_attempt(attempt: u32) -> Duration {
    let base_ms = BACKOFF_MIN.as_millis() as u64;
    let max_ms = BACKOFF_MAX.as_millis() as u64;
    let scaled = base_ms.saturating_mul(1u64 << attempt.min(8));
    Duration::from_millis(scaled.min(max_ms))
}

/// Specifies how to spawn the Python gateway process. Tests use
/// this to inject `sleep`/`true` instead of a real Python child.
#[derive(Debug, Clone)]
pub struct GatewayLaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

impl GatewayLaunchSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
            cwd: None,
        }
    }
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }
    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.envs.push((k.into(), v.into()));
        self
    }
    pub fn cwd(mut self, dir: PathBuf) -> Self {
        self.cwd = Some(dir);
        self
    }

    /// Default gateway spec for the bundled harveychat Python
    /// gateway. The supervisor cd's into the python/ source dir and
    /// invokes `gateway.py` directly.
    ///
    /// `effective_llm` lets the caller propagate the slot's resolved
    /// LLM config (per-call > slot.toml > system defaults). `None`
    /// means use the gateway's `MAKAKOO_LLM_*` defaults from
    /// `LlmConfig.from_env({})` (system fallback).
    pub fn harveychat_default(
        makakoo_home: &Path,
        slot_id: &str,
        effective_llm: Option<&crate::agents::llm_override::EffectiveLlm>,
    ) -> Self {
        let home_str = makakoo_home.to_string_lossy().into_owned();
        let python_dir = makakoo_home
            .join("plugins-core/agent-harveychat/python");
        let mut spec = Self::new("python3")
            .arg("gateway.py")
            .arg("--slot")
            .arg(slot_id)
            .env("MAKAKOO_AGENT_SLOT", slot_id)
            .env("MAKAKOO_HOME", &home_str)
            .cwd(python_dir);
        if let Some(eff) = effective_llm {
            for (k, v) in crate::agents::llm_override::effective_to_env(eff) {
                spec = spec.env(k, v);
            }
        }
        spec
    }
}

/// Live in-process state of a slot supervisor. Cheap to clone via
/// `Arc<Mutex<...>>`; the status-writer task and the gateway-watch
/// task both hold their own Arcs.
#[derive(Debug)]
pub struct SupervisorInner {
    pub slot_id: String,
    pub state: SupervisorState,
    pub gateway_pid: Option<u32>,
    pub gateway_started_at: Option<DateTime<Utc>>,
    pub last_frame_at: Option<DateTime<Utc>>,
    pub restart: RestartBudget,
    pub transports: Vec<TransportStatusHandle>,
}

impl SupervisorInner {
    pub fn new(slot_id: impl Into<String>) -> Self {
        Self {
            slot_id: slot_id.into(),
            state: SupervisorState::Starting,
            gateway_pid: None,
            gateway_started_at: None,
            last_frame_at: None,
            restart: RestartBudget::new(),
            transports: Vec::new(),
        }
    }

    pub fn snapshot(&self, supervisor_pid: u32) -> SupervisorStatusFile {
        let gateway = GatewayStatus {
            alive: matches!(self.state, SupervisorState::Running),
            pid: self.gateway_pid,
            last_frame_at: self.last_frame_at,
        };
        let transports: Vec<_> =
            self.transports.iter().map(|h| h.snapshot()).collect();
        let circuit_break_until = self.restart.tripped_at().map(|t| {
            t + chrono::Duration::from_std(RESTART_BUDGET_WINDOW).unwrap()
        });
        SupervisorStatusFile {
            slot_id: self.slot_id.clone(),
            state: self.state,
            supervisor_pid,
            gateway,
            transports,
            restart_count: self.restart.count() as u32,
            circuit_break_until,
            written_at: Utc::now(),
        }
    }
}

/// Convenience cloneable handle the supervisor passes to background
/// tasks (status writer, gateway watcher).
pub type SupervisorHandle = Arc<Mutex<SupervisorInner>>;

pub fn handle(slot_id: impl Into<String>) -> SupervisorHandle {
    Arc::new(Mutex::new(SupervisorInner::new(slot_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn now_at(secs_from_epoch: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(secs_from_epoch, 0).unwrap()
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_for_attempt(0), BACKOFF_MIN);
        assert_eq!(backoff_for_attempt(1), Duration::from_millis(1000));
        assert_eq!(backoff_for_attempt(2), Duration::from_millis(2000));
        // Within the cap.
        assert!(backoff_for_attempt(5) <= BACKOFF_MAX);
        // Beyond saturates to BACKOFF_MAX.
        assert_eq!(backoff_for_attempt(20), BACKOFF_MAX);
    }

    #[test]
    fn restart_budget_within_limit_returns_backoff() {
        let mut b = RestartBudget::new();
        let t0 = now_at(1000);
        for i in 0..RESTART_BUDGET_LIMIT {
            let r = b.record_crash(now_at(1000 + i as i64));
            assert!(matches!(r, RestartDecision::Backoff(_)), "crash {i} should backoff");
        }
        assert!(!b.is_tripped());
        // Touch t0 so clippy doesn't complain about the unused
        // helper variable; semantically t0 is the start of the
        // window we just exhausted.
        assert!(b.crashes.front().copied().unwrap() >= t0);
    }

    #[test]
    fn restart_budget_overflow_trips_circuit_breaker() {
        let mut b = RestartBudget::new();
        for i in 0..=RESTART_BUDGET_LIMIT {
            b.record_crash(now_at(1000 + i as i64));
        }
        assert!(b.is_tripped());
        assert!(b.tripped_at().is_some());
    }

    #[test]
    fn restart_budget_window_evicts_old_crashes() {
        let mut b = RestartBudget::new();
        // 5 crashes within the window — uses the entire budget but
        // does not trip.
        for i in 0..5 {
            b.record_crash(now_at(1000 + i));
        }
        assert!(!b.is_tripped());
        assert_eq!(b.count(), 5);
        // Wait past the window (60s + slack). The next crash sits
        // alone in the window since the prior 5 are evicted.
        let later = now_at(1000 + 5 + (RESTART_BUDGET_WINDOW.as_secs() as i64) + 1);
        let r = b.record_crash(later);
        assert!(matches!(r, RestartDecision::Backoff(_)));
        assert_eq!(b.count(), 1);
    }

    #[test]
    fn restart_budget_reset_clears_state() {
        let mut b = RestartBudget::new();
        for i in 0..=RESTART_BUDGET_LIMIT {
            b.record_crash(now_at(1000 + i as i64));
        }
        assert!(b.is_tripped());
        b.reset();
        assert!(!b.is_tripped());
        assert_eq!(b.count(), 0);
    }

    #[test]
    fn status_file_round_trip_through_atomic_write() {
        let tmp = TempDir::new().unwrap();
        let snap = SupervisorStatusFile {
            slot_id: "secretary".into(),
            state: SupervisorState::Running,
            supervisor_pid: 42,
            gateway: GatewayStatus {
                alive: true,
                pid: Some(123),
                last_frame_at: Some(Utc::now()),
            },
            transports: Vec::new(),
            restart_count: 0,
            circuit_break_until: None,
            written_at: Utc::now(),
        };
        snap.write_atomic(tmp.path()).unwrap();
        let back = SupervisorStatusFile::read(tmp.path()).unwrap().unwrap();
        assert_eq!(back.slot_id, "secretary");
        assert_eq!(back.state, SupervisorState::Running);
        assert_eq!(back.gateway.pid, Some(123));
    }

    #[test]
    fn status_file_read_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let back = SupervisorStatusFile::read(tmp.path()).unwrap();
        assert!(back.is_none());
    }

    #[test]
    fn run_dir_path_is_under_makakoo_home() {
        let home = PathBuf::from("/Users/sebastian/MAKAKOO");
        let d = run_dir(&home, "secretary");
        assert!(d.ends_with("run/agents/secretary"));
        assert!(d.starts_with(&home));
    }

    #[test]
    fn supervisor_inner_initial_state_is_starting() {
        let inner = SupervisorInner::new("secretary");
        assert_eq!(inner.state, SupervisorState::Starting);
        assert!(inner.gateway_pid.is_none());
        assert!(inner.transports.is_empty());
    }

    #[test]
    fn supervisor_inner_snapshot_reflects_state() {
        let mut inner = SupervisorInner::new("secretary");
        inner.state = SupervisorState::Running;
        inner.gateway_pid = Some(999);
        inner.last_frame_at = Some(Utc::now());
        let snap = inner.snapshot(42);
        assert_eq!(snap.slot_id, "secretary");
        assert_eq!(snap.state, SupervisorState::Running);
        assert_eq!(snap.supervisor_pid, 42);
        assert_eq!(snap.gateway.pid, Some(999));
        assert!(snap.gateway.alive);
    }

    #[test]
    fn supervisor_inner_snapshot_marks_gateway_dead_when_not_running() {
        let mut inner = SupervisorInner::new("secretary");
        inner.state = SupervisorState::Restarting;
        inner.gateway_pid = None;
        let snap = inner.snapshot(42);
        assert!(!snap.gateway.alive);
    }

    #[test]
    fn gateway_launch_spec_default_runs_gateway_py_from_python_dir() {
        let home = PathBuf::from("/Users/sebastian/MAKAKOO");
        let spec = GatewayLaunchSpec::harveychat_default(&home, "secretary", None);
        assert_eq!(spec.program, "python3");
        assert_eq!(spec.args, vec!["gateway.py", "--slot", "secretary"]);
        assert!(spec
            .envs
            .iter()
            .any(|(k, v)| k == "MAKAKOO_AGENT_SLOT" && v == "secretary"));
        assert!(spec.envs.iter().any(|(k, _)| k == "MAKAKOO_HOME"));
        // cwd MUST be the python/ source dir so flat sibling imports
        // (`from bridge import ...`) resolve at runtime.
        let expected_cwd = home.join("plugins-core/agent-harveychat/python");
        assert_eq!(spec.cwd.as_deref(), Some(expected_cwd.as_path()));
    }

    #[test]
    fn gateway_launch_spec_propagates_llm_override_env() {
        use crate::agents::llm_override::{
            resolve_effective, LlmDefaults, LlmOverride, ReasoningEffort,
        };
        let home = PathBuf::from("/Users/sebastian/MAKAKOO");
        let over = LlmOverride {
            model: Some("claude-opus-4-7".into()),
            max_tokens: Some(8192),
            temperature: Some(0.3),
            reasoning_effort: Some(ReasoningEffort::High),
        };
        let eff = resolve_effective(Some(&over), &LlmDefaults::builtin_fallback());
        let spec = GatewayLaunchSpec::harveychat_default(&home, "secretary", Some(&eff));
        let env: std::collections::HashMap<_, _> = spec.envs.iter().cloned().collect();
        assert_eq!(env.get("MAKAKOO_LLM_MODEL").unwrap(), "claude-opus-4-7");
        assert_eq!(env.get("MAKAKOO_LLM_MAX_TOKENS").unwrap(), "8192");
        assert_eq!(env.get("MAKAKOO_LLM_REASONING_EFFORT").unwrap(), "high");
    }
}
