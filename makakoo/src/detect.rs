//! CLI host detection — which AI tools live on this machine.
//!
//! Spec: `spec/INSTALL_MATRIX.md`. Phase F/1 ships the 7 primary CLI
//! hosts (claude, gemini, codex, opencode, vibe, cursor, qwen). VSCode
//! extensions (Copilot / Continue / Cline) and JetBrains AI ship in a
//! later slice — they have weirder path patterns (globalStorage, per-
//! version IDE dirs) and benefit from their own iteration cycle.
//!
//! Detection strategy per §1:
//!
//! 1. **Binary probe:** is the host's CLI binary on `PATH`?
//! 2. **Config probe:** does the host's canonical config dir exist?
//!
//! A host is detected if EITHER probe succeeds. False positives are
//! not acceptable (we'd infect something the host isn't using);
//! false negatives are tolerable (user can override with
//! `--config-path`).
//!
//! The name → config-path table is already in `infect::slots::SLOTS`;
//! this module layers binary-probe info on top so `makakoo install`
//! can show both signals in the plan.

use std::path::{Path, PathBuf};

use crate::infect::slots::{CliSlot, SLOTS};

/// Binary name for each of the 7 CLI hosts. Keyed by slot name so the
/// order matches `SLOTS`.
pub const BINARIES: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("gemini", "gemini"),
    ("codex", "codex"),
    ("opencode", "opencode"),
    ("vibe", "vibe"),
    ("cursor", "cursor"),
    ("qwen", "qwen"),
];

/// Result of probing one host.
#[derive(Debug, Clone)]
pub struct DetectedHost {
    pub name: &'static str,
    /// Absolute path of the binary on `PATH`, if any.
    pub binary_on_path: Option<PathBuf>,
    /// Where the instructions file lives (whether or not it exists).
    pub instructions_path: PathBuf,
    /// Whether the instructions file exists on disk.
    pub instructions_exists: bool,
    /// Whether the instructions file already contains our bootstrap
    /// marker. Tells `makakoo install` whether it's a fresh infect
    /// or a no-op refresh.
    pub bootstrap_present: bool,
}

impl DetectedHost {
    /// A host is "detected" if EITHER the binary is on PATH OR the
    /// config dir exists. Spec §1.
    pub fn is_detected(&self) -> bool {
        self.binary_on_path.is_some() || self.instructions_exists
    }
}

/// Probe the 7 CLI hosts. `home` is the user's home dir — pass
/// `dirs::home_dir().unwrap_or_default()` in production, inject a
/// tempdir in tests.
pub fn detect_all(home: &Path) -> Vec<DetectedHost> {
    SLOTS
        .iter()
        .map(|slot| probe_host(slot, home))
        .collect()
}

/// Detect only hosts where at least one signal (binary OR config)
/// came back positive. Equivalent to `detect_all().into_iter()
/// .filter(|h| h.is_detected())`.
pub fn detect_present(home: &Path) -> Vec<DetectedHost> {
    detect_all(home)
        .into_iter()
        .filter(DetectedHost::is_detected)
        .collect()
}

pub fn probe_host(slot: &CliSlot, home: &Path) -> DetectedHost {
    let instructions_path = slot.absolute(home);
    let instructions_exists = instructions_path.exists();
    let binary_on_path = binary_for(slot.name).and_then(find_on_path);
    let bootstrap_present = if instructions_exists {
        // Cheap check: read the first few KB and look for the marker.
        // Full regex match happens at infect time; here we only care
        // about "is this fresh or refresh?" for the plan output.
        check_bootstrap_marker(&instructions_path)
    } else {
        false
    };
    DetectedHost {
        name: slot.name,
        binary_on_path,
        instructions_path,
        instructions_exists,
        bootstrap_present,
    }
}

/// Binary-name lookup by slot name. Returns `None` for unknown names.
pub fn binary_for(slot_name: &str) -> Option<&'static str> {
    BINARIES
        .iter()
        .find(|(name, _)| *name == slot_name)
        .map(|(_, bin)| *bin)
}

/// `which`-style lookup. Walks `$PATH` entries checking for an
/// executable file named `binary`. Returns the first match.
pub fn find_on_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
        // Windows convention: also check with `.exe` / `.cmd` suffix.
        #[cfg(windows)]
        for ext in &["exe", "cmd", "bat"] {
            let with_ext = candidate.with_extension(ext);
            if is_executable_file(&with_ext) {
                return Some(with_ext);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Quick scan for the infect bootstrap marker. Reads up to 16 KiB from
/// the head of the file — markers always appear near the top or in the
/// system-prompt scope, so this is a safe fast-path.
fn check_bootstrap_marker(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 16 * 1024];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    head.contains("harvey:infect-global") || head.contains("makakoo:infect-global")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn empty_home_has_no_config_files() {
        // A tempdir home obviously has no host config files. Binary
        // probe results are machine-dependent — Sebastian's dev box
        // has all 7 CLIs on PATH, CI may have none. We only assert
        // the filesystem contract here.
        let tmp = TempDir::new().unwrap();
        for h in detect_all(tmp.path()) {
            assert!(!h.instructions_exists, "{} should not exist", h.name);
            assert!(!h.bootstrap_present);
        }
    }

    #[test]
    fn config_only_detection_succeeds() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        // Seed only Claude's config file.
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/CLAUDE.md"), "# empty").unwrap();

        let claude = detect_all(home).into_iter().find(|h| h.name == "claude").unwrap();
        assert!(claude.instructions_exists);
        assert!(claude.is_detected());
        assert!(!claude.bootstrap_present); // no marker yet
    }

    #[test]
    fn bootstrap_marker_detected() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude/CLAUDE.md"),
            "# Claude\n<!-- harvey:infect-global START v9 -->\nbootstrap\n<!-- harvey:infect-global END -->\n",
        )
        .unwrap();
        let claude = detect_all(home).into_iter().find(|h| h.name == "claude").unwrap();
        assert!(claude.bootstrap_present);
    }

    #[test]
    fn find_on_path_returns_real_binary_when_exists() {
        // `sh` is on PATH on every Unix; a reasonable stand-in for the
        // "binary probe finds things" check.
        #[cfg(unix)]
        {
            let p = find_on_path("sh");
            assert!(p.is_some(), "sh should be on PATH on Unix");
            let p = p.unwrap();
            assert!(p.ends_with("sh"));
        }
    }

    #[test]
    fn find_on_path_returns_none_for_ghost() {
        let p = find_on_path("definitely-not-a-real-binary-e8f21a");
        assert!(p.is_none());
    }

    #[test]
    fn binary_for_known_slot() {
        assert_eq!(binary_for("claude"), Some("claude"));
        assert_eq!(binary_for("qwen"), Some("qwen"));
    }

    #[test]
    fn binary_for_unknown_slot() {
        assert_eq!(binary_for("nonexistent"), None);
    }

    #[test]
    fn probe_covers_all_seven_slots() {
        let tmp = TempDir::new().unwrap();
        let all = detect_all(tmp.path());
        assert_eq!(all.len(), 7);
        let names: Vec<&str> = all.iter().map(|h| h.name).collect();
        assert_eq!(
            names,
            vec!["claude", "gemini", "codex", "opencode", "vibe", "cursor", "qwen"]
        );
    }
}
