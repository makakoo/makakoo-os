//! Agent process supervision — spawn, health, signal, restart.
//!
//! Rust owns process supervision; the LLM loop itself stays Python (it
//! lives in the `lib-agent-loop` plugin). This module gives the daemon
//! the primitives it needs to keep agent plugins alive across crashes:
//!
//!   * `AgentLaunchSpec` — declarative launch config (program, args,
//!     envs, working dir).
//!   * `AgentProcess` — handle around a spawned child with restart
//!     bookkeeping (count, last restart, current PID).
//!   * `AgentSupervisor` — owns a `Vec<AgentProcess>`, polls health on
//!     a fixed interval, and restarts crashed children with exponential
//!     backoff (1 → 2 → 4 → … → 60 s, capped).
//!
//! Health-check is deliberately minimal at this stage: a kill(pid, 0)
//! liveness probe. Plugins that want richer probes wire them through
//! the capability socket they already own.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::{MakakooError, Result};

/// Declarative launch config for a supervised process.
#[derive(Debug, Clone)]
pub struct AgentLaunchSpec {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub envs: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
}

impl AgentLaunchSpec {
    pub fn new(name: impl Into<String>, program: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args: Vec::new(),
            envs: HashMap::new(),
            cwd: None,
        }
    }
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }
    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.envs.insert(k.into(), v.into());
        self
    }
    pub fn cwd(mut self, dir: PathBuf) -> Self {
        self.cwd = Some(dir);
        self
    }
}

/// Health snapshot of an `AgentProcess`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Process responded to liveness probe.
    Alive,
    /// Spawned but no longer reachable (likely crashed).
    Dead,
    /// Never started (initial state) or stopped intentionally.
    Stopped,
}

/// One supervised agent. Cheap to clone via `Arc<Mutex<...>>` — the
/// supervisor holds an `Arc` to mutate restart state from the polling
/// task while still surfacing read-only status to the daemon.
pub struct AgentProcess {
    spec: AgentLaunchSpec,
    child: Option<Child>,
    pid: Option<u32>,
    restart_count: u32,
    last_restart: Option<Instant>,
    stopped: bool,
}

