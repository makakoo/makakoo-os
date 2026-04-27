//! Async supervisor runtime — spawns gateway, watches for crashes,
//! applies restart budget, writes status.json, handles SIGTERM.
//!
//! Phase 1 deliberately keeps the gateway as ANY child process
//! described by `GatewayLaunchSpec` so tests can inject `sleep` /
//! `true` instead of a real Python child. Phase 3 wires in the
//! actual `plugins-core/agent-harveychat/python/gateway.py`.
//!
//! Lifecycle:
//!
//!   start_supervisor(spec, handle, run_dir)
//!     ├─ spawn gateway child
//!     ├─ status writer task (5s interval)
//!     ├─ SIGTERM listener task
//!     └─ child watcher loop
//!         on exit:
//!           if RestartBudget says Backoff(d) → sleep d → respawn
//!           if RestartBudget says CircuitBreak → state=Crashed → exit
//!         on SIGTERM:
//!           SIGTERM child, wait SHUTDOWN_GRACE, SIGKILL if alive,
//!           write final status with state=Stopped, exit
//!
//! Shutdown signal is `tokio::sync::watch::channel<bool>` so a
//! `send(true)` is observable forever after — including by
//! subscribers created post-signal. This avoids the
//! `Notify::notify_waiters` race where a future created after the
//! notification never fires.

use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use tokio::process::{Child, Command};
use tokio::sync::watch;

use crate::agents::supervisor::{
    GatewayLaunchSpec, RestartDecision, SupervisorHandle, SupervisorState, SHUTDOWN_GRACE,
    STATUS_WRITE_INTERVAL,
};
use crate::error::{MakakooError, Result};

/// Durable shutdown subscriber. `wait()` returns immediately if the
/// trigger has already fired, and otherwise blocks until it does.
#[derive(Debug, Clone)]
pub struct ShutdownSignal {
    rx: watch::Receiver<bool>,
}

impl ShutdownSignal {
    pub async fn wait(&mut self) {
        let _ = self.rx.wait_for(|v| *v).await;
    }

    pub fn is_fired(&self) -> bool {
        *self.rx.borrow()
    }
}

#[derive(Debug)]
pub struct ShutdownTrigger {
    tx: watch::Sender<bool>,
}

impl ShutdownTrigger {
    pub fn fire(&self) {
        let _ = self.tx.send(true);
    }
}

pub fn shutdown_pair() -> (ShutdownTrigger, ShutdownSignal) {
    let (tx, rx) = watch::channel(false);
    (ShutdownTrigger { tx }, ShutdownSignal { rx })
}

/// Spawn the gateway child described by `spec`. Returns the live
/// `tokio::process::Child` so the caller can `wait()` or signal it.
pub fn spawn_gateway(spec: &GatewayLaunchSpec) -> Result<Child> {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    for (k, v) in &spec.envs {
        cmd.env(k, v);
    }
    if let Some(dir) = &spec.cwd {
        cmd.current_dir(dir);
    }
    cmd.kill_on_drop(true);
    cmd.spawn().map_err(|e| {
        MakakooError::Internal(format!("spawn gateway '{}': {e}", spec.program))
    })
}

/// Periodic task that snapshots the supervisor state to status.json
/// every `STATUS_WRITE_INTERVAL`. Exits when shutdown signals.
pub async fn run_status_writer(
    handle: SupervisorHandle,
    run_dir: PathBuf,
    supervisor_pid: u32,
    mut shutdown: ShutdownSignal,
) {
    let mut interval = tokio::time::interval(STATUS_WRITE_INTERVAL);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let snap = handle.lock().unwrap().snapshot(supervisor_pid);
                let _ = snap.write_atomic(&run_dir);
            }
            _ = shutdown.wait() => {
                let mut inner = handle.lock().unwrap();
                inner.state = SupervisorState::Stopped;
                let snap = inner.snapshot(supervisor_pid);
                drop(inner);
                let _ = snap.write_atomic(&run_dir);
                return;
            }
        }
    }
}

