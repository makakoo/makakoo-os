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

/// Render the daemon-restart command users can copy-paste.
///
/// `makakoo daemon restart` intentionally re-registers the OS service
/// descriptor before starting it, so upgrades installed into a new Homebrew
/// Cellar/prefix path do not leave launchd/systemd pointing at the old binary.
pub fn daemon_restart_hint() -> String {
    "makakoo daemon restart".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_version_returns_none_for_missing_binary() {
        assert_eq!(
            capture_version("/definitely/not/a/binary/makakoo-12345"),
            None
        );
    }

    #[test]
    fn daemon_restart_hint_is_non_empty() {
        let hint = daemon_restart_hint();
        assert!(!hint.is_empty());
    }

    #[test]
    fn restart_hint_uses_first_class_cli_command() {
        assert_eq!(daemon_restart_hint(), "makakoo daemon restart");
    }
}