impl AgentProcess {
    pub fn new(spec: AgentLaunchSpec) -> Self {
        Self {
            spec,
            child: None,
            pid: None,
            restart_count: 0,
            last_restart: None,
            stopped: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.spec.name
    }
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
    pub fn restart_count(&self) -> u32 {
        self.restart_count
    }

    /// Spawn (or re-spawn) the child process. Returns a `MakakooError`
    /// if the process can't be launched.
    pub fn spawn(&mut self) -> Result<()> {
        if self.child.is_some() {
            return Ok(());
        }
        let mut cmd = Command::new(&self.spec.program);
        cmd.args(&self.spec.args);
        for (k, v) in &self.spec.envs {
            cmd.env(k, v);
        }
        if let Some(dir) = &self.spec.cwd {
            cmd.current_dir(dir);
        }
        let child = cmd.spawn().map_err(|e| {
            MakakooError::Internal(format!(
                "spawn '{}' ({}): {e}",
                self.spec.name, self.spec.program
            ))
        })?;
        self.pid = Some(child.id());
        self.child = Some(child);
        self.stopped = false;
        Ok(())
    }

    /// Liveness probe — kill(pid, 0). Returns `HealthStatus::Alive`
    /// when the kernel still knows about the PID, `Dead` once the
    /// child has exited (`wait()` cleared by polling).
    pub fn health(&mut self) -> HealthStatus {
        if self.stopped {
            return HealthStatus::Stopped;
        }
        let child = match self.child.as_mut() {
            Some(c) => c,
            None => return HealthStatus::Stopped,
        };
        match child.try_wait() {
            Ok(Some(_status)) => {
                self.child = None;
                self.pid = None;
                HealthStatus::Dead
            }
            Ok(None) => HealthStatus::Alive,
            Err(_) => HealthStatus::Dead,
        }
    }

    /// Send SIGTERM, wait `grace` for the child to exit, then SIGKILL
    /// if it's still alive. Marks the process as intentionally stopped
    /// so the supervisor won't auto-restart it.
    pub fn stop(&mut self, grace: Duration) -> Result<()> {
        self.stopped = true;
        let pid = match self.pid {
            Some(p) => p,
            None => return Ok(()),
        };
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        let deadline = Instant::now() + grace;
        loop {
            if let Some(child) = self.child.as_mut() {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    self.child = None;
                    self.pid = None;
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // Grace expired — SIGKILL.
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait();
        }
        self.child = None;
        self.pid = None;
        Ok(())
    }

    /// Stop and respawn with exponential backoff (capped at 60 s).
    pub fn restart(&mut self) -> Result<()> {
        let backoff = backoff_for_attempt(self.restart_count);
        if let Some(prev) = self.last_restart {
            let since = prev.elapsed();
            if since < backoff {
                std::thread::sleep(backoff - since);
            }
        }
        self.stop(Duration::from_millis(500))?;
        self.stopped = false;
        self.spawn()?;
        self.restart_count += 1;
        self.last_restart = Some(Instant::now());
        Ok(())
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        // Best-effort cleanup so test tempdirs don't leak children.
        let _ = self.stop(Duration::from_millis(100));
    }
}

/// Exponential backoff: 1, 2, 4, 8, ..., capped at 60 s.
fn backoff_for_attempt(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.min(6); // 1..=64; cap below at 60
    Duration::from_secs(secs.min(60))
}

/// Owns and supervises a set of `AgentProcess` handles.
///
/// Construction is pure data; spawn/health/restart calls do I/O.
/// Wiring into the daemon's `tokio::spawn` happens at the call site
/// (we don't pull in tokio here to keep the module unit-testable
/// without async runtime setup).
pub struct AgentSupervisor {
    agents: Mutex<Vec<Arc<Mutex<AgentProcess>>>>,
}

impl Default for AgentSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentSupervisor {
    pub fn new() -> Self {
        Self {
            agents: Mutex::new(Vec::new()),
        }
    }

    /// Add (and spawn) a new agent. Returns the supervised handle.
    pub fn add(&self, spec: AgentLaunchSpec) -> Result<Arc<Mutex<AgentProcess>>> {
        let mut proc = AgentProcess::new(spec);
        proc.spawn()?;
        let arc = Arc::new(Mutex::new(proc));
        self.agents.lock().unwrap().push(Arc::clone(&arc));
        Ok(arc)
    }

    /// Number of supervised agents.
    pub fn len(&self) -> usize {
        self.agents.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Poll each agent's health. Restart any that died and weren't
    /// stopped intentionally. Returns the count restarted.
    pub fn check_and_restart(&self) -> usize {
        let snapshot: Vec<Arc<Mutex<AgentProcess>>> =
            self.agents.lock().unwrap().iter().cloned().collect();
        let mut restarted = 0usize;
        for handle in snapshot {
            let mut p = handle.lock().unwrap();
            match p.health() {
                HealthStatus::Dead => {
                    if p.restart().is_ok() {
                        restarted += 1;
                    }
                }
                HealthStatus::Alive | HealthStatus::Stopped => {}
            }
        }
        restarted
    }

    /// Stop every supervised agent. Used on daemon shutdown.
    pub fn shutdown(&self, grace: Duration) {
        let snapshot: Vec<Arc<Mutex<AgentProcess>>> =
            self.agents.lock().unwrap().iter().cloned().collect();
        for handle in snapshot {
            let _ = handle.lock().unwrap().stop(grace);
        }
        self.agents.lock().unwrap().clear();
    }

    /// Snapshot of `(name, pid, restart_count)` for status reporting.
    pub fn snapshot(&self) -> Vec<(String, Option<u32>, u32)> {
        self.agents
            .lock()
            .unwrap()
            .iter()
            .map(|h| {
                let p = h.lock().unwrap();
                (p.name().to_string(), p.pid(), p.restart_count())
            })
            .collect()
    }
}

// Agent-lifecycle tests shell out to the Unix `true` + `sleep`
// binaries (they're in every POSIX path); Windows would need a
// `cmd.exe /C ...` rewrite of every sleep_spec / "true" literal.
// Gate the whole module to Unix for v0.1; Windows siblings land
// alongside any Windows-specific agent-spawn story.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sleep_spec(name: &str, secs: u32) -> AgentLaunchSpec {
        AgentLaunchSpec::new(name, "sleep").arg(format!("{secs}"))
    }

    #[test]
    fn backoff_doubles_then_caps_at_sixty_seconds() {
        assert_eq!(backoff_for_attempt(0), Duration::from_secs(1));
        assert_eq!(backoff_for_attempt(1), Duration::from_secs(2));
        assert_eq!(backoff_for_attempt(2), Duration::from_secs(4));
        assert_eq!(backoff_for_attempt(6), Duration::from_secs(60));
        assert_eq!(backoff_for_attempt(20), Duration::from_secs(60));
    }

    #[test]
    fn agent_process_spawns_and_reports_alive() {
        let mut p = AgentProcess::new(sleep_spec("sleeper", 5));
        p.spawn().unwrap();
        assert!(p.pid().is_some());
        assert_eq!(p.health(), HealthStatus::Alive);
        p.stop(Duration::from_millis(500)).unwrap();
    }

    #[test]
    fn agent_process_health_flips_to_dead_after_natural_exit() {
        let mut p = AgentProcess::new(AgentLaunchSpec::new("quick", "true"));
        p.spawn().unwrap();
        // Wait for `true` to exit (it returns immediately).
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(p.health(), HealthStatus::Dead);
    }

    #[test]
    fn stop_terminates_and_releases_pid() {
        let mut p = AgentProcess::new(sleep_spec("tostop", 60));
        p.spawn().unwrap();
        let pid = p.pid().expect("must have pid");
        assert!(pid > 0);
        p.stop(Duration::from_millis(500)).unwrap();
        assert!(p.pid().is_none());
        assert_eq!(p.health(), HealthStatus::Stopped);
    }

    #[test]
    fn supervisor_restarts_dead_child() {
        let sup = AgentSupervisor::new();
        // Spawn a `true` — exits immediately.
        let _ = sup.add(AgentLaunchSpec::new("die", "true")).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let restarted = sup.check_and_restart();
        // restart() runs sleep(backoff_for_attempt(0)) = 1s before respawn.
        assert_eq!(restarted, 1);
        let snap = sup.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "die");
        assert!(snap[0].2 >= 1, "restart_count should advance");
        sup.shutdown(Duration::from_millis(200));
    }

