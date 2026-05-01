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
    ("pi", "pi"),
    ("kimi", "kimi"),
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

/// Extension hosts covered by INSTALL_MATRIX §3.8 (VSCode extensions)
/// and §3.9 (JetBrains AI). These don't fit the 7-CLI `SLOTS` table
/// because their paths are OS-specific application-support dirs,
/// not `~/.foo/`. Detection only — infect of these hosts is Phase F/5+.
#[derive(Debug, Clone)]
pub struct DetectedExtHost {
    pub id: &'static str,
    pub display_name: &'static str,
    pub config_path: PathBuf,
    pub config_exists: bool,
}

impl DetectedExtHost {
    pub fn is_detected(&self) -> bool {
        self.config_exists
    }
}

/// Detect the 4 extension-based hosts (VSCode Copilot + Continue + Cline
/// + JetBrains AI). On macOS these live under
/// `~/Library/Application Support/…`; on Linux under `~/.config/…`; on
/// Windows under `%APPDATA%…`.
pub fn detect_ext_hosts(home: &Path) -> Vec<DetectedExtHost> {
    let mut out = Vec::new();

    // 1. GitHub Copilot for VSCode — user-level copilot-instructions.md
    if let Some(path) = vscode_user_dir(home).map(|d| d.join("copilot-instructions.md")) {
        out.push(DetectedExtHost {
            id: "vscode-copilot",
            display_name: "VSCode + GitHub Copilot",
            config_exists: path.parent().map(|p| p.exists()).unwrap_or(false),
            config_path: path,
        });
    }

    // 2. Continue.dev — ~/.continue/config.json (Continue reads
    //    systemMessage from here). All three OSes use $HOME/.continue.
    let continue_path = home.join(".continue/config.json");
    out.push(DetectedExtHost {
        id: "continue-dev",
        display_name: "Continue.dev",
        config_exists: home.join(".continue").is_dir(),
        config_path: continue_path,
    });

    // 3. Cline (Claude Dev) — VSCode extension globalStorage. Long path.
    if let Some(user_dir) = vscode_user_dir(home) {
        let cline_dir =
            user_dir.join("globalStorage/saoudrizwan.claude-dev");
        let cline_md = cline_dir.join("CLAUDE.md");
        out.push(DetectedExtHost {
            id: "cline",
            display_name: "Cline (Claude Dev)",
            config_exists: cline_dir.is_dir(),
            config_path: cline_md,
        });
    }

    // 4. JetBrains AI — scan for any IDE product-version directory under
    //    the vendor root. Pick the first found; infecting multiple IDEs
    //    is Phase F/5 scope.
    let jb_root = jetbrains_config_root(home);
    if let Some(ref root) = jb_root {
        let mut product_dirs: Vec<PathBuf> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(root) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // JetBrains dirs look like "IntelliJIdea2025.1" /
                // "PyCharm2025.1" / "GoLand2024.3" / "WebStorm2025.1" /
                // "RustRover2025.1". The product-version shape is our
                // filter: first letter uppercase + contains a version
                // digit somewhere after the prefix.
                if name.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
                    && name.chars().any(|c| c.is_ascii_digit())
                {
                    product_dirs.push(entry.path());
                }
            }
        }
        // Pick newest (alphabetical sort + last since versions like
        // 2024.3 < 2025.1 when sorted lexically).
        product_dirs.sort();
        if let Some(latest) = product_dirs.into_iter().next_back() {
            let rules = latest.join("AI_Assistant/rules.md");
            out.push(DetectedExtHost {
                id: "jetbrains-ai",
                display_name: format!(
                    "JetBrains AI ({})",
                    latest.file_name().unwrap_or_default().to_string_lossy()
                )
                .leak(),
                config_exists: latest.is_dir(),
                config_path: rules,
            });
        }
    }

    out
}

fn vscode_user_dir(home: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/Code/User"))
    } else if cfg!(target_os = "linux") {
        Some(home.join(".config/Code/User"))
    } else if cfg!(target_os = "windows") {
        // On Windows the User dir is under %APPDATA%, which isn't
        // trivially derivable from $HOME. Callers on Windows should
        // use `dirs::data_dir()` or check `%APPDATA%` directly. For
        // now we return a best-guess path that may not be accurate.
        std::env::var_os("APPDATA")
            .map(|p| PathBuf::from(p).join("Code/User"))
            .or_else(|| Some(home.join("AppData/Roaming/Code/User")))
    } else {
        None
    }
}

