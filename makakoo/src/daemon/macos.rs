//! macOS launchd plist writer.
//!
//! `makakoo daemon install` drops a LaunchAgent plist at
//! `~/Library/LaunchAgents/com.makakoo.daemon.plist` and runs
//! `launchctl load` on it. Uninstall runs `launchctl unload` and removes
//! the plist. Both operations are idempotent.

use std::path::PathBuf;

use anyhow::{anyhow, Result};

pub const LABEL: &str = "com.makakoo.daemon";
pub const PLIST_FILENAME: &str = "com.makakoo.daemon.plist";

pub fn plist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?;
    Ok(home.join("Library/LaunchAgents").join(PLIST_FILENAME))
}

pub fn render_plist(exe: &std::path::Path, log_dir: &std::path::Path, home: &std::path::Path) -> String {
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

pub fn install() -> Result<PathBuf> {
    let path = plist_path()?;
    std::fs::create_dir_all(path.parent().unwrap())?;

    let exe = std::env::current_exe()?;
    let log_dir = makakoo_core::platform::data_dir().join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let home = makakoo_core::platform::makakoo_home();

    let plist = render_plist(&exe, &log_dir, &home);
    std::fs::write(&path, plist)?;

    // Best-effort load; ignore errors so the install is still idempotent if
    // the agent is already loaded. The plist file is the source of truth.
    let _ = std::process::Command::new("launchctl")
        .args(["load", path.to_str().unwrap_or("")])
        .status();
    Ok(path)
}

pub fn uninstall() -> Result<()> {
    let path = plist_path()?;
    if path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", path.to_str().unwrap_or("")])
            .status();
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn is_installed() -> bool {
    plist_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn is_running() -> bool {
    std::process::Command::new("launchctl")
        .args(["list", LABEL])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn plist_shape_is_well_formed_xml() {
        let plist = render_plist(
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
        let p = plist_path().unwrap();
        assert!(p.ends_with(PLIST_FILENAME));
        assert!(p.to_string_lossy().contains("LaunchAgents"));
    }
}
