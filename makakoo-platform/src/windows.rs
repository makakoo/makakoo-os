//! Windows platform adapter — auto-launch Registry + Dev Mode symlinks.
//!
//! The daemon is registered via the `auto-launch` crate, which writes a
//! Registry entry under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//! That's the standard per-user auto-launch hook on Windows and doesn't
//! require admin privileges.
//!
//! Symlinks on Windows require either admin privileges OR Developer Mode
//! to be enabled (Settings → For Developers → Developer Mode, available
//! since Win10 1703 / April 2017). The adapter refuses symlink creation
//! when Developer Mode is off, with a clear error pointing at the toggle
//! (D9 in ARCHITECTURE.md — no copy-sync fallback).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::{PlatformAdapter, PlatformError};

pub const APP_NAME: &str = "Makakoo";

#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPlatform;

impl WindowsPlatform {
    /// Build the `auto-launch` descriptor from the current binary location.
    pub fn build_launch() -> Result<auto_launch::AutoLaunch> {
        let exe = std::env::current_exe()?;
        let path = exe
            .to_str()
            .ok_or_else(|| anyhow!("exe path contains invalid UTF-8"))?;
        let args: Vec<&str> = vec!["daemon", "run"];
        auto_launch::AutoLaunchBuilder::new()
            .set_app_name(APP_NAME)
            .set_app_path(path)
            .set_args(&args)
            .build()
            .map_err(|e| anyhow!("auto-launch build: {e}"))
    }

    /// Probe whether Developer Mode is enabled for the current user.
    ///
    /// Reads `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock\
    /// AllowDevelopmentWithoutDevLicense` which is the canonical flag set
    /// by Settings → For Developers → Developer Mode.
    ///
    /// Returns `false` on any read error — we prefer to refuse symlinks
    /// than silently succeed with broken semantics.
    #[cfg(target_os = "windows")]
    pub fn developer_mode_enabled() -> bool {
        // Lightweight registry probe via the `reg` CLI to avoid pulling
        // in the `winreg` crate for a single one-shot read. The `reg`
        // binary is in every Windows install.
        let out = std::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock",
                "/v",
                "AllowDevelopmentWithoutDevLicense",
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains("0x1")
            }
            _ => false,
        }
    }

    /// Non-Windows cfg path for cross-target compilation. Always returns
    /// false because this stub is only compiled for non-Windows targets.
    #[cfg(not(target_os = "windows"))]
    pub fn developer_mode_enabled() -> bool {
        false
    }
}

impl PlatformAdapter for WindowsPlatform {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn default_home(&self) -> PathBuf {
        // %LOCALAPPDATA%\Makakoo
        if let Some(local) = dirs::data_local_dir() {
            return local.join("Makakoo");
        }
        // Fallback: %USERPROFILE%\Makakoo
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Makakoo")
    }

    fn daemon_install(&self) -> Result<PathBuf> {
        let launch = Self::build_launch()?;
        launch
            .enable()
            .map_err(|e| anyhow!("auto-launch enable: {e}"))?;
        // Auto-launch doesn't give us a "file path" to return. We return
        // the registry key path as a descriptor for logging.
        Ok(PathBuf::from(
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Makakoo",
        ))
    }

    fn daemon_uninstall(&self) -> Result<()> {
        let launch = Self::build_launch()?;
        launch
            .disable()
            .map_err(|e| anyhow!("auto-launch disable: {e}"))?;
        Ok(())
    }

    fn daemon_is_installed(&self) -> bool {
        Self::build_launch()
            .and_then(|l| l.is_enabled().map_err(|e| anyhow!(e.to_string())))
            .unwrap_or(false)
    }

    fn daemon_is_running(&self) -> bool {
        // Windows doesn't give us a cheap "is this process name running"
        // check from stdlib. `is_installed == is_running` is an honest
        // approximation until we shell out to `tasklist`.
        self.daemon_is_installed()
    }

    fn symlink_dir(&self, target: &Path, link: &Path) -> Result<()> {
        if !Self::developer_mode_enabled() {
            return Err(PlatformError::SymlinkNotAllowed.into());
        }
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(target_os = "windows")]
        {
            std::os::windows::fs::symlink_dir(target, link)?;
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Unreachable when the type alias selects this impl; present
            // for cross-target compilation.
            let _ = (target, link);
            Err(PlatformError::NotSupported {
                operation: "symlink_dir",
                platform: "windows-stub-on-non-windows",
            }
            .into())
        }
    }

    fn can_symlink(&self) -> bool {
        Self::developer_mode_enabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_windows() {
        assert_eq!(WindowsPlatform.name(), "windows");
    }

    #[test]
    fn default_home_contains_makakoo() {
        let p = WindowsPlatform.default_home();
        assert!(p.to_string_lossy().contains("Makakoo"));
    }

    #[test]
    fn dev_mode_probe_does_not_panic() {
        // Just make sure the probe returns a bool and doesn't blow up.
        let _ = WindowsPlatform::developer_mode_enabled();
    }
}