/// Watch the gateway child. On exit, consult the restart budget and
/// either respawn (after backoff) or trip the circuit breaker.
/// Returns when:
///   - shutdown signal fires (graceful stop)
///   - circuit breaker trips (state stays Crashed)
///
/// IMPORTANT: callers must wire the returned `Result` to fire
/// shutdown so any sibling tasks (e.g., status writer) exit too.
/// The top-level `run_supervisor()` does this.
pub async fn run_gateway_watcher(
    spec: GatewayLaunchSpec,
    handle: SupervisorHandle,
    mut shutdown: ShutdownSignal,
) -> Result<()> {
    // Initial spawn.
    let mut child = spawn_gateway(&spec)?;
    {
        let mut inner = handle.lock().unwrap();
        inner.gateway_pid = child.id();
        inner.gateway_started_at = Some(Utc::now());
        inner.state = SupervisorState::Running;
    }

    loop {
        tokio::select! {
            wait = child.wait() => {
                let _ = wait;
                let decision = {
                    let mut inner = handle.lock().unwrap();
                    inner.gateway_pid = None;
                    let d = inner.restart.record_crash(Utc::now());
                    inner.state = match d {
                        RestartDecision::Backoff(_) => SupervisorState::Restarting,
                        RestartDecision::CircuitBreak => SupervisorState::Crashed,
                    };
                    d
                };
                match decision {
                    RestartDecision::Backoff(delay) => {
                        let mut sd = shutdown.clone();
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {}
                            _ = sd.wait() => return Ok(()),
                        }
                        match spawn_gateway(&spec) {
                            Ok(new_child) => {
                                let mut inner = handle.lock().unwrap();
                                inner.gateway_pid = new_child.id();
                                inner.gateway_started_at = Some(Utc::now());
                                inner.state = SupervisorState::Running;
                                drop(inner);
                                child = new_child;
                            }
                            Err(_) => {
                                let tripped = {
                                    let mut inner = handle.lock().unwrap();
                                    let d = inner.restart.record_crash(Utc::now());
                                    if matches!(d, RestartDecision::CircuitBreak) {
                                        inner.state = SupervisorState::Crashed;
                                        true
                                    } else {
                                        false
                                    }
                                };
                                if tripped {
                                    return Ok(());
                                }
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        }
                    }
                    RestartDecision::CircuitBreak => {
                        // Circuit broken — stay alive so status.json keeps
                        // reporting Crashed, until shutdown.
                        shutdown.wait().await;
                        return Ok(());
                    }
                }
            }
            _ = shutdown.wait() => {
                let pid = child.id();
                if let Some(pid) = pid {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }
                let exited = tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await;
                if exited.is_err() {
                    if let Some(pid) = pid {
                        #[cfg(unix)]
                        unsafe {
                            libc::kill(pid as i32, libc::SIGKILL);
                        }
                    }
                    let _ = child.wait().await;
                }
                let mut inner = handle.lock().unwrap();
                inner.gateway_pid = None;
                inner.state = SupervisorState::Stopped;
                return Ok(());
            }
        }
    }
}

/// SIGTERM listener — fires the shutdown trigger when the supervisor
/// receives SIGTERM (or SIGINT for Ctrl-C). Once fired, every other
/// task observing `ShutdownSignal::wait` will exit.
#[cfg(unix)]
pub async fn run_signal_listener(trigger: ShutdownTrigger) -> std::io::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    trigger.fire();
    Ok(())
}

#[cfg(not(unix))]
pub async fn run_signal_listener(trigger: ShutdownTrigger) -> std::io::Result<()> {
    tokio::signal::ctrl_c().await?;
    trigger.fire();
    Ok(())
}

