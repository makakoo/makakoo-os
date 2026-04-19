//! GYM error-funnel hook installer — cross-CLI.
//!
//! Sprint-010 bug 4: `harvey-gym-error.js` was installed only under
//! `~/.claude/hooks/`. OpenCode and Gemini share the same JS-style Stop
//! hook convention but had no copy, so errors from those CLIs never
//! landed in the GYM funnel at `data/errors/YYYY-MM-DD/tool.jsonl`.
//!
//! This module is the single source of truth for where the hook ships
//! per CLI. Codex, Qwen, Cursor, and Vibe use different conventions
//! (extension-based or not yet specified) and are *documented gaps*
//! per the Phase E plan — file a follow-up sprint when any of them
//! ships a JS-compatible hook surface.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// The canonical hook content — embedded at compile time so `makakoo
/// infect --global` is self-contained and doesn't need `$MAKAKOO_HOME`
/// reachable at install time.
pub const GYM_HOOK_SOURCE: &str = include_str!("../../assets/hooks/harvey-gym-error.js");

/// Basename of the hook file on every supported CLI.
pub const GYM_HOOK_FILENAME: &str = "harvey-gym-error.js";

/// One CLI that installs a JS-style Stop hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSlot {
    /// Short CLI name (`claude`, `gemini`, `opencode`).
    pub name: &'static str,
    /// `$HOME`-relative hook dir. Must end in `/hooks` by convention.
    pub hook_dir: &'static str,
}

impl HookSlot {
    pub fn absolute_dir(&self, home: &Path) -> PathBuf {
        home.join(self.hook_dir)
    }
    pub fn absolute_file(&self, home: &Path) -> PathBuf {
        self.absolute_dir(home).join(GYM_HOOK_FILENAME)
    }
}

/// CLIs with confirmed JS-compatible Stop hook surface. Adding a new
/// CLI here makes `makakoo infect --global` install the hook into it.
/// Codex, Qwen, Cursor, Vibe intentionally absent — file a follow-up
/// when they ship a compatible hook runner.
pub const HOOK_SLOTS: &[HookSlot] = &[
    HookSlot {
        name: "claude",
        hook_dir: ".claude/hooks",
    },
    HookSlot {
        name: "gemini",
        hook_dir: ".gemini/hooks",
    },
    HookSlot {
        name: "opencode",
        hook_dir: ".config/opencode/hooks",
    },
];

/// What happened for one slot on one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookInstallStatus {
    /// Written fresh (no prior file).
    Installed,
    /// Existing file content differed — overwritten.
    Updated,
    /// Existing file content matched — no-op.
    Unchanged,
    /// Slot skipped because the CLI's parent dir (`~/.<cli>`) doesn't
    /// exist — not considered an error; user simply doesn't have that
    /// CLI installed.
    Skipped,
    /// Slot was not visited because of dry-run mode.
    DryRun,
    /// Any I/O failure. Non-fatal — other slots still attempted.
    Error(String),
}

#[derive(Debug, Clone)]
pub struct HookInstallResult {
    pub slot_name: &'static str,
    pub path: PathBuf,
    pub status: HookInstallStatus,
}

#[derive(Debug, Clone, Default)]
pub struct HookInstallReport {
    pub results: Vec<HookInstallResult>,
    pub dry_run: bool,
}

impl HookInstallReport {
    pub fn installed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.status, HookInstallStatus::Installed))
            .count()
    }
    pub fn updated_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.status, HookInstallStatus::Updated))
            .count()
    }
    pub fn error_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(&r.status, HookInstallStatus::Error(_)))
            .count()
    }
    pub fn human_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "gym-hook infect — {} slot(s){}\n",
            self.results.len(),
            if self.dry_run { " [dry-run]" } else { "" }
        ));
        for r in &self.results {
            let tag = match &r.status {
                HookInstallStatus::Installed => "installed",
                HookInstallStatus::Updated => "updated",
                HookInstallStatus::Unchanged => "unchanged",
                HookInstallStatus::Skipped => "skipped",
                HookInstallStatus::DryRun => "would-write",
                HookInstallStatus::Error(_) => "error",
            };
            out.push_str(&format!(
                "  {:<10} {:<12} {}\n",
                r.slot_name,
                tag,
                r.path.display()
            ));
            if let HookInstallStatus::Error(e) = &r.status {
                out.push_str(&format!("    ! {e}\n"));
            }
        }
        out
    }
}

