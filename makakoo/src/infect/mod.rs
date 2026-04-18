//! Infect — writes the Makakoo bootstrap block into every CLI global slot.
//!
//! The infect system ensures every LLM CLI the user drops into
//! (Claude, Gemini, Codex, OpenCode, Vibe, Cursor, Qwen) loads the same
//! Harvey persona + tool knowledge at session start, so there's no such
//! thing as a "vanilla" session on the user's machine.
//!
//! This is the Rust rewrite of `core/orchestration/infect_global.py` —
//! reads the canonical bootstrap from `$MAKAKOO_HOME/global_bootstrap.md`
//! and writes it into all 7 slots (or more, if dynamic hosts are
//! registered — dynamic registration is tracked for a later wave).

// Public API surface — the individual count helpers and planned_paths
// are exported for CLI output + future audit tooling; allow dead_code
// until those callers land.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

pub mod ext;
pub mod renderer;
pub mod slots;
pub mod writer;

use slots::{CliSlot, BLOCK_VERSION, SLOTS};
use writer::{write_bootstrap_to_slot, SlotStatus, SlotWriteResult};

/// Aggregate result of running infect across every slot.
#[derive(Debug, Default)]
pub struct InfectReport {
    pub results: Vec<SlotWriteResult>,
    pub bootstrap_version: String,
    pub dry_run: bool,
}

impl InfectReport {
    pub fn installed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.status, SlotStatus::Installed))
            .count()
    }
    pub fn updated_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.status, SlotStatus::Updated))
            .count()
    }
    pub fn unchanged_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.status, SlotStatus::Unchanged))
            .count()
    }
    pub fn error_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.status, SlotStatus::Error(_)))
            .count()
    }

    /// Pretty one-liner per slot for CLI output.
    pub fn human_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "makakoo infect — bootstrap v{} ({} slots)\n",
            self.bootstrap_version,
            self.results.len()
        ));
        if self.dry_run {
            out.push_str("[dry-run] no files were modified\n");
        }
        for r in &self.results {
            let tag = match &r.status {
                SlotStatus::Installed => "installed",
                SlotStatus::Updated => "updated",
                SlotStatus::Unchanged => "unchanged",
                SlotStatus::DryRun => "would-write",
                SlotStatus::Error(_) => "error",
            };
            out.push_str(&format!(
                "  {:<12} {:<10} {}\n",
                r.slot_name,
                tag,
                r.path.display()
            ));
            if let SlotStatus::Error(e) = &r.status {
                out.push_str(&format!("    ! {e}\n"));
            }
        }
        out
    }
}

/// Load the canonical bootstrap body. Searches, in order:
///   1. `$MAKAKOO_HOME/global_bootstrap.md`
///   2. `$MAKAKOO_HOME/harvey-os/global_bootstrap.md`  (current layout —
///      harvey-os/ is the Rust submodule root for the user's install)
///   3. `./global_bootstrap.md` relative to the current working directory
///
/// Errors if none exist — the infect system refuses to write a stub
/// bootstrap. That's by design; silently writing the wrong content into
/// every CLI slot would be much worse than a loud failure.
/// Load the bootstrap content. Priority:
///   1. Render from plugin registry (cache → fresh render)
///   2. Fall back to static `global_bootstrap.md` (legacy compat)
pub fn load_bootstrap() -> Result<String> {
    let home = makakoo_core::platform::makakoo_home();

    // Try dynamic rendering from plugin registry.
    let registry = makakoo_core::plugin::PluginRegistry::load_default(&home)
        .unwrap_or_default();
    if !registry.is_empty() {
        match renderer::load_or_render(&registry, &home, None) {
            Ok(body) => return Ok(body),
            Err(e) => {
                tracing::warn!(error = %e, "fragment renderer failed, falling back to static bootstrap");
            }
        }
    }

    // Legacy fallback: static file.
    let candidates = [
        home.join("global_bootstrap.md"),
        home.join("harvey-os/global_bootstrap.md"),
        PathBuf::from("global_bootstrap.md"),
    ];
    for path in &candidates {
        if path.exists() {
            let body = std::fs::read_to_string(path)
                .map_err(|e| anyhow!("failed to read {}: {}", path.display(), e))?;
            return Ok(body.trim_end().to_string() + "\n");
        }
    }

    // Last resort: use the compiled-in base template without fragments.
    Ok(renderer::render(&registry, &home, None)?)
}

