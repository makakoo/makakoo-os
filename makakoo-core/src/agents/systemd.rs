//! Linux systemd-user integration — generator + invocation helpers.
//!
//! Locked design (Phase 0 Q1):
//!
//! 1. `SystemdUserUnit::from_slot(slot_id, makakoo_bin)` returns the
//!    INI body for `~/.config/systemd/user/makakoo-agent-<slot>.service`.
//!
//! 2. `SystemctlInstaller::reload_and_start(unit)` runs
//!    `systemctl --user daemon-reload && systemctl --user start ...`.
//!
//! Restart=on-failure + RestartSec=10s gives systemd-side restart
//! that complements the user-space supervisor restart budget; the
//! supervisor's circuit-breaker still trips after 6 crashes/min,
//! systemd just keeps the process around if the kernel kills it.

use std::path::{Path, PathBuf};

use crate::agents::slot::validate_slot_id;
use crate::error::{MakakooError, Result};

/// Generated systemd user unit body + the path it should land at.
#[derive(Debug, Clone)]
pub struct SystemdUserUnit {
    pub unit_name: String,
    pub unit_body: String,
    pub unit_path: PathBuf,
}

impl SystemdUserUnit {
    /// `os_home` is the OS user $HOME — the unit MUST land under
    /// `$HOME/.config/systemd/user/` (systemd-user requirement).
    /// `makakoo_home` is the Makakoo install root — used for log
    /// file paths under `$MAKAKOO_HOME/data/log/`.
    pub fn from_slot(
        slot_id: &str,
        makakoo_bin: &Path,
        os_home: &Path,
        makakoo_home: &Path,
    ) -> Result<Self> {
        validate_slot_id(slot_id).map_err(|e| {
            MakakooError::Internal(format!("invalid slot id '{slot_id}': {e}"))
        })?;
        let unit_name = format!("makakoo-agent-{slot_id}.service");
        let unit_path = os_home
            .join(".config/systemd/user")
            .join(&unit_name);
        let body = render_unit_body(slot_id, makakoo_bin, makakoo_home);
        Ok(Self {
            unit_name,
            unit_body: body,
            unit_path,
        })
    }

    pub fn write(&self) -> Result<&Path> {
        if let Some(parent) = self.unit_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MakakooError::Internal(format!("create systemd user dir: {e}"))
            })?;
        }
        std::fs::write(&self.unit_path, &self.unit_body).map_err(|e| {
            MakakooError::Internal(format!("write {}: {e}", self.unit_path.display()))
        })?;
        Ok(&self.unit_path)
    }
}

#[derive(Debug)]
pub struct SystemctlOutput {
    pub exit_code: i32,
    pub stderr: String,
}

