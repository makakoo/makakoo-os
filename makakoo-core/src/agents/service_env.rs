//! Environment construction shared by every supervised-agent service
//! backend (launchd on macOS, systemd-user on Linux).
//!
//! Both service managers start a unit with a deliberately minimal
//! environment. Neither inherits the PATH, the LLM credentials, or the
//! `MAKAKOO_HOME` of the shell that ran `makakoo agent start`. v0.3.0
//! shipped without this, so the supervisor died with "slot not found"
//! and then "Node.js 22.9+ is required". These helpers live outside the
//! per-OS modules so the two backends cannot drift apart — and because
//! `launchd` is `cfg(macos)` and `systemd` is `cfg(linux)`, so neither
//! may depend on the other.

use std::path::Path;

/// Baseline PATH for a supervised agent service, with the verified
/// `node` directory prepended when known.
///
/// launchd hands services `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, and a
/// systemd user unit is equally minimal. Neither includes Homebrew or
/// nvm, so DeepSeek Harness cannot find `node` without this.
pub fn service_path(node_bin_dir: Option<&Path>) -> String {
    const BASE: &str = "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    match node_bin_dir {
        Some(dir) if !dir.as_os_str().is_empty() => {
            let dir = dir.to_string_lossy();
            if BASE.split(':').any(|entry| entry == dir) {
                BASE.to_string()
            } else {
                format!("{dir}:{BASE}")
            }
        }
        _ => BASE.to_string(),
    }
}

/// POSIX single-quote a string for safe inlining into `sh -c`.
/// Wraps in `'...'` and rewrites embedded `'` as `'\''`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_path_prepends_node_dir_without_duplicating_base_entries() {
        let with_node = service_path(Some(Path::new("/opt/nvm/bin")));
        assert!(with_node.starts_with("/opt/nvm/bin:"), "{with_node}");
        // A node dir already in the baseline must not be duplicated.
        let dupe = service_path(Some(Path::new("/opt/homebrew/bin")));
        assert_eq!(dupe.matches("/opt/homebrew/bin").count(), 1, "{dupe}");
        assert!(service_path(None).starts_with("/usr/local/bin:"));
    }

    #[test]
    fn service_path_ignores_an_empty_node_dir() {
        assert_eq!(service_path(Some(Path::new(""))), service_path(None));
    }

    #[test]
    fn shell_quote_neutralizes_embedded_single_quotes() {
        assert_eq!(shell_quote("/tmp/plain"), "'/tmp/plain'");
        assert_eq!(shell_quote("/tmp/it's"), r"'/tmp/it'\''s'");
    }
}