/// Install the GYM hook into every `HOOK_SLOTS` slot whose parent CLI
/// dir (`~/.<cli>/` or `~/.config/<cli>/`) exists. Idempotent.
pub fn install_gym_hooks(home: &Path, dry_run: bool) -> HookInstallReport {
    let mut report = HookInstallReport {
        dry_run,
        ..Default::default()
    };
    for slot in HOOK_SLOTS {
        let result = install_one(slot, home, dry_run);
        report.results.push(result);
    }
    report
}

fn install_one(slot: &HookSlot, home: &Path, dry_run: bool) -> HookInstallResult {
    let target_dir = slot.absolute_dir(home);
    let target_file = slot.absolute_file(home);

    // The hook dir's *parent* tells us whether the user has that CLI
    // installed. `~/.claude/` is the signal, not `~/.claude/hooks/`
    // (the hooks subdir only exists after something writes a hook).
    let cli_root = target_dir
        .parent()
        .unwrap_or(&target_dir);
    if !cli_root.exists() {
        return HookInstallResult {
            slot_name: slot.name,
            path: target_file,
            status: HookInstallStatus::Skipped,
        };
    }

    if dry_run {
        return HookInstallResult {
            slot_name: slot.name,
            path: target_file,
            status: HookInstallStatus::DryRun,
        };
    }

    // Ensure hook dir exists.
    if let Err(e) = std::fs::create_dir_all(&target_dir) {
        return HookInstallResult {
            slot_name: slot.name,
            path: target_file,
            status: HookInstallStatus::Error(format!("create hook dir: {e}")),
        };
    }

    // Compare existing content if present.
    if target_file.exists() {
        match std::fs::read_to_string(&target_file) {
            Ok(existing) if existing == GYM_HOOK_SOURCE => {
                return HookInstallResult {
                    slot_name: slot.name,
                    path: target_file,
                    status: HookInstallStatus::Unchanged,
                };
            }
            Ok(_) => {
                if let Err(e) = std::fs::write(&target_file, GYM_HOOK_SOURCE) {
                    return HookInstallResult {
                        slot_name: slot.name,
                        path: target_file,
                        status: HookInstallStatus::Error(format!("overwrite: {e}")),
                    };
                }
                return HookInstallResult {
                    slot_name: slot.name,
                    path: target_file,
                    status: HookInstallStatus::Updated,
                };
            }
            Err(e) => {
                return HookInstallResult {
                    slot_name: slot.name,
                    path: target_file,
                    status: HookInstallStatus::Error(format!("read existing: {e}")),
                };
            }
        }
    }

    match write_hook(&target_file) {
        Ok(()) => HookInstallResult {
            slot_name: slot.name,
            path: target_file,
            status: HookInstallStatus::Installed,
        },
        Err(e) => HookInstallResult {
            slot_name: slot.name,
            path: target_file,
            status: HookInstallStatus::Error(format!("write: {e}")),
        },
    }
}

fn write_hook(path: &Path) -> Result<()> {
    std::fs::write(path, GYM_HOOK_SOURCE)?;
    // Set executable bit so the CLI can shebang-launch without `node`
    // being explicitly in the spawn. On non-unix this is a no-op.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Pure drift check — does this CLI have the hook, and if so, is it
/// byte-identical to the canonical content?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDriftState {
    /// CLI not installed (`~/.<cli>/` missing) — drift ignored.
    NotApplicable,
    /// Hook file present and content matches.
    Clean,
    /// Hook file missing even though the CLI is installed.
    Missing,
    /// Hook file present but content diverges from canonical.
    Divergent,
}