/// Top-level entry point invoked by the `agent _supervisor` subcommand.
///
/// Critical: ensures the status writer is unblocked even if the watcher
/// returns Err on initial spawn. Without this, a missing gateway binary
/// (e.g., gateway.py absent) would wedge the supervisor forever waiting
/// for a status flush that never gets a shutdown signal.
pub async fn run_supervisor(
    spec: GatewayLaunchSpec,
    handle: SupervisorHandle,
    run_dir: PathBuf,
) -> Result<()> {
    let supervisor_pid = std::process::id();
    let (trigger, signal) = shutdown_pair();

    // Status writer in background — its own ShutdownSignal clone.
    let writer_handle = tokio::spawn(run_status_writer(
        handle.clone(),
        run_dir.clone(),
        supervisor_pid,
        signal.clone(),
    ));

    // SIGTERM listener — owns its own ShutdownTrigger.
    let signal_trigger = ShutdownTrigger {
        tx: trigger.tx.clone(),
    };
    tokio::spawn(async move {
        let _ = run_signal_listener(signal_trigger).await;
    });

    // Watcher runs to completion. Whether it succeeds or errors,
    // signal shutdown so the writer exits.
    let watcher_result = run_gateway_watcher(spec, handle, signal).await;
    trigger.fire();

    let _ = writer_handle.await;

    watcher_result
}

/// Test-only convenience for spawning gateways under controlled
/// scenarios. Each constructor returns a `GatewayLaunchSpec` whose
/// real-process behavior simulates the named scenario.
///
/// This is the "MockGateway" abstraction validators asked for —
/// named scenarios are easier to read than raw `sleep`/`true`/`false`
/// calls in tests, and keep the runtime path identical to production.
#[cfg(test)]
pub mod mock {
    use super::GatewayLaunchSpec;

    pub struct MockGateway;

    impl MockGateway {
        /// Gateway runs forever (sleep 60). Tests use this for SIGTERM
        /// + graceful-shutdown scenarios.
        pub fn sleep_forever() -> GatewayLaunchSpec {
            GatewayLaunchSpec::new("sleep").arg("60")
        }

        /// Gateway crashes immediately with exit code 1. Drives the
        /// restart budget + circuit-breaker tests.
        pub fn crash_immediately() -> GatewayLaunchSpec {
            GatewayLaunchSpec::new("false")
        }

        /// Gateway exits cleanly with code 0. Tests respawn-on-clean-exit
        /// (the watcher treats any exit as a "crash" since we don't
        /// expect the gateway to ever exit on its own).
        pub fn exit_clean() -> GatewayLaunchSpec {
            GatewayLaunchSpec::new("true")
        }

