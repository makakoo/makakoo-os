//! MCP-server propagation for the infect parasite.
//!
//! `harvey infect` historically wrote bootstrap markdown into each
//! infected CLI's instructions slot but never touched the CLI's MCP
//! configuration — so a bootstrap could promise tools (`harvey_describe_video`)
//! that the CLI couldn't actually call (vibe drift caught 2026-04-18).
//!
//! This module closes that gap. It owns:
//!
//!   * `McpServerSpec` — the canonical `harvey` server entry every CLI
//!     must register.
//!   * `McpTarget` — the seven (and growing) CLIs we infect, each with
//!     metadata describing where its MCP config lives and what format
//!     it uses.
//!   * Adapters (in `adapters/`) — one per format family. JSON covers
//!     5 CLIs; the two TOML CLIs (Codex inline-table, Vibe array-of-tables)
//!     each get a dedicated writer because their schemas are incompatible.
//!   * Drift detection (in `drift.rs`) — catches missing servers, stale
//!     paths, broken symlinks, recursive symlinks.
//!
//! Every write is idempotent, format-preserving, and atomic.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub mod adapters;
pub mod deep;
pub mod drift;
pub mod target;

pub use target::{McpFormat, McpTarget};

/// The canonical `harvey` MCP server entry. Every infected CLI must
/// register this one server. Other servers (Cursor's GitKraken, etc.)
/// are preserved untouched — we only ever upsert by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    /// Server alias as stored in the CLI's config. Always `"harvey"`.
    pub name: String,
    /// Absolute path to the `makakoo-mcp` binary.
    pub command: String,
    /// Arguments — empty for stdio.
    pub args: Vec<String>,
    /// Environment variables passed to the spawned MCP process.
    /// `BTreeMap` (not `HashMap`) so JSON/TOML writes are deterministic
    /// across runs — without this, idempotency checks would fail on
    /// HashMap iteration order alone.
    pub env: BTreeMap<String, String>,
    /// Optional usage hint surfaced by some CLIs (vibe shows it as a
    /// tool description suffix).
    pub prompt: Option<String>,
}

impl McpServerSpec {
    /// Build the canonical spec. `home` resolves `MAKAKOO_HOME`/`HARVEY_HOME`
    /// env values; `mcp_binary` is the path to `makakoo-mcp` resolved by
    /// [`resolve_mcp_binary`]. If the caller passes None, fall back to the
    /// bare command name so the host CLI can use its own PATH instead of a
    /// stale hardcoded cargo path.
    pub fn default_harvey(home: &Path, mcp_binary: Option<&Path>) -> Self {
        let command = mcp_binary
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| mcp_binary_filename().to_string());
        let home_str = home.to_string_lossy().to_string();
        let mut env = BTreeMap::new();
        env.insert("MAKAKOO_HOME".to_string(), home_str.clone());
        env.insert("HARVEY_HOME".to_string(), home_str.clone());
        // No PYTHONPATH — the Rust MCP binary is self-contained; Python
        // subprocesses it shells into (knowledge_ingest → ingest.py) are
        // themselves self-contained. Library imports like `from core.*`
        // are resolved by `build_skill_env` inserting lib-hte + lib-harvey-core
        // /src/ paths at skill-dispatch time, not here.
        //
        // v0.1-public state: harvey-os/ is archived. Any PYTHONPATH
        // entry pointing at it would just be noise — Python silently
        // ignores non-existent sys.path entries, but stale config is
        // still a footgun for future debugging.
        Self {
            name: "harvey".to_string(),
            command,
            args: Vec::new(),
            env,
            prompt: Some(
                "Harvey/Makakoo native MCP — 41 tools incl. \
                 harvey_describe_image / harvey_describe_audio / \
                 harvey_describe_video for any media URL or local file. \
                 ALWAYS call these for YouTube / screenshots / voice notes \
                 instead of falling back to web_search."
                    .to_string(),
            ),
        }
    }
}