pub trait SystemctlExec: Send + Sync {
    fn daemon_reload(&self) -> Result<SystemctlOutput>;
    fn start(&self, unit_name: &str) -> Result<SystemctlOutput>;
    fn stop(&self, unit_name: &str) -> Result<SystemctlOutput>;
    fn status(&self, unit_name: &str) -> Result<SystemctlOutput>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RealSystemctl;

impl SystemctlExec for RealSystemctl {
    fn daemon_reload(&self) -> Result<SystemctlOutput> {
        run(&["--user", "daemon-reload"])
    }
    fn start(&self, unit_name: &str) -> Result<SystemctlOutput> {
        run(&["--user", "start", unit_name])
    }
    fn stop(&self, unit_name: &str) -> Result<SystemctlOutput> {
        run(&["--user", "stop", unit_name])
    }
    fn status(&self, unit_name: &str) -> Result<SystemctlOutput> {
        run(&["--user", "status", unit_name])
    }
}

fn run(args: &[&str]) -> Result<SystemctlOutput> {
    let out = std::process::Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|e| MakakooError::Internal(format!("invoke systemctl: {e}")))?;
    Ok(SystemctlOutput {
        exit_code: out.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Render the systemd user unit. Locked schema:
///
/// * `Type=simple`              — supervisor stays in foreground.
/// * `Restart=on-failure`       — restart only on non-zero exit.
/// * `RestartSec=10`            — 10s minimum between systemd restarts;
///                                user-space backoff handles finer
///                                granularity inside this window.
/// * `Environment=...`          — propagates MAKAKOO_AGENT_SLOT.
/// * `StandardOutput=append:`   — log to `$MAKAKOO_HOME/data/log/agent-<slot>.out.log`.
///
/// Path quoting: `ExecStart` and `StandardOutput=append:` accept
/// double-quoted strings to handle paths with spaces. We always
/// quote, even when not strictly needed, for resilience.
fn render_unit_body(slot_id: &str, makakoo_bin: &Path, makakoo_home: &Path) -> String {
    let bin = systemd_quote(&makakoo_bin.to_string_lossy());
    let stdout = systemd_quote(
        &makakoo_home
            .join(format!("data/log/agent-{slot_id}.out.log"))
            .to_string_lossy(),
    );
    let stderr = systemd_quote(
        &makakoo_home
            .join(format!("data/log/agent-{slot_id}.err.log"))
            .to_string_lossy(),
    );
    format!(
        r#"[Unit]
Description=Makakoo agent slot {slot_id}
After=default.target

[Service]
Type=simple
ExecStart={bin} agent _supervisor --slot {slot_id}
Environment=MAKAKOO_AGENT_SLOT={slot_id}
Restart=on-failure
RestartSec=10
StandardOutput=append:{stdout}
StandardError=append:{stderr}

[Install]
WantedBy=default.target
"#
    )
}

/// systemd unit-file string quoting. Uses double-quotes; escapes
/// any embedded `"` and `\`. Matches the `systemd.unit(5)` rules.
fn systemd_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn unit_path_is_under_user_systemd_config() {
        let home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let u = SystemdUserUnit::from_slot("secretary", &bin, home.path(), home.path()).unwrap();
        assert_eq!(u.unit_name, "makakoo-agent-secretary.service");
        assert!(
            u.unit_path
                .ends_with(".config/systemd/user/makakoo-agent-secretary.service"),
            "unit path: {}",
            u.unit_path.display()
        );
    }

    #[test]
    fn unit_body_embeds_slot_supervisor_invocation() {
        let home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let u = SystemdUserUnit::from_slot("secretary", &bin, home.path(), home.path()).unwrap();
        assert!(u.unit_body.contains("Description=Makakoo agent slot secretary"));
        assert!(
            u.unit_body
                .contains("ExecStart=/usr/local/bin/makakoo agent _supervisor --slot secretary")
        );
        assert!(u.unit_body.contains("Environment=MAKAKOO_AGENT_SLOT=secretary"));
        assert!(u.unit_body.contains("Restart=on-failure"));
        assert!(u.unit_body.contains("RestartSec=10"));
        assert!(u.unit_body.contains("StandardOutput=append:"));
    }

    #[test]
    fn unit_rejects_invalid_slot_id() {
        let home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        assert!(SystemdUserUnit::from_slot("Bad Slot!", &bin, home.path(), home.path()).is_err());
    }

    #[test]
    fn unit_path_uses_os_home_log_paths_use_makakoo_home() {
        let os_home = TempDir::new().unwrap();
        let makakoo_home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let u = SystemdUserUnit::from_slot(
            "secretary",
            &bin,
            os_home.path(),
            makakoo_home.path(),
        )
        .unwrap();
        assert!(u.unit_path.starts_with(os_home.path()));
        assert!(!u.unit_path.starts_with(makakoo_home.path()));
        let mh = makakoo_home.path().to_string_lossy().into_owned();
        assert!(
            u.unit_body
                .contains(&format!("{mh}/data/log/agent-secretary.out.log")),
            "stdout log must use Makakoo-home path; got body:\n{}",
            u.unit_body
        );
    }

    #[test]
    fn unit_quotes_paths_with_spaces() {
        let os_home = TempDir::new().unwrap();
        let makakoo_home = TempDir::new().unwrap();
        let bin = PathBuf::from("/opt/My Apps/makakoo");
        let u = SystemdUserUnit::from_slot(
            "secretary",
            &bin,
            os_home.path(),
            makakoo_home.path(),
        )
        .unwrap();
        assert!(
            u.unit_body
                .contains(r#"ExecStart="/opt/My Apps/makakoo" agent _supervisor"#),
            "ExecStart must double-quote paths with spaces:\n{}",
            u.unit_body
        );
    }

    #[test]
    fn unit_write_creates_systemd_user_dir() {
        let home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let u = SystemdUserUnit::from_slot("secretary", &bin, home.path(), home.path()).unwrap();
        let path = u.write().unwrap();
        assert!(path.exists());
        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains("ExecStart="));
    }
}
