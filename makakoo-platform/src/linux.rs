//! Linux platform adapter — systemd user services + XDG paths + native symlinks.
//!
//! The daemon is registered as a systemd user unit at
//! `~/.config/systemd/user/makakoo.service`. Install writes the file and
//! runs `systemctl --user daemon-reload / enable / start` (best-effort);
//! uninstall runs `systemctl --user stop / disable` and removes the file.
//!
//! Symlinks are native POSIX symlinks via `std::os::unix::fs::symlink`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::PlatformAdapter;

pub const UNIT_FILENAME: &str = "makakoo.service";

#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxPlatform;

impl LinuxPlatform {
    pub fn unit_path() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?;
        Ok(home.join(".config/systemd/user").join(UNIT_FILENAME))
    }

    pub fn render_unit(exe: &Path, home: &Path) -> String {
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
}

impl PlatformAdapter for LinuxPlatform {
    fn name(&self) -> &'static str {
        "linux"
    }

    fn default_home(&self) -> PathBuf {
        // XDG_DATA_HOME takes precedence, fallback to ~/.local/share/makakoo.
        if let Ok(x) = std::env::var("XDG_DATA_HOME") {
            if !x.is_empty() {
                return PathBuf::from(x).join("makakoo");
            }
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/share/makakoo")
    }

    fn daemon_install(&self) -> Result<PathBuf> {
        let path = Self::unit_path()?;
        std::fs::create_dir_all(path.parent().unwrap())?;

        let exe = std::env::current_exe()?;
        let home = crate::paths::makakoo_home();
        std::fs::write(&path, Self::render_unit(&exe, &home))?;

        // Best-effort. Unit file is the source of truth even if systemd
        // isn't running (e.g. container builds).
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

    fn daemon_uninstall(&self) -> Result<()> {
        let path = Self::unit_path()?;
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

    fn daemon_is_installed(&self) -> bool {
        Self::unit_path().map(|p| p.exists()).unwrap_or(false)
    }

    fn daemon_is_running(&self) -> bool {
        std::process::Command::new("systemctl")
            .args(["--user", "is-active", UNIT_FILENAME])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn symlink_dir(&self, target: &Path, link: &Path) -> Result<()> {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::os::unix::fs::symlink(target, link)?;
        Ok(())
    }

    fn can_symlink(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_shape_has_exec_start_and_env() {
        let u = LinuxPlatform::render_unit(
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

    #[test]
    fn default_home_follows_xdg() {
        // Without XDG_DATA_HOME set, falls back to ~/.local/share/makakoo
        std::env::remove_var("XDG_DATA_HOME");
        let p = LinuxPlatform.default_home();
        assert!(p.ends_with("makakoo"));
        assert!(p.to_string_lossy().contains(".local/share"));

        // With XDG_DATA_HOME set, uses it
        std::env::set_var("XDG_DATA_HOME", "/tmp/test-xdg-data");
        let p2 = LinuxPlatform.default_home();
        assert_eq!(p2, PathBuf::from("/tmp/test-xdg-data/makakoo"));
        std::env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    fn can_symlink_is_true_on_linux() {
        assert!(LinuxPlatform.can_symlink());
    }

    #[test]
    fn name_is_linux() {
        assert_eq!(LinuxPlatform.name(), "linux");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn symlink_dir_creates_native_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");

        LinuxPlatform.symlink_dir(&target, &link).unwrap();
        assert!(link.exists());
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    }
}