/// Outcome of a single sync operation against one CLI target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Target had no harvey entry; we added one.
    Added,
    /// Target had a stale harvey entry; we replaced it.
    Updated,
    /// Target already had the correct entry; no write performed.
    Unchanged,
    /// Dry-run: would have written this diff (no file touched).
    WouldChange { kind: ChangeKind },
    /// Target's parent dir doesn't exist (CLI not installed); skipped.
    Skipped { reason: String },
    /// Hard error; target left untouched.
    Error { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Add,
    Update,
}

/// Aggregate report of running mcp sync across every target.
#[derive(Debug, Default)]
pub struct McpSyncReport {
    pub results: Vec<(McpTarget, SyncOutcome)>,
    pub dry_run: bool,
    pub verify_only: bool,
}

impl McpSyncReport {
    pub fn added(&self) -> usize {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, SyncOutcome::Added))
            .count()
    }
    pub fn updated(&self) -> usize {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, SyncOutcome::Updated))
            .count()
    }
    pub fn unchanged(&self) -> usize {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, SyncOutcome::Unchanged))
            .count()
    }
    pub fn would_change(&self) -> usize {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, SyncOutcome::WouldChange { .. }))
            .count()
    }
    pub fn skipped(&self) -> usize {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, SyncOutcome::Skipped { .. }))
            .count()
    }
    pub fn errors(&self) -> usize {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, SyncOutcome::Error { .. }))
            .count()
    }
    pub fn human_summary(&self) -> String {
        let mut out = String::new();
        let header = if self.verify_only {
            "makakoo infect --verify (mcp)"
        } else if self.dry_run {
            "makakoo infect --mcp --dry-run"
        } else {
            "makakoo infect --mcp"
        };
        out.push_str(&format!("{header} — {} target(s)\n", self.results.len()));
        for (target, outcome) in &self.results {
            let tag = match outcome {
                SyncOutcome::Added => "added",
                SyncOutcome::Updated => "updated",
                SyncOutcome::Unchanged => "unchanged",
                SyncOutcome::WouldChange { kind } => match kind {
                    ChangeKind::Add => "would-add",
                    ChangeKind::Update => "would-update",
                },
                SyncOutcome::Skipped { .. } => "skipped",
                SyncOutcome::Error { .. } => "error",
            };
            out.push_str(&format!(
                "  {:<10} {:<14} {}\n",
                target.short_name(),
                tag,
                target
                    .config_path_for_home(&dirs::home_dir().unwrap_or_default())
                    .display(),
            ));
            if let SyncOutcome::Error { message } = outcome {
                out.push_str(&format!("    ! {message}\n"));
            }
            if let SyncOutcome::Skipped { reason } = outcome {
                out.push_str(&format!("    : {reason}\n"));
            }
        }
        out
    }
}

/// Run mcp sync across every target known to the infect system.
///
/// `cli_home` is OS-level $HOME (where CLI dotdirs live).
/// `makakoo_home` is `$MAKAKOO_HOME` (used to populate the spec's env).
/// `dry_run`: when true, no files are written; outcomes are
/// `WouldChange` for any target that would be touched.
/// `targets`: when `Some`, restricts the run to that subset (matched
/// by short name, e.g. `["claude", "vibe"]`).
pub fn sync_all(
    cli_home: &Path,
    makakoo_home: &Path,
    mcp_binary: Option<&Path>,
    dry_run: bool,
    targets: Option<&[String]>,
) -> McpSyncReport {
    let spec = McpServerSpec::default_harvey(makakoo_home, mcp_binary);
    let mut report = McpSyncReport {
        dry_run,
        verify_only: false,
        ..Default::default()
    };
    for target in McpTarget::all() {
        if let Some(filter) = targets {
            if !filter.iter().any(|n| n == target.short_name()) {
                continue;
            }
        }
        let outcome = sync_one(cli_home, makakoo_home, target, &spec, dry_run);
        report.results.push((*target, outcome));
    }
    report
}