        /// Gateway binary does not exist. Tests the spawn-error path.
        pub fn spawn_failure() -> GatewayLaunchSpec {
            GatewayLaunchSpec::new("/this/does/not/exist/binary_xyz_v2_mega")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::supervisor::{handle, GatewayLaunchSpec, SupervisorState};
    use mock::MockGateway;
    use tempfile::TempDir;

    #[tokio::test]
    async fn spawn_gateway_with_real_command_returns_child() {
        let spec = MockGateway::sleep_forever();
        let mut child = spawn_gateway(&spec).expect("sleep should spawn");
        assert!(child.id().is_some());
        child.start_kill().unwrap();
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn spawn_gateway_with_missing_binary_errors() {
        let spec = MockGateway::spawn_failure();
        let err = spawn_gateway(&spec);
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn watcher_marks_running_then_stopped_on_shutdown() {
        let h = handle("secretary");
        let (trigger, signal) = shutdown_pair();
        let spec = MockGateway::sleep_forever();

        let watcher_handle = {
            let h = h.clone();
            tokio::spawn(async move { run_gateway_watcher(spec, h, signal).await })
        };

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.lock().unwrap().state, SupervisorState::Running);
        assert!(h.lock().unwrap().gateway_pid.is_some());

        trigger.fire();
        watcher_handle.await.unwrap().unwrap();

        let inner = h.lock().unwrap();
        assert_eq!(inner.state, SupervisorState::Stopped);
        assert!(inner.gateway_pid.is_none());
    }

    #[tokio::test]
    async fn watcher_respawns_on_natural_exit_within_budget() {
        let h = handle("secretary");
        let (trigger, signal) = shutdown_pair();
        let spec = MockGateway::exit_clean();

        let watcher_handle = {
            let h = h.clone();
            tokio::spawn(async move { run_gateway_watcher(spec, h, signal).await })
        };

        tokio::time::sleep(Duration::from_millis(800)).await;

        {
            let inner = h.lock().unwrap();
            assert!(inner.restart.count() >= 1, "expected at least one crash recorded");
        }

        trigger.fire();
        watcher_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn watcher_circuit_breaks_after_six_crashes() {
        let h = handle("secretary");
        let (trigger, signal) = shutdown_pair();
        let spec = MockGateway::crash_immediately();

        let watcher_handle = {
            let h = h.clone();
            tokio::spawn(async move { run_gateway_watcher(spec, h, signal).await })
        };

        // Backoff schedule: 500ms,1s,2s,4s,8s,... Tripping the breaker
        // requires 6 crashes total. After ~3.5s wall-clock, 4-5 crashes
        // recorded. Wait up to 20s for full circuit-break.
        let mut tripped = false;
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if h.lock().unwrap().restart.is_tripped() {
                tripped = true;
                break;
            }
        }
        assert!(tripped, "expected breaker to trip within 20s");
        assert_eq!(h.lock().unwrap().state, SupervisorState::Crashed);

        trigger.fire();
        watcher_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn watcher_handles_sigterm_with_grace() {
        let h = handle("secretary");
        let (trigger, signal) = shutdown_pair();
        let spec = MockGateway::sleep_forever();

        let watcher_handle = {
            let h = h.clone();
            tokio::spawn(async move { run_gateway_watcher(spec, h, signal).await })
        };

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(h.lock().unwrap().gateway_pid.is_some());

        let start = std::time::Instant::now();
        trigger.fire();
        watcher_handle.await.unwrap().unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed < SHUTDOWN_GRACE,
            "shutdown took {elapsed:?}, expected under {SHUTDOWN_GRACE:?}"
        );
        assert_eq!(h.lock().unwrap().state, SupervisorState::Stopped);
        assert!(h.lock().unwrap().gateway_pid.is_none());
    }

    #[tokio::test]
    async fn shutdown_signal_durability_post_fire_subscribers_observe() {
        // Critical regression test: a subscriber created AFTER the
        // shutdown is fired must still observe the fire on its first
        // wait() call. This is the property `Notify::notified()` lacks.
        let (trigger, signal) = shutdown_pair();
        trigger.fire();
        // New "subscriber" is just a clone of the existing signal.
        let mut late = signal.clone();
        // wait() must return immediately.
        tokio::time::timeout(Duration::from_millis(50), late.wait())
            .await
            .expect("late subscriber must observe past shutdown fire");
        assert!(signal.is_fired());
    }

    #[tokio::test]
    async fn run_supervisor_shuts_writer_when_watcher_errors() {
        // Regression: if spawn_gateway fails, watcher returns Err.
        // run_supervisor() must signal shutdown so the writer task
        // exits — otherwise the supervisor wedges forever.
        let h = handle("secretary");
        let tmp = TempDir::new().unwrap();
        let spec = MockGateway::spawn_failure();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            run_supervisor(spec, h, tmp.path().to_path_buf()),
        )
        .await
        .expect("run_supervisor must not wedge on watcher error");
        assert!(result.is_err(), "spawn failure should propagate as Err");
    }

    #[tokio::test]
    async fn status_writer_flushes_to_disk() {
        let tmp = TempDir::new().unwrap();
        let h = handle("secretary");
        h.lock().unwrap().state = SupervisorState::Running;
        h.lock().unwrap().gateway_pid = Some(123);
        let (trigger, signal) = shutdown_pair();

        let writer = {
            let h = h.clone();
            let dir = tmp.path().to_path_buf();
            tokio::spawn(async move { run_status_writer(h, dir, 42, signal).await })
        };

        tokio::time::sleep(Duration::from_millis(50)).await;
        let path = tmp.path().join("status.json");
        for _ in 0..50 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(path.exists(), "status.json should have been written");

        trigger.fire();
        writer.await.unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"state\": \"stopped\""), "got: {body}");
    }
}
