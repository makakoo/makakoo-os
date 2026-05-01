//! Post-upgrade version capture + daemon restart hint.
//!
//! `makakoo version` prints text like `makakoo 0.1.0 (gitsha)`. We
//! parse the first line; no `--json` flag exists in v1 by design.

use std::process::Command;

/// Capture the first-line version banner from a `makakoo` binary.
/// Returns the line as-is (e.g. `makakoo 0.1.0 (abc1234)`), or `None`
/// if the binary is unreachable / errored.
pub fn capture_version(binary: &str) -> Option<String> {
    let out = Command::new(binary).arg("version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout.lines().next().map(|s| s.trim().to_string())
}

/// Render the platform-specific daemon-restart command.
///
/// v1 of `makakoo upgrade` does not auto-restart the daemon — see
/// PHASE-0-RESULTS.md P0.3. This helper renders the one-line manual
/// command the user can copy-paste.
pub fn daemon_restart_hint() -> String {
    if cfg!(target_os = "macos") {
        "launchctl kickstart -k gui/$UID/com.traylinx.makakoo".into()
    } else if cfg!(target_os = "linux") {
        "systemctl --user restart makakoo".into()
    } else {
        "(no daemon-restart command for this platform — restart manually if needed)".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_version_returns_none_for_missing_binary() {
        assert_eq!(capture_version("/definitely/not/a/binary/makakoo-12345"), None);
    }

    #[test]
    fn daemon_restart_hint_is_non_empty() {
        let hint = daemon_restart_hint();
        assert!(!hint.is_empty());
    }

    #[test]
    fn macos_hint_uses_launchctl() {
        if cfg!(target_os = "macos") {
            assert!(daemon_restart_hint().contains("launchctl"));
        }
    }

    #[test]
    fn linux_hint_uses_systemctl() {
        if cfg!(target_os = "linux") {
            assert!(daemon_restart_hint().contains("systemctl"));
        }
    }
}