/// Sync a single target. Dispatches to the matching adapter based on
/// the target's `format`.
pub fn sync_one(
    home: &Path,
    makakoo_home: &Path,
    target: &McpTarget,
    spec: &McpServerSpec,
    dry_run: bool,
) -> SyncOutcome {
    let path = target.config_path_for_home(home);
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            return SyncOutcome::Skipped {
                reason: format!(
                    "{} not installed (no config dir at {})",
                    target.short_name(),
                    parent.display()
                ),
            };
        }
    }
    match target.format() {
        McpFormat::JsonMcpServers => adapters::json::sync(target, &path, spec, dry_run, false),
        McpFormat::JsonOpencode => adapters::json::sync(target, &path, spec, dry_run, true),
        McpFormat::TomlInlineTable => {
            // Codex doesn't read ~/AGENTS.md from $HOME by default —
            // its walk-up search stops at project markers, so the
            // home-level pointer never reaches it. We patch the
            // canonical bootstrap path directly into ~/.codex/config.toml
            // via `model_instructions_file`, which Codex DOES load on
            // every session. Discovered 2026-04-25 in live test —
            // Codex resolved $MAKAKOO_HOME via printenv and missed
            // every paths/conventions in the bootstrap until this fix.
            let canonical = crate::infect::canonical_bootstrap_path(makakoo_home);
            adapters::codex::sync(&path, spec, dry_run, Some(&canonical))
        }
        McpFormat::TomlArrayOfTables => adapters::vibe::sync(&path, spec, dry_run),
    }
}

/// Resolve the installed `makakoo-mcp` binary path for infected MCP configs.
///
/// Resolution order:
/// 1. `$MAKAKOO_MCP_BIN`, if it points at an existing executable/file.
/// 2. Sibling of the currently running `makakoo` binary (curl/Homebrew/tarball
///    installs put `makakoo` and `makakoo-mcp` in the same directory).
/// 3. `makakoo-mcp` on `$PATH`.
/// 4. `~/.cargo/bin/makakoo-mcp`, but only if it actually exists.
///
/// Returns `None` when no usable path is found. The caller then writes the bare
/// `makakoo-mcp` command instead of poisoning fresh installs with a dead
/// `~/.cargo/bin` path.
pub fn resolve_mcp_binary() -> Option<PathBuf> {
    resolve_mcp_binary_from(
        std::env::var_os("MAKAKOO_MCP_BIN"),
        std::env::current_exe().ok(),
        std::env::var_os("PATH"),
        dirs::home_dir(),
    )
}

fn resolve_mcp_binary_from(
    env_bin: Option<OsString>,
    current_exe: Option<PathBuf>,
    path_var: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = env_bin.and_then(existing_file) {
        return Some(p);
    }
    if let Some(exe) = current_exe {
        if let Some(parent) = exe.parent() {
            if let Some(p) = existing_file(parent.join(mcp_binary_filename())) {
                return Some(p);
            }
        }
    }
    if let Some(path_var) = path_var {
        if let Some(p) = find_binary_on_path(&path_var, mcp_binary_filename()) {
            return Some(p);
        }
    }
    if let Some(home) = home {
        if let Some(p) = existing_file(home.join(".cargo/bin").join(mcp_binary_filename())) {
            return Some(p);
        }
    }
    None
}

fn find_binary_on_path(path_var: &OsString, binary: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        if let Some(p) = existing_file(dir.join(binary)) {
            return Some(p);
        }
        #[cfg(windows)]
        for ext in &["exe", "cmd", "bat"] {
            if let Some(p) = existing_file(dir.join(binary).with_extension(ext)) {
                return Some(p);
            }
        }
    }
    None
}

fn existing_file<P: Into<PathBuf>>(path: P) -> Option<PathBuf> {
    let path = path.into();
    if path.is_file() {
        Some(path.canonicalize().unwrap_or(path))
    } else {
        None
    }
}

#[cfg(windows)]
fn mcp_binary_filename() -> &'static str {
    "makakoo-mcp.exe"
}

