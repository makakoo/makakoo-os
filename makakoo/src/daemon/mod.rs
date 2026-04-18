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
/// task and blocks on Ctrl-C.
pub async fn run_forever() -> Result<()> {
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
    let bus = ctx.event_bus()?;
    let llm = ctx.llm();
    let emb = ctx.embeddings();
    let sancho_ctx = Arc::new(SanchoContext::new(store, bus, llm, emb, home));
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

    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received — stopping sancho");
    shutdown.notify_waiters();
    let _ = sancho_handle.await;
    info!("daemon exiting");
    Ok(())
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