/// Run infect across every built-in slot. `global` is reserved for a
/// future `--local` mode that targets per-project `.harvey/context.md`
/// instead; the 2026-04-14 cutover always operates globally.
pub async fn run(_global: bool, dry_run: bool) -> Result<InfectReport> {
    run_with_home(&dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?, dry_run).await
}

/// Same as [`run`] but lets callers (tests, daemons) override the home
/// directory where slots are written. The bootstrap body is still
/// loaded from the real `$MAKAKOO_HOME/global_bootstrap.md`.
pub async fn run_with_home(home: &Path, dry_run: bool) -> Result<InfectReport> {
    let body = load_bootstrap()?;
    run_with_home_and_body(home, &body, dry_run).await
}

/// Fully hermetic variant used by tests — both the home directory and the
/// bootstrap body are supplied by the caller. Never touches the real
/// filesystem outside `home`.
pub async fn run_with_home_and_body(
    home: &Path,
    body: &str,
    dry_run: bool,
) -> Result<InfectReport> {
    let mut report = InfectReport {
        bootstrap_version: BLOCK_VERSION.to_string(),
        dry_run,
        ..Default::default()
    };
    for slot in SLOTS {
        let r = write_bootstrap_to_slot(slot, body, home, dry_run);
        report.results.push(r);
    }

    // Extension hosts (VSCode Copilot/Cline/Continue + JetBrains AI).
    // Targets are resolved from the current machine's filesystem — we
    // only write to a host if its config dir exists, so a user without
    // VSCode or JetBrains isn't surprised by unexpected file creations.
    for target in ext_targets_from(home) {
        // Dry-run + target-doesn't-exist = nothing to do. Writing to a
        // fresh Cline/Continue dir is safe because detect_ext_hosts
        // already told us the parent is present.
        let r = ext::write_ext_host(&target, body, dry_run);
        report.results.push(r);
    }

    Ok(report)
}

/// Resolve extension-host write targets for the current machine. Only
/// hosts whose config dir is present are returned — we don't spawn
/// new VSCode or JetBrains installs.
///
/// Paths mirror `spec/INSTALL_MATRIX.md §3.8-3.9` and `detect.rs`'s
/// `detect_ext_hosts` logic; this function is a sibling consumer of
/// the same table.
fn ext_targets_from(home: &Path) -> Vec<ext::ExtTarget> {
    let mut out = Vec::new();

    // VSCode user dir — Copilot + Cline share this parent.
    let vscode_user = if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/Code/User"))
    } else if cfg!(target_os = "linux") {
        Some(home.join(".config/Code/User"))
    } else {
        std::env::var_os("APPDATA")
            .map(|p| PathBuf::from(p).join("Code/User"))
            .or_else(|| Some(home.join("AppData/Roaming/Code/User")))
    };

    if let Some(vs) = vscode_user {
        if vs.exists() {
            out.push(ext::ExtTarget {
                kind: ext::ExtHostKind::Copilot,
                path: vs.join("copilot-instructions.md"),
            });
            let cline_dir = vs.join("globalStorage/saoudrizwan.claude-dev");
            if cline_dir.exists() {
                out.push(ext::ExtTarget {
                    kind: ext::ExtHostKind::Cline,
                    path: cline_dir.join("CLAUDE.md"),
                });
            }
        }
    }

    // Continue.dev — ~/.continue/config.json, same path on all OSes.
    let continue_dir = home.join(".continue");
    if continue_dir.exists() {
        out.push(ext::ExtTarget {
            kind: ext::ExtHostKind::Continue,
            path: continue_dir.join("config.json"),
        });
    }

    // JetBrains — pick the newest product-version dir and target its
    // AI_Assistant/rules.md. Multi-IDE coverage is a later slice.
    let jb_root = if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/JetBrains"))
    } else if cfg!(target_os = "linux") {
        Some(home.join(".config/JetBrains"))
    } else {
        std::env::var_os("APPDATA")
            .map(|p| PathBuf::from(p).join("JetBrains"))
            .or_else(|| Some(home.join("AppData/Roaming/JetBrains")))
    };
    if let Some(root) = jb_root {
        if root.is_dir() {
            let mut product_dirs: Vec<PathBuf> = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&root) {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_uppercase())
                        .unwrap_or(false)
                        && name.chars().any(|c| c.is_ascii_digit())
                    {
                        product_dirs.push(entry.path());
                    }
                }
            }
            product_dirs.sort();
            if let Some(latest) = product_dirs.into_iter().next_back() {
                out.push(ext::ExtTarget {
                    kind: ext::ExtHostKind::JetBrains,
                    path: latest.join("AI_Assistant/rules.md"),
                });
            }
        }
    }

    out
}

