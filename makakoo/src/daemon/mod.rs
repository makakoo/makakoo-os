//! Daemon subsystem — install, uninstall, status, logs, and the actual
//! `daemon run` loop that platform auto-start hooks invoke.
//!
//! Each platform has its own install/uninstall writer:
//!   - macOS: `launchd` plist under `~/Library/LaunchAgents/`
//!   - Linux: systemd user service under `~/.config/systemd/user/`
//!   - Windows: `HKCU\...\Run` via the `auto-launch` crate
//!
//! The `run` subcommand is what the OS auto-start hook actually invokes —
//! it boots the SANCHO engine, the Olibia subagent, and any other
//! long-running components, then blocks on Ctrl-C.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Subcommand;
use tracing::{error, info};

pub mod install;
pub mod status;
pub mod uninstall;
pub mod watchdog;

// Per-OS daemon modules retired in Phase B — all logic now lives behind
// the `makakoo_platform::PlatformAdapter` trait. See
// `../../makakoo-platform/src/{macos,linux,windows,redox}.rs` for the
// new implementations. This module now only hosts the dispatch shim,
// the CLI subcommand enum, and the daemon main loop.

/// `makakoo daemon <subcommand>`.
#[derive(Debug, Subcommand)]
pub enum DaemonCmd {
    /// Install makakoo as an auto-starting background service for the
    /// current user.
    Install,
    /// Uninstall the auto-start service.
    Uninstall,
    /// Show daemon status (running / installed / not installed).
    Status,
    /// Tail the daemon log file.
    Logs {
        /// How many lines to tail from the end of the log.
        #[arg(short, long, default_value_t = 50)]
        lines: usize,
    },
    /// Run the daemon in the foreground. This is what the OS auto-start
    /// hook actually invokes — not meant for interactive use, though it
    /// works fine for debugging via Ctrl-C.
    Run,
}

pub async fn dispatch(cmd: DaemonCmd) -> Result<()> {
    match cmd {
        DaemonCmd::Install => install::run().await,
        DaemonCmd::Uninstall => uninstall::run().await,
        DaemonCmd::Status => status::run().await,
        DaemonCmd::Logs { lines } => status::tail_logs(lines).await,
        DaemonCmd::Run => run_forever().await,
    }
}