    #[test]
    fn supervisor_does_not_restart_intentional_stop() {
        let sup = AgentSupervisor::new();
        let h = sup.add(sleep_spec("stayed", 60)).unwrap();
        h.lock().unwrap().stop(Duration::from_millis(500)).unwrap();
        let restarted = sup.check_and_restart();
        assert_eq!(restarted, 0);
        sup.shutdown(Duration::from_millis(200));
    }

    #[test]
    fn spawn_failure_returns_error() {
        let mut p = AgentProcess::new(AgentLaunchSpec::new(
            "nope",
            "/this/does/not/exist/binary_xyz",
        ));
        assert!(p.spawn().is_err());
    }

    #[test]
    #[cfg(unix)]
    fn supervisor_restarts_after_external_sigkill() {
        let sup = AgentSupervisor::new();
        let h = sup.add(sleep_spec("killed", 60)).unwrap();
        let pid = h.lock().unwrap().pid().expect("must have pid");
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        // Wait for the kernel to reap.
        std::thread::sleep(Duration::from_millis(200));
        let restarted = sup.check_and_restart();
        assert_eq!(restarted, 1);
        let snap = sup.snapshot();
        assert_eq!(snap.len(), 1);
        let new_pid = snap[0].1.expect("restarted agent must have new pid");
        assert_ne!(new_pid, pid, "restart should produce a fresh pid");
        assert_eq!(snap[0].2, 1);
        sup.shutdown(Duration::from_millis(200));
    }

    #[test]
    fn supervisor_shutdown_clears_handles() {
        let sup = AgentSupervisor::new();
        sup.add(sleep_spec("a", 60)).unwrap();
        sup.add(sleep_spec("b", 60)).unwrap();
        assert_eq!(sup.len(), 2);
        sup.shutdown(Duration::from_millis(200));
        assert!(sup.is_empty());
    }
}
