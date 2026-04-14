//! Daemon status + logs.

use anyhow::Result;
use std::path::PathBuf;

/// Three-state view of the daemon, used by `makakoo daemon status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonState {
    NotInstalled,
    InstalledStopped,
    Running,
}

impl DaemonState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DaemonState::NotInstalled => "not installed",
            DaemonState::InstalledStopped => "installed (stopped)",
            DaemonState::Running => "running",
        }
    }
}

pub fn current_state() -> DaemonState {
    #[cfg(target_os = "macos")]
    {
        if !super::macos::is_installed() {
            DaemonState::NotInstalled
        } else if super::macos::is_running() {
            DaemonState::Running
        } else {
            DaemonState::InstalledStopped
        }
    }
    #[cfg(target_os = "linux")]
    {
        if !super::linux::is_installed() {
            DaemonState::NotInstalled
        } else if super::linux::is_running() {
            DaemonState::Running
        } else {
            DaemonState::InstalledStopped
        }
    }
    #[cfg(target_os = "windows")]
    {
        if !super::windows::is_installed() {
            DaemonState::NotInstalled
        } else if super::windows::is_running() {
            DaemonState::Running
        } else {
            DaemonState::InstalledStopped
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        DaemonState::NotInstalled
    }
}

pub async fn run() -> Result<()> {
    let state = current_state();
    println!("makakoo daemon: {}", state.as_str());
    println!("log dir: {}", log_dir().display());
    Ok(())
}

pub fn log_dir() -> PathBuf {
    makakoo_core::platform::data_dir().join("logs")
}

/// Tail the last `lines` lines from the stdout log file. If no log file
/// exists (daemon never ran) returns a friendly message.
pub async fn tail_logs(lines: usize) -> Result<()> {
    let out = log_dir().join("makakoo.out.log");
    if !out.exists() {
        println!("no daemon log yet at {}", out.display());
        return Ok(());
    }
    let content = std::fs::read_to_string(&out)?;
    let collected: Vec<&str> = content.lines().collect();
    let start = collected.len().saturating_sub(lines);
    for l in &collected[start..] {
        println!("{l}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_state_has_human_labels() {
        assert_eq!(DaemonState::NotInstalled.as_str(), "not installed");
        assert_eq!(DaemonState::InstalledStopped.as_str(), "installed (stopped)");
        assert_eq!(DaemonState::Running.as_str(), "running");
    }

    #[test]
    fn log_dir_under_data_dir() {
        let d = log_dir();
        assert!(d.ends_with("logs"));
    }
}
