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
pub mod restart;
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
    /// Restart the daemon service so it picks up the current makakoo binary.
    Restart,
    /// Tail the daemon log file.
    Logs {
        /// How many lines to tail from the end of the log.
        #[arg(short, long, default_value_t = 50)]
        lines: usize,
    },
    /// Run the daemon in the foreground. This is what the OS auto-start
    /// hook actually invokes — not meant for interactive use, though it
    /// works fine for debugging via Ctrl-C.
    Run {
        /// Headless engine mode (also enabled via `MAKAKOO_HEADLESS=1`):
        /// runs Superbrain integrity check, folder-index directory readiness,
        /// and subprocess-free heartbeat only. Disables the SANCHO clock,
        /// plugin tasks, and agent supervisor, and refuses startup if any
        /// enabled plugin declares `exec/*`.
        #[arg(long, default_value_t = false)]
        headless: bool,
    },
}

fn is_headless_env() -> bool {
    std::env::var("MAKAKOO_HEADLESS")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub async fn dispatch(cmd: DaemonCmd) -> Result<()> {
    match cmd {
        DaemonCmd::Install => install::run().await,
        DaemonCmd::Uninstall => uninstall::run().await,
        DaemonCmd::Status => status::run().await,
        DaemonCmd::Restart => restart::run().await,
        DaemonCmd::Logs { lines } => status::tail_logs(lines).await,
        DaemonCmd::Run { headless } => {
            if headless || is_headless_env() {
                run_headless().await
            } else {
                run_forever().await
            }
        }
    }
}

/// Build the daemon's SANCHO engine and trigger the store-open Brain probe on
/// the same process-wide [`makakoo_core::superbrain::vector_model::BrainRebuildCoordinator`].
pub(crate) fn build_daemon_sancho_engine(
    ctx: &crate::context::CliContext,
) -> Result<(
    Arc<makakoo_core::superbrain::store::SuperbrainStore>,
    makakoo_core::sancho::SanchoEngine,
)> {
    use makakoo_core::plugin::PluginRegistry;
    use makakoo_core::sancho::{default_registry, SanchoEngine};

    let store = ctx.probe_brain_on_open()?;
    let sancho_ctx = ctx.build_sancho_context()?;
    let plugins = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let registry = default_registry(Arc::clone(&sancho_ctx), &plugins);
    let engine = SanchoEngine::new(registry, sancho_ctx, Duration::from_secs(60));
    Ok((store, engine))
}

/// The actual daemon main loop. Spawns SANCHO as a background tokio
/// task, runs the agent supervisor on a polling tick, and blocks on
/// Ctrl-C.
pub async fn run_forever() -> Result<()> {
    use makakoo_core::agents::AgentSupervisor;

    let ctx = crate::context::CliContext::new()?;
    let home = ctx.home().clone();
    let log_dir = status::log_dir();
    std::fs::create_dir_all(&log_dir).ok();
    info!(
        makakoo_home = %home.display(),
        log_dir = %log_dir.display(),
        "makakoo daemon starting"
    );

    // Build the SANCHO engine with the same setup as `makakoo sancho tick`
    // and wire the store-open probe onto the same process-wide coordinator.
    let (store, engine) = build_daemon_sancho_engine(&ctx)?;

    // Post-crash integrity probe. `PRAGMA quick_check(1)` verifies B-tree
    // page integrity and stops on the first defect without scanning every
    // secondary index (avoids multi-second boot stalls on large DBs). We
    // surface issues via a structured log — NOT auto-recovery — so the
    // operator can decide whether to .recover or restore from backup.
    {
        let arc = store.conn_arc();
        let conn = arc.lock().expect("integrity probe conn poisoned");
        match conn.query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0)) {
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

/// Fail closed if any enabled plugin declares an `exec/*` capability or shell
/// runtime in headless mode.
pub(crate) fn validate_headless_plugins(
    plugins: &makakoo_core::plugin::PluginRegistry,
) -> Result<()> {
    use makakoo_core::capability::verb::normalize_grant;
    use makakoo_core::plugin::manifest::PluginLanguage;

    let mut offenders = Vec::new();
    for p in plugins.plugins().iter().filter(|p| p.enabled) {
        if p.manifest.plugin.language == PluginLanguage::Shell {
            offenders.push(format!("{} (language=shell)", p.manifest.plugin.name));
            continue;
        }
        for raw in &p.manifest.capabilities.grants {
            if let Ok(parsed) = normalize_grant(raw) {
                if parsed.verb.starts_with("exec/") {
                    offenders.push(format!("{} ({})", p.manifest.plugin.name, parsed.verb));
                    break;
                }
            }
        }
    }
    if !offenders.is_empty() {
        anyhow::bail!(
            "headless daemon refused startup: enabled plugin(s) declare exec capability: {}",
            offenders.join(", ")
        );
    }
    Ok(())
}

/// Headless startup preflight: validates no enabled plugin has `exec/*`,
/// probes `superbrain.db` integrity, and ensures the folder-index root exists.
/// Does not start SANCHO's clock, plugin tasks, or the agent supervisor.
pub(crate) fn headless_preflight(home: &std::path::Path) -> Result<std::path::PathBuf> {
    use makakoo_core::plugin::PluginRegistry;
    use makakoo_core::superbrain::store::SuperbrainStore;

    let plugins = PluginRegistry::load_default(home)?;
    validate_headless_plugins(&plugins)?;

    let db_path = home.join("data").join("superbrain.db");
    let store = SuperbrainStore::open(&db_path)?;
    {
        let arc = store.conn_arc();
        let conn = arc.lock().expect("integrity probe conn poisoned");
        match conn.query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0)) {
            Ok(status) if status == "ok" => {
                info!("headless superbrain.db integrity check: ok");
            }
            Ok(status) => {
                error!(first_issue = %status, "headless superbrain.db integrity check FAILED");
            }
            Err(e) => {
                error!(error = %e, "headless superbrain.db integrity check could not run");
            }
        }
    }

    let folder_index_dir = std::env::var("MAKAKOO_FOLDER_INDEX_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join("data").join("folder-indexes"));
    std::fs::create_dir_all(&folder_index_dir)?;
    Ok(folder_index_dir)
}

/// Headless daemon main loop: Superbrain integrity probe, folder-index
/// directory readiness, and subprocess-free heartbeat only. No SANCHO clock,
/// no plugin task scheduling, no agent supervisor, and no OS keyring/launchd.
pub async fn run_headless() -> Result<()> {
    let home = makakoo_core::platform::makakoo_home();
    let folder_index_dir = headless_preflight(&home)?;
    info!(
        makakoo_home = %home.display(),
        folder_index_dir = %folder_index_dir.display(),
        "makakoo daemon starting in headless mode (sancho clock off, agent supervisor off)"
    );

    let watchdog_shutdown = Arc::new(tokio::sync::Notify::new());
    let watchdog_handle = watchdog::spawn_headless(
        home,
        watchdog::DEFAULT_WATCHDOG_INTERVAL,
        Arc::clone(&watchdog_shutdown),
    );

    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received — stopping headless watchdog");
    watchdog_shutdown.notify_waiters();
    let _ = watchdog_handle.await;
    info!("headless daemon exiting");
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

    fn write_test_plugin(home: &std::path::Path, name: &str, grants: &str, extra: &str) {
        let dir = home.join("plugins").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let body = format!(
            r#"
[plugin]
name = "{name}"
version = "0.1.0"
kind = "skill"
language = "python"

[source]
path = "plugins-core/{name}"

[abi]
skill = "^1.0"

[entrypoint]
run = "python3 run.py"

[capabilities]
grants = [{grants}]
{extra}
"#
        );
        std::fs::write(dir.join("plugin.toml"), body).unwrap();
    }

    #[test]
    fn headless_rejects_enabled_plugin_with_exec_shell() {
        let tmp = tempfile::tempdir().unwrap();
        write_test_plugin(
            tmp.path(),
            "shell-runner",
            "\"exec/shell\"",
            "[state]\ndir = \"$MAKAKOO_HOME/state/shell-runner\"\nretention = \"purge_on_uninstall\"\n",
        );
        let err = headless_preflight(tmp.path()).unwrap_err();
        assert!(
            err.to_string().contains("exec/shell"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn headless_rejects_enabled_plugin_with_exec_binary() {
        let tmp = tempfile::tempdir().unwrap();
        write_test_plugin(tmp.path(), "bin-runner", "\"exec/binary:curl\"", "");
        let err = headless_preflight(tmp.path()).unwrap_err();
        assert!(
            err.to_string().contains("exec/binary"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn headless_preflight_allows_clean_plugins_and_initializes_folder_index_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write_test_plugin(tmp.path(), "clean-reader", "\"brain/read\"", "");
        let fi_dir = headless_preflight(tmp.path()).unwrap();
        assert!(fi_dir.is_dir());
        assert!(tmp.path().join("data").join("superbrain.db").is_file());
    }

    #[test]
    fn daemon_sancho_engine_shares_process_brain_rebuild_coordinator() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("data/Brain/journals")).unwrap();
        let ctx = crate::context::CliContext::for_home(tmp.path().to_path_buf());
        let (_store, engine) = build_daemon_sancho_engine(&ctx).unwrap();
        assert!(
            Arc::ptr_eq(&ctx.brain_rebuild(), &engine.context().brain_rebuild),
            "daemon SANCHO engine must share the same BrainRebuildCoordinator Arc as the open probe"
        );
    }
}