#[cfg(not(windows))]
fn mcp_binary_filename() -> &'static str {
    "makakoo-mcp"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[cfg(unix)]
    fn default_harvey_uses_canonical_env() {
        // v0.1-public: canonical env is MAKAKOO_HOME + HARVEY_HOME only.
        // PYTHONPATH is explicitly absent — harvey-os is archived, and
        // library imports are wired by build_skill_env at skill-dispatch
        // time rather than leaking into every MCP child's environment.
        let home = PathBuf::from("/Users/test/MAKAKOO");
        let bin = PathBuf::from("/opt/cargo/bin/makakoo-mcp");
        let spec = McpServerSpec::default_harvey(&home, Some(&bin));
        assert_eq!(spec.name, "harvey");
        assert_eq!(spec.command, "/opt/cargo/bin/makakoo-mcp");
        assert!(spec.args.is_empty());
        assert_eq!(spec.env.get("MAKAKOO_HOME").unwrap(), "/Users/test/MAKAKOO");
        assert_eq!(spec.env.get("HARVEY_HOME").unwrap(), "/Users/test/MAKAKOO");
        assert!(
            !spec.env.contains_key("PYTHONPATH"),
            "PYTHONPATH must not be in canonical env after harvey-os retirement"
        );
        assert!(spec
            .prompt
            .as_deref()
            .unwrap()
            .contains("harvey_describe_video"));
    }

    #[test]
    fn default_harvey_falls_back_to_bare_command_when_unspecified() {
        let home = PathBuf::from("/h/MAKAKOO");
        let spec = McpServerSpec::default_harvey(&home, None);
        assert_eq!(spec.command, mcp_binary_filename());
    }

    #[test]
    fn resolve_mcp_binary_prefers_sibling_of_current_exe() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe = bin_dir.join("makakoo");
        let mcp = bin_dir.join(mcp_binary_filename());
        std::fs::write(&exe, b"kernel").unwrap();
        std::fs::write(&mcp, b"mcp").unwrap();

        let resolved = resolve_mcp_binary_from(
            None,
            Some(exe),
            Some(OsString::new()),
            Some(dir.path().join("home")),
        )
        .unwrap();
        assert_eq!(resolved, mcp.canonicalize().unwrap());
    }

    #[test]
    fn resolve_mcp_binary_uses_path_before_cargo_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path_dir = dir.path().join("path-bin");
        let home_bin = dir.path().join("home/.cargo/bin");
        std::fs::create_dir_all(&path_dir).unwrap();
        std::fs::create_dir_all(&home_bin).unwrap();
        let path_mcp = path_dir.join(mcp_binary_filename());
        let cargo_mcp = home_bin.join(mcp_binary_filename());
        std::fs::write(&path_mcp, b"path").unwrap();
        std::fs::write(&cargo_mcp, b"cargo").unwrap();

        let resolved = resolve_mcp_binary_from(
            None,
            None,
            Some(OsString::from(path_dir.as_os_str())),
            Some(dir.path().join("home")),
        )
        .unwrap();
        assert_eq!(resolved, path_mcp.canonicalize().unwrap());
    }

    #[test]
    fn report_human_summary_lists_every_target() {
        let report = McpSyncReport {
            results: vec![
                (McpTarget::Claude, SyncOutcome::Added),
                (McpTarget::Vibe, SyncOutcome::Unchanged),
            ],
            dry_run: false,
            verify_only: false,
        };
        let txt = report.human_summary();
        assert!(txt.contains("claude"));
        assert!(txt.contains("vibe"));
        assert!(txt.contains("added"));
        assert!(txt.contains("unchanged"));
    }

    #[test]
    fn skipped_when_target_dir_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let report = sync_all(tmp.path(), tmp.path(), None, true, None);
        // Every target should be skipped because tmpdir has no CLI dirs.
        assert!(report.skipped() > 0);
        assert_eq!(report.added(), 0);
    }
}