/// Paths that would be written for the given home. Used by `--dry-run`
/// pretty-printing without invoking the writer.
pub fn planned_paths(home: &Path) -> Vec<(&'static str, PathBuf)> {
    SLOTS
        .iter()
        .map(|s: &CliSlot| (s.name, s.absolute(home)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TEST_BODY: &str = "# Makakoo OS — Global Bootstrap\n\nYou are Harvey.\n";

    #[tokio::test]
    async fn run_with_fake_home_installs_all_seven() {
        let tmp = TempDir::new().unwrap();
        let report = run_with_home_and_body(tmp.path(), TEST_BODY, false)
            .await
            .unwrap();
        assert_eq!(report.results.len(), 7);
        assert_eq!(report.installed_count(), 7);
        assert_eq!(report.error_count(), 0);
        // Verify each slot exists on disk.
        for slot in SLOTS {
            let p = slot.absolute(tmp.path());
            assert!(p.exists(), "slot {} should exist at {}", slot.name, p.display());
        }
    }

    #[tokio::test]
    async fn dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let report = run_with_home_and_body(tmp.path(), TEST_BODY, true)
            .await
            .unwrap();
        assert_eq!(report.results.len(), 7);
        for r in &report.results {
            assert!(matches!(r.status, SlotStatus::DryRun));
            assert!(!r.path.exists());
        }
    }

    #[tokio::test]
    async fn second_run_is_unchanged() {
        let tmp = TempDir::new().unwrap();
        run_with_home_and_body(tmp.path(), TEST_BODY, false)
            .await
            .unwrap();
        let report = run_with_home_and_body(tmp.path(), TEST_BODY, false)
            .await
            .unwrap();
        assert_eq!(report.unchanged_count(), 7);
        assert_eq!(report.installed_count(), 0);
    }

    #[tokio::test]
    async fn upgrades_old_version_to_v9() {
        let tmp = TempDir::new().unwrap();
        // Seed claude slot with a v7 block and some surrounding content.
        let claude_path = tmp.path().join(".claude/CLAUDE.md");
        std::fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        std::fs::write(
            &claude_path,
            "# My own notes\n\n<!-- harvey:infect-global START v7 -->\nold body\n<!-- harvey:infect-global END -->\n\nAfter block.\n",
        )
        .unwrap();

        let report = run_with_home_and_body(tmp.path(), TEST_BODY, false)
            .await
            .unwrap();
        // Claude got updated, the other 6 got installed.
        assert_eq!(report.updated_count(), 1);
        assert_eq!(report.installed_count(), 6);

        let content = std::fs::read_to_string(&claude_path).unwrap();
        assert!(content.contains("# My own notes"));
        assert!(content.contains("After block."));
        assert!(content.contains("You are Harvey."));
        assert!(content.contains("v9"));
        assert!(!content.contains("old body"));
    }

    #[test]
    fn planned_paths_lists_seven_absolute() {
        let tmp = TempDir::new().unwrap();
        let planned = planned_paths(tmp.path());
        assert_eq!(planned.len(), 7);
        for (_, p) in &planned {
            assert!(p.starts_with(tmp.path()));
        }
    }

    #[test]
    fn load_bootstrap_errors_when_missing() {
        // Temporarily point MAKAKOO_HOME at an empty dir. Takes the
        // crate-wide ENV_MUTEX so we don't race another test also
        // setting MAKAKOO_HOME (see context::tests::ENV_MUTEX).
        let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_var("MAKAKOO_HOME", tmp.path());
        let r = load_bootstrap();
        std::env::remove_var("MAKAKOO_HOME");
        // With the compiled-in base template, bootstrap never fails —
        // it falls through to the embedded template without fragments.
        assert!(r.is_ok());
        assert!(r.unwrap().contains("You are Harvey"));
    }
}
