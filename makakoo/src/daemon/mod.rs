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

use anyhow::Result;
use clap::Subcommand;
use tracing::info;

pub mod install;
pub mod status;
pub mod uninstall;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

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

/// The actual daemon main loop. Spawns the long-running workers and
/// blocks on Ctrl-C.
///
/// Wave-5 scope is deliberately minimal: init tracing, log that the
/// daemon is up, and wait for shutdown. Subsequent waves wire SANCHO,
/// Olibia, and the event bus in here as background tokio tasks.
pub async fn run_forever() -> Result<()> {
    let home = makakoo_core::platform::makakoo_home();
    let log_dir = status::log_dir();
    std::fs::create_dir_all(&log_dir).ok();
    info!(
        makakoo_home = %home.display(),
        log_dir = %log_dir.display(),
        "makakoo daemon starting"
    );

    // Wait for a graceful shutdown signal. Future waves spawn SANCHO /
    // Olibia / bus workers as tokio tasks before this await.
    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received — daemon exiting");
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
