//! Linux systemd user service writer.
//!
//! `makakoo daemon install` drops a user-scoped unit file at
//! `~/.config/systemd/user/makakoo.service`, reloads the daemon, enables
//! the unit, and starts it. All three systemctl calls are best-effort —
//! the unit file is the source of truth.

use std::path::PathBuf;

use anyhow::{anyhow, Result};

pub const UNIT_FILENAME: &str = "makakoo.service";

pub fn unit_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?;
    Ok(home.join(".config/systemd/user").join(UNIT_FILENAME))
}

pub fn render_unit(exe: &std::path::Path, home: &std::path::Path) -> String {
    format!(
        r#"[Unit]
Description=Makakoo OS daemon — Harvey's persistent body
After=network.target

[Service]
Type=simple
ExecStart={exe} daemon run
Restart=always
RestartSec=5
Environment=MAKAKOO_HOME={home}

[Install]
WantedBy=default.target
"#,
        exe = exe.display(),
        home = home.display(),
    )
}

pub fn install() -> Result<PathBuf> {
    let path = unit_path()?;
    std::fs::create_dir_all(path.parent().unwrap())?;

    let exe = std::env::current_exe()?;
    let home = makakoo_core::platform::makakoo_home();
    std::fs::write(&path, render_unit(&exe, &home))?;

    // Best-effort daemon-reload / enable / start. If systemd isn't running
    // the unit file is still on disk and the user can start it manually.
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "enable", UNIT_FILENAME])
        .status();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "start", UNIT_FILENAME])
        .status();
    Ok(path)
}

pub fn uninstall() -> Result<()> {
    let path = unit_path()?;
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", UNIT_FILENAME])
        .status();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", UNIT_FILENAME])
        .status();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    Ok(())
}

pub fn is_installed() -> bool {
    unit_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn is_running() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", UNIT_FILENAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn unit_shape_has_exec_start_and_env() {
        let u = render_unit(
            &PathBuf::from("/usr/local/bin/makakoo"),
            &PathBuf::from("/tmp/makakoo-test-home"),
        );
        assert!(u.contains("[Unit]"));
        assert!(u.contains("[Service]"));
        assert!(u.contains("[Install]"));
        assert!(u.contains("ExecStart=/usr/local/bin/makakoo daemon run"));
        assert!(u.contains("Environment=MAKAKOO_HOME=/tmp/makakoo-test-home"));
        assert!(u.contains("Restart=always"));
    }
}