/// The actual daemon main loop. Spawns SANCHO as a background tokio
/// task, runs the agent supervisor on a polling tick, and blocks on
/// Ctrl-C.
pub async fn run_forever() -> Result<()> {
    use makakoo_core::agents::AgentSupervisor;
    use makakoo_core::plugin::PluginRegistry;
    use makakoo_core::sancho::{default_registry, SanchoContext, SanchoEngine};

    let ctx = crate::context::CliContext::new()?;
    let home = ctx.home().clone();
    let log_dir = status::log_dir();
    std::fs::create_dir_all(&log_dir).ok();
    info!(
        makakoo_home = %home.display(),
        log_dir = %log_dir.display(),
        "makakoo daemon starting"
    );

    // Build the SANCHO engine with the same setup as `makakoo sancho tick`.
    let store = ctx.store()?;

    // Post-crash integrity probe. `PRAGMA integrity_check` is O(DB) but
    // runs once at boot and is dominated by the first WAL checkpoint
    // anyway. We surface issues via a structured log — NOT auto-recovery
    // — so the operator can decide whether to .recover or restore from
    // backup. The 2026-04-22 post-reboot corruption would have surfaced
    // here 90 seconds after boot instead of silently returning Error 11
    // to every read path until a human noticed.
    {
        let arc = store.conn_arc();
        let conn = arc.lock().expect("integrity probe conn poisoned");
        match conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)) {
            Ok(status) if status == "ok" => {
                info!("superbrain.db integrity check: ok");
            }
            Ok(status) => {
                error!(
                    first_issue = %status,
                    "superbrain.db integrity check FAILED — database corruption detected. \
                     Sync and chat may hit Error 11/1555. Recovery: sqlite3 superbrain.db '.recover' > recovered.sql; \
                     mv superbrain.db superbrain.db.corrupt; sqlite3 superbrain.db < recovered.sql"
                );
            }
            Err(e) => {
                error!(error = %e, "superbrain.db integrity check could not run");
            }
        }
    }
    let bus = ctx.event_bus()?;
    let llm = ctx.llm();
    let emb = ctx.embeddings();
    let sancho_ctx = Arc::new(SanchoContext::new(store, bus, llm, emb, home.clone()));
    let plugins = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let registry = default_registry(Arc::clone(&sancho_ctx), &plugins);
    let engine = SanchoEngine::new(registry, sancho_ctx, Duration::from_secs(60));

    info!(tasks = engine.task_count(), "sancho engine started");

    // Spawn SANCHO in the background; shut it down on Ctrl-C.
    let shutdown = engine.shutdown_handle();
    let sancho_handle = tokio::spawn(async move {
        if let Err(e) = engine.run_forever().await {
            error!(error = %e, "sancho engine crashed");
        }
    });

    // Agent supervisor — currently has no auto-discovered agent plugins
    // (Phase 5 vendoring lands those). Polls health every 30s; restarts
    // crashed children with exponential backoff. The empty-supervisor
    // case is a no-op so this is safe to wire even when no agents exist.
    let supervisor = Arc::new(AgentSupervisor::new());
    let sup_for_loop = Arc::clone(&supervisor);
    let sup_token = tokio_util_cancel::CancelOnce::new();
    let sup_token_for_loop = sup_token.clone();
    let supervisor_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let restarted = sup_for_loop.check_and_restart();
                    if restarted > 0 {
                        info!(restarted, "agent supervisor restarted dead children");
                    }
                }
                _ = sup_token_for_loop.wait() => break,
            }
        }
    });
    info!(
        agents = supervisor.len(),
        "agent supervisor started (poll every 30s)"
    );

    // Phase 1 SPRINT-HARVEY-BRAIN-ORCHESTRATION: daemon self-watchdog
    // writes a heartbeat JSONL line every 5 min so `harvey memory health`
    // and Pixel mascot can prove the daemon is alive without parsing logs.
    let watchdog_shutdown = Arc::new(tokio::sync::Notify::new());
    let watchdog_handle = watchdog::spawn(
        home.clone(),
        watchdog::DEFAULT_WATCHDOG_INTERVAL,
        Arc::clone(&watchdog_shutdown),
    );
    info!(
        interval_sec = watchdog::DEFAULT_WATCHDOG_INTERVAL.as_secs(),
        "daemon self-watchdog started"
    );

    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received — stopping sancho + agent supervisor + watchdog");
    shutdown.notify_waiters();
    sup_token.cancel();
    watchdog_shutdown.notify_waiters();
    supervisor.shutdown(Duration::from_millis(500));
    let _ = sancho_handle.await;
    let _ = supervisor_handle.await;
    let _ = watchdog_handle.await;
    info!("daemon exiting");
    Ok(())
}

/// Tiny cancellation helper to avoid pulling in `tokio_util` as a
/// dep. One-shot signal driven by an atomic flag + a Notify so the
/// supervisor loop can wake on shutdown rather than waiting for the
/// next 30s tick.
mod tokio_util_cancel {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::Notify;

    #[derive(Clone)]
    pub struct CancelOnce {
        flag: Arc<AtomicBool>,
        notify: Arc<Notify>,
    }
    impl CancelOnce {
        pub fn new() -> Self {
            Self {
                flag: Arc::new(AtomicBool::new(false)),
                notify: Arc::new(Notify::new()),
            }
        }
        pub fn cancel(&self) {
            if !self.flag.swap(true, Ordering::SeqCst) {
                self.notify.notify_waiters();
            }
        }
        pub async fn wait(&self) {
            if self.flag.load(Ordering::SeqCst) {
                return;
            }
            self.notify.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_subcommand_does_not_panic() {
        // Smoke test — status should always return Ok on a fresh machine.
        let r = status::run().await;
        assert!(r.is_ok());
    }

    #[test]
    fn daemon_state_initial_is_reasonable() {
        // Just make sure current_state() resolves without panicking.
        let _ = status::current_state();
    }
}