pub fn audit_hook(slot: &HookSlot, home: &Path) -> HookDriftState {
    let target_dir = slot.absolute_dir(home);
    let cli_root = target_dir.parent().unwrap_or(&target_dir);
    if !cli_root.exists() {
        return HookDriftState::NotApplicable;
    }
    let target_file = slot.absolute_file(home);
    if !target_file.exists() {
        return HookDriftState::Missing;
    }
    match std::fs::read_to_string(&target_file) {
        Ok(s) if s == GYM_HOOK_SOURCE => HookDriftState::Clean,
        Ok(_) => HookDriftState::Divergent,
        Err(_) => HookDriftState::Divergent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_home_with_cli_dirs(cli_dirs: &[&str]) -> tempfile::TempDir {
        let home = tempdir().unwrap();
        for d in cli_dirs {
            std::fs::create_dir_all(home.path().join(d)).unwrap();
        }
        home
    }

    #[test]
    fn hook_source_is_nonempty() {
        assert!(GYM_HOOK_SOURCE.contains("harvey-gym-error"));
        assert!(GYM_HOOK_SOURCE.starts_with("#!"));
    }

    #[test]
    fn three_default_slots_for_phase_e() {
        let names: Vec<&str> = HOOK_SLOTS.iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["claude", "gemini", "opencode"]);
    }

    #[test]
    fn install_into_all_three_slots_when_all_present() {
        let home = setup_home_with_cli_dirs(&[".claude", ".gemini", ".config/opencode"]);
        let report = install_gym_hooks(home.path(), false);
        assert_eq!(report.installed_count(), 3);
        for slot in HOOK_SLOTS {
            assert!(slot.absolute_file(home.path()).exists());
        }
    }

    #[test]
    fn install_skips_missing_clis() {
        let home = setup_home_with_cli_dirs(&[".gemini"]);
        let report = install_gym_hooks(home.path(), false);
        assert_eq!(report.installed_count(), 1);
        let skipped = report
            .results
            .iter()
            .filter(|r| matches!(r.status, HookInstallStatus::Skipped))
            .count();
        assert_eq!(skipped, 2);
    }

    #[test]
    fn install_is_idempotent_on_matching_content() {
        let home = setup_home_with_cli_dirs(&[".claude"]);
        install_gym_hooks(home.path(), false);
        let second = install_gym_hooks(home.path(), false);
        let unchanged = second
            .results
            .iter()
            .filter(|r| matches!(r.status, HookInstallStatus::Unchanged))
            .count();
        assert_eq!(unchanged, 1);
    }

    #[test]
    fn install_overwrites_divergent_file() {
        let home = setup_home_with_cli_dirs(&[".claude"]);
        let hooks = home.path().join(".claude/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("harvey-gym-error.js"), b"// stale content").unwrap();
        let report = install_gym_hooks(home.path(), false);
        assert_eq!(report.updated_count(), 1);
        let out = std::fs::read_to_string(hooks.join("harvey-gym-error.js")).unwrap();
        assert!(out.contains("harvey-gym-error"));
    }

    #[test]
    fn dry_run_writes_no_files() {
        let home = setup_home_with_cli_dirs(&[".claude"]);
        let report = install_gym_hooks(home.path(), true);
        assert!(report.dry_run);
        assert_eq!(
            report.results.iter().filter(|r| matches!(r.status, HookInstallStatus::DryRun)).count(),
            1
        );
        assert!(!home.path().join(".claude/hooks/harvey-gym-error.js").exists());
    }

    #[test]
    fn audit_reports_missing_when_cli_present_but_no_hook() {
        let home = setup_home_with_cli_dirs(&[".claude"]);
        let slot = &HOOK_SLOTS[0];
        assert_eq!(audit_hook(slot, home.path()), HookDriftState::Missing);
    }

    #[test]
    fn audit_reports_clean_after_install() {
        let home = setup_home_with_cli_dirs(&[".claude"]);
        install_gym_hooks(home.path(), false);
        let slot = &HOOK_SLOTS[0];
        assert_eq!(audit_hook(slot, home.path()), HookDriftState::Clean);
    }

    #[test]
    fn audit_reports_divergent_on_stale_content() {
        let home = setup_home_with_cli_dirs(&[".claude"]);
        let hooks = home.path().join(".claude/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("harvey-gym-error.js"), b"// drift").unwrap();
        let slot = &HOOK_SLOTS[0];
        assert_eq!(audit_hook(slot, home.path()), HookDriftState::Divergent);
    }

    #[test]
    fn audit_reports_not_applicable_when_cli_missing() {
        let home = tempdir().unwrap();
        let slot = &HOOK_SLOTS[0];
        assert_eq!(audit_hook(slot, home.path()), HookDriftState::NotApplicable);
    }

    #[cfg(unix)]
    #[test]
    fn installed_hook_is_executable() {
        use std::os::unix::fs::PermissionsExt;
        let home = setup_home_with_cli_dirs(&[".claude"]);
        install_gym_hooks(home.path(), false);
        let meta = std::fs::metadata(home.path().join(".claude/hooks/harvey-gym-error.js"))
            .unwrap();
        assert_ne!(meta.permissions().mode() & 0o111, 0, "not executable");
    }
}
