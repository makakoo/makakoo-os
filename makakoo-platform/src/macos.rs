//! macOS platform adapter — launchd + `~/Library/LaunchAgents/` + native
//! symlinks.
//!
//! The daemon is registered as a LaunchAgent plist at
//! `~/Library/LaunchAgents/com.makakoo.daemon.plist`. Install writes the
//! file and runs `launchctl load` (best-effort); uninstall runs
//! `launchctl unload` and removes the file.
//!
//! Symlinks are native POSIX symlinks via `std::os::unix::fs::symlink`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::PlatformAdapter;

pub const LABEL: &str = "com.makakoo.daemon";
pub const PLIST_FILENAME: &str = "com.makakoo.daemon.plist";

#[derive(Debug, Default, Clone, Copy)]
pub struct MacOsPlatform;

impl MacOsPlatform {
    pub fn plist_path() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?;
        Ok(home.join("Library/LaunchAgents").join(PLIST_FILENAME))
    }

    pub fn render_plist(
        exe: &Path,
        log_dir: &Path,
        home: &Path,
    ) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log_dir}/makakoo.out.log</string>
    <key>StandardErrorPath</key>
    <string>{log_dir}/makakoo.err.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>MAKAKOO_HOME</key>
        <string>{home}</string>
    </dict>
</dict>
</plist>
"#,
            label = LABEL,
            exe = exe.display(),
            log_dir = log_dir.display(),
            home = home.display()
        )
    }
}

impl PlatformAdapter for MacOsPlatform {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn default_home(&self) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".makakoo")
    }

    fn daemon_install(&self) -> Result<PathBuf> {
        let path = Self::plist_path()?;
        std::fs::create_dir_all(path.parent().unwrap())?;

        let exe = std::env::current_exe()?;
        let log_dir = crate::paths::data_dir().join("logs");
        std::fs::create_dir_all(&log_dir)?;
        let home = crate::paths::makakoo_home();

        let plist = Self::render_plist(&exe, &log_dir, &home);
        std::fs::write(&path, plist)?;

        // Best-effort load; plist file is the source of truth if launchctl fails.
        let _ = std::process::Command::new("launchctl")
            .args(["load", path.to_str().unwrap_or("")])
            .status();
        Ok(path)
    }

    fn daemon_uninstall(&self) -> Result<()> {
        let path = Self::plist_path()?;
        if path.exists() {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", path.to_str().unwrap_or("")])
                .status();
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn daemon_is_installed(&self) -> bool {
        Self::plist_path().map(|p| p.exists()).unwrap_or(false)
    }

    fn daemon_is_running(&self) -> bool {
        std::process::Command::new("launchctl")
            .args(["list", LABEL])
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
    fn plist_shape_is_well_formed_xml() {
        let plist = MacOsPlatform::render_plist(
            &PathBuf::from("/opt/makakoo/bin/makakoo"),
            &PathBuf::from("/var/log/makakoo"),
            &PathBuf::from("/tmp/makakoo-test-home"),
        );
        assert!(plist.starts_with("<?xml version=\"1.0\""));
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains(LABEL));
        assert!(plist.contains("/opt/makakoo/bin/makakoo"));
        assert!(plist.contains("MAKAKOO_HOME"));
        assert!(plist.contains("/tmp/makakoo-test-home"));
        assert!(plist.trim_end().ends_with("</plist>"));
    }

    #[test]
    fn plist_path_under_library_launch_agents() {
        let p = MacOsPlatform::plist_path().unwrap();
        assert!(p.ends_with(PLIST_FILENAME));
        assert!(p.to_string_lossy().contains("LaunchAgents"));
    }

    #[test]
    fn default_home_ends_with_dot_makakoo() {
        let p = MacOsPlatform.default_home();
        assert!(p.ends_with(".makakoo"));
    }

    #[test]
    fn can_symlink_is_true_on_macos() {
        assert!(MacOsPlatform.can_symlink());
    }

    #[test]
    fn name_is_macos() {
        assert_eq!(MacOsPlatform.name(), "macos");
    }

    #[test]
    fn symlink_dir_creates_native_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");

        MacOsPlatform.symlink_dir(&target, &link).unwrap();
        assert!(link.exists());
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    }
}