fn jetbrains_config_root(home: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/JetBrains"))
    } else if cfg!(target_os = "linux") {
        Some(home.join(".config/JetBrains"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(|p| PathBuf::from(p).join("JetBrains"))
            .or_else(|| Some(home.join("AppData/Roaming/JetBrains")))
    } else {
        None
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
    fn ext_hosts_empty_home_returns_not_detected() {
        let tmp = TempDir::new().unwrap();
        let hosts = detect_ext_hosts(tmp.path());
        for h in &hosts {
            assert!(!h.is_detected(), "{} shouldn't be detected", h.id);
        }
        // Order + presence: continue + copilot + cline always shipped
        // as probe targets. JetBrains conditional on the vendor root
        // existing, so not guaranteed here.
        let ids: Vec<&str> = hosts.iter().map(|h| h.id).collect();
        assert!(ids.contains(&"continue-dev"));
    }

    #[test]
    fn continue_dev_detected_when_config_dir_exists() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".continue")).unwrap();
        let hosts = detect_ext_hosts(tmp.path());
        let cont = hosts.iter().find(|h| h.id == "continue-dev").unwrap();
        assert!(cont.is_detected());
        assert!(cont
            .config_path
            .to_string_lossy()
            .contains(".continue/config.json"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn jetbrains_detected_with_product_version_dir() {
        let tmp = TempDir::new().unwrap();
        let jb = tmp
            .path()
            .join("Library/Application Support/JetBrains/IntelliJIdea2025.1");
        fs::create_dir_all(&jb).unwrap();
        let hosts = detect_ext_hosts(tmp.path());
        let jb_host = hosts.iter().find(|h| h.id == "jetbrains-ai");
        assert!(jb_host.is_some(), "JetBrains should be detected");
        let h = jb_host.unwrap();
        assert!(h.is_detected());
        assert!(h.config_path.ends_with("AI_Assistant/rules.md"));
        assert!(h.display_name.contains("IntelliJIdea2025.1"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn jetbrains_picks_newest_product_version() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("Library/Application Support/JetBrains");
        for name in ["IntelliJIdea2024.1", "IntelliJIdea2025.1", "PyCharm2024.3"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let hosts = detect_ext_hosts(tmp.path());
        let h = hosts.iter().find(|h| h.id == "jetbrains-ai").unwrap();
        // PyCharm2024.3 is alphabetically last → picked.
        // This is a deliberate simplification; Phase F/5 can infect all.
        assert!(h.display_name.contains("PyCharm2024.3"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cline_detected_when_globalstorage_exists() {
        let tmp = TempDir::new().unwrap();
        let cline = tmp
            .path()
            .join("Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev");
        fs::create_dir_all(&cline).unwrap();
        let hosts = detect_ext_hosts(tmp.path());
        let h = hosts.iter().find(|h| h.id == "cline").unwrap();
        assert!(h.is_detected());
        assert!(h.config_path.ends_with("CLAUDE.md"));
    }

    #[test]
    fn probe_covers_canonical_slots() {
        // Open-ended assertion per feedback `dont_pin_cli_count` —
        // the roster grows as new AI CLIs/IDEs land, so the test
        // checks for known slots being present rather than pinning
        // a specific count. Adding a new slot should not break
        // this test; only forgetting to wire one should.
        let tmp = TempDir::new().unwrap();
        let all = detect_all(tmp.path());
        let names: std::collections::HashSet<&str> =
            all.iter().map(|h| h.name).collect();
        for required in &[
            "claude", "gemini", "codex", "opencode", "vibe", "cursor", "qwen", "pi", "kimi",
        ] {
            assert!(
                names.contains(required),
                "expected slot {required:?} in detect_all, got {names:?}"
            );
        }
        assert!(
            all.len() >= 8,
            "expected at least 8 slots, got {} ({names:?})",
            all.len()
        );
    }
}
