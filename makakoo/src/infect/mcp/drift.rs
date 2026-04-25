//! Drift detection + repair across the broader infect surface.
//!
//! Catches the bug class that bit Sebastian 2026-04-18 and any
//! similar variant: bootstrap missing, MCP entry missing/stale, dead
//! `~/HARVEY/skills-shared` symlink, recursive symlink inside the
//! shared auto-memory dir.
//!
//! Detection is read-only and side-effect-free. Repair is idempotent
//! and only touches the specific drift items the audit found —
//! nothing else.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

use crate::infect::mcp::{McpServerSpec, McpTarget};

/// Per-target drift status. Every field defaults to `false` (clean).
#[derive(Debug, Default, Clone)]
pub struct DriftReport {
    pub target: Option<McpTarget>,
    pub mcp_missing: bool,
    pub mcp_stale_path: bool,
    pub mcp_stale_env: bool,
    pub bootstrap_missing: bool,
    pub memory_broken: bool,
    pub memory_wrong_target: bool,
    pub skills_broken: bool,
    pub skills_wrong_target: bool,
    pub recursive_symlink_in_memory: bool,
    /// Sprint-010: GYM error-funnel hook missing from a CLI that
    /// supports the JS Stop-hook convention (claude, gemini, opencode).
    pub gym_hook_missing: bool,
    /// Sprint-010: GYM hook present but content diverges from canonical.
    pub gym_hook_divergent: bool,
}

impl DriftReport {
    pub fn is_clean(&self) -> bool {
        !self.mcp_missing
            && !self.mcp_stale_path
            && !self.mcp_stale_env
            && !self.bootstrap_missing
            && !self.memory_broken
            && !self.memory_wrong_target
            && !self.skills_broken
            && !self.skills_wrong_target
            && !self.recursive_symlink_in_memory
            && !self.gym_hook_missing
            && !self.gym_hook_divergent
    }

    pub fn issue_count(&self) -> usize {
        [
            self.mcp_missing,
            self.mcp_stale_path,
            self.mcp_stale_env,
            self.bootstrap_missing,
            self.memory_broken,
            self.memory_wrong_target,
            self.skills_broken,
            self.skills_wrong_target,
            self.recursive_symlink_in_memory,
            self.gym_hook_missing,
            self.gym_hook_divergent,
        ]
        .iter()
        .filter(|x| **x)
        .count()
    }

    pub fn issues_human(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.mcp_missing {
            out.push("mcp-missing");
        }
        if self.mcp_stale_path {
            out.push("mcp-stale-command");
        }
        if self.mcp_stale_env {
            out.push("mcp-stale-env");
        }
        if self.bootstrap_missing {
            out.push("bootstrap-missing");
        }
        if self.memory_broken {
            out.push("memory-symlink-broken");
        }
        if self.memory_wrong_target {
            out.push("memory-symlink-wrong-target");
        }
        if self.skills_broken {
            out.push("skills-symlink-broken");
        }
        if self.skills_wrong_target {
            out.push("skills-symlink-wrong-target");
        }
        if self.recursive_symlink_in_memory {
            out.push("recursive-symlink-in-memory");
        }
        if self.gym_hook_missing {
            out.push("gym-hook-missing");
        }
        if self.gym_hook_divergent {
            out.push("gym-hook-divergent");
        }
        out
    }
}

/// Audit one target. `home` is the OS-level $HOME (where CLI dotdirs
/// live), `makakoo_home` is `$MAKAKOO_HOME` (where data/auto-memory and
/// skills-shared live). The two are usually different — the canonical
/// memory dir is at `$MAKAKOO_HOME/data/auto-memory` not
/// `$HOME/data/auto-memory`. Read-only — never modifies anything.
pub fn audit(
    home: &Path,
    makakoo_home: &Path,
    target: McpTarget,
    spec: &McpServerSpec,
) -> DriftReport {
    let mut r = DriftReport {
        target: Some(target),
        ..Default::default()
    };

    // -- Bootstrap presence --------------------------------------------
    let bootstrap = home.join(target.bootstrap_rel_path());
    if !bootstrap.exists() || !contains_bootstrap_marker(&bootstrap) {
        r.bootstrap_missing = true;
    }

    // -- MCP entry -----------------------------------------------------
    let mcp_path = target.config_path_for_home(home);
    if !mcp_path.exists() {
        r.mcp_missing = true;
    } else {
        match check_mcp_entry(home, &target, spec) {
            McpEntryStatus::Missing => r.mcp_missing = true,
            McpEntryStatus::StaleCommand => r.mcp_stale_path = true,
            McpEntryStatus::StaleEnv => r.mcp_stale_env = true,
            McpEntryStatus::Ok => {}
        }
    }

    // -- Memory symlink ------------------------------------------------
    if let Some(rel) = target.memory_rel_path() {
        let mempath = home.join(rel);
        let canonical = makakoo_home.join("data").join("auto-memory");
        match symlink_status(&mempath, &canonical) {
            SymlinkStatus::Missing => {} // intentionally not present
            SymlinkStatus::BrokenOrAbsent => r.memory_broken = true,
            SymlinkStatus::WrongTarget => r.memory_wrong_target = true,
            SymlinkStatus::Ok => {}
        }
        // Recursive child symlink inside the canonical dir.
        let recursive = canonical.join("auto-memory");
        if recursive.is_symlink()
            || recursive.exists()
                && fs::symlink_metadata(&recursive)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
        {
            r.recursive_symlink_in_memory = true;
        }
    }

    // -- Skills symlink (only some CLIs use it) ------------------------
    if let Some(rel) = target.skills_rel_path() {
        let skpath = home.join(rel);
        let canonical = makakoo_home.join("skills-shared");
        match symlink_status(&skpath, &canonical) {
            SymlinkStatus::Missing => {}
            SymlinkStatus::BrokenOrAbsent => r.skills_broken = true,
            SymlinkStatus::WrongTarget => r.skills_wrong_target = true,
            SymlinkStatus::Ok => {}
        }
    }

    // -- GYM error-funnel hook (only CLIs with JS Stop-hook surface) ---
    // Only 3 of 7 targets have a JS hook convention today — the others
    // report `NotApplicable` and leave both flags false (clean).
    if let Some(slot) = gym_hook_slot_for_target(&target) {
        use crate::infect::hooks::{audit_hook, HookDriftState};
        match audit_hook(slot, home) {
            HookDriftState::NotApplicable | HookDriftState::Clean => {}
            HookDriftState::Missing => r.gym_hook_missing = true,
            HookDriftState::Divergent => r.gym_hook_divergent = true,
        }
    }

    r
}

/// Map an `McpTarget` to its `HookSlot` if one exists. Codex, Qwen,
/// Cursor, Vibe return `None` (documented gap — see hooks.rs).
fn gym_hook_slot_for_target(target: &McpTarget) -> Option<&'static crate::infect::hooks::HookSlot> {
    use crate::infect::hooks::HOOK_SLOTS;
    let name = target.short_name();
    HOOK_SLOTS.iter().find(|s| s.name == name)
}

/// Audit every target. See [`audit`] for the home/makakoo_home split.
pub fn audit_all(home: &Path, makakoo_home: &Path, spec: &McpServerSpec) -> Vec<DriftReport> {
    McpTarget::all()
        .iter()
        .map(|t| audit(home, makakoo_home, *t, spec))
        .collect()
}

/// Apply repairs derived from a previous audit. `home` is OS-level $HOME,
/// `makakoo_home` is `$MAKAKOO_HOME`. Returns a list of human-readable
/// repair actions taken. MCP repair is delegated to the regular `sync`
/// path (idempotent) — this function only handles the symlink +
/// recursive-symlink classes the regular adapters can't see.
pub fn repair_symlinks(
    home: &Path,
    makakoo_home: &Path,
    target: McpTarget,
    drift: &DriftReport,
) -> Vec<String> {
    let mut actions = Vec::new();

    // Recursive symlink in shared auto-memory — kill it.
    if drift.recursive_symlink_in_memory {
        let recursive = makakoo_home
            .join("data")
            .join("auto-memory")
            .join("auto-memory");
        if fs::remove_file(&recursive).is_ok() {
            actions.push(format!("removed recursive symlink {}", recursive.display()));
        }
    }

    if drift.memory_broken || drift.memory_wrong_target {
        if let Some(rel) = target.memory_rel_path() {
            let mempath = home.join(rel);
            let canonical = makakoo_home.join("data").join("auto-memory");
            if let Err(e) = recreate_symlink(&mempath, &canonical) {
                actions.push(format!(
                    "FAILED to repair memory symlink {}: {}",
                    mempath.display(),
                    e
                ));
            } else {
                actions.push(format!(
                    "repointed {} → {}",
                    mempath.display(),
                    canonical.display()
                ));
            }
        }
    }

    if drift.skills_broken || drift.skills_wrong_target {
        if let Some(rel) = target.skills_rel_path() {
            let skpath = home.join(rel);
            let canonical = makakoo_home.join("skills-shared");
            if let Err(e) = recreate_symlink(&skpath, &canonical) {
                actions.push(format!(
                    "FAILED to repair skills symlink {}: {}",
                    skpath.display(),
                    e
                ));
            } else {
                actions.push(format!(
                    "repointed {} → {}",
                    skpath.display(),
                    canonical.display()
                ));
            }
        }
    }

    actions
}

// ─────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
enum McpEntryStatus {
    Missing,
    StaleCommand,
    StaleEnv,
    Ok,
}

/// Read the target's MCP config and check whether the harvey entry
/// matches `spec`. Reuses the adapter's sync function in dry-run mode
/// for the source of truth — that way the audit + the repair use the
/// same comparison logic and can never disagree.
fn check_mcp_entry(home: &Path, target: &McpTarget, spec: &McpServerSpec) -> McpEntryStatus {
    use crate::infect::mcp::{ChangeKind, SyncOutcome};
    // Drift uses sync in dry-run mode as the comparison oracle, so
    // pass the same makakoo_home that prod uses. Resolve via the
    // platform helper since drift doesn't have it plumbed in.
    let makakoo_home = makakoo_core::platform::makakoo_home();
    let outcome = crate::infect::mcp::sync_one(home, &makakoo_home, target, spec, true);
    match outcome {
        SyncOutcome::Unchanged => McpEntryStatus::Ok,
        SyncOutcome::WouldChange { kind: ChangeKind::Add } => McpEntryStatus::Missing,
        SyncOutcome::WouldChange { kind: ChangeKind::Update } => McpEntryStatus::StaleCommand,
        // Skipped / errored / Added (not in dry-run) → treat as missing
        // so the report nudges the user to run repair.
        _ => McpEntryStatus::Missing,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SymlinkStatus {
    /// Path does not exist. Caller decides whether that's a problem.
    Missing,
    /// Path exists but is a dangling symlink (target gone).
    BrokenOrAbsent,
    /// Symlink points somewhere other than `canonical`.
    WrongTarget,
    /// Symlink is correct and target exists.
    Ok,
}

fn symlink_status(path: &Path, canonical: &Path) -> SymlinkStatus {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return SymlinkStatus::Missing,
    };
    if meta.file_type().is_symlink() {
        let target = match fs::read_link(path) {
            Ok(t) => t,
            Err(_) => return SymlinkStatus::BrokenOrAbsent,
        };
        let target_resolved = if target.is_absolute() {
            target.clone()
        } else {
            path.parent().unwrap_or(Path::new("/")).join(&target)
        };
        if !target_resolved.exists() {
            return SymlinkStatus::BrokenOrAbsent;
        }
        let normalise = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if normalise(&target_resolved) != normalise(canonical) {
            return SymlinkStatus::WrongTarget;
        }
        SymlinkStatus::Ok
    } else if meta.is_dir() {
        // Real directory where we expected a symlink — likely user-owned
        // content (qwen ships its own skills/, etc.). Don't flag as
        // drift — auto-repair would destroy data. User-managed setups
        // stay user-managed.
        SymlinkStatus::Missing
    } else {
        SymlinkStatus::WrongTarget
    }
}

/// Sibling-aware sym-link recreation. Removes the existing entry
/// (if any), creates parents, makes a fresh link to `canonical`.
fn recreate_symlink(path: &Path, canonical: &Path) -> std::io::Result<()> {
    if path.exists() || path.is_symlink() {
        // remove_file also removes symlinks (broken or otherwise).
        fs::remove_file(path).ok();
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(canonical, path)?;
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(canonical, path)?;
    }
    Ok(())
}

fn contains_bootstrap_marker(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|s| s.contains("harvey:infect-global"))
        .unwrap_or(false)
}

// Drift tests cover Unix symlink behaviour specifically (the MCP
// memory/ + skills/ symlink audit + repair paths only apply where
// the kernel writes real POSIX symlinks). Windows symlink coverage
// belongs to makakoo-platform's Windows adapter tests + the Phase F
// VM smoke, not here.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn spec(home: &Path) -> McpServerSpec {
        let mut env = BTreeMap::new();
        env.insert(
            "MAKAKOO_HOME".to_string(),
            home.to_string_lossy().to_string(),
        );
        env.insert(
            "HARVEY_HOME".to_string(),
            home.to_string_lossy().to_string(),
        );
        env.insert(
            "PYTHONPATH".to_string(),
            home.join("harvey-os").to_string_lossy().to_string(),
        );
        McpServerSpec {
            name: "harvey".to_string(),
            command: "/opt/cargo/bin/makakoo-mcp".to_string(),
            args: vec![],
            env,
            prompt: Some("desc".to_string()),
        }
    }

    #[test]
    fn empty_home_reports_missing_bootstrap_and_mcp() {
        let dir = tempdir().unwrap();
        let r = audit(dir.path(), dir.path(), McpTarget::Vibe, &spec(dir.path()));
        assert!(r.bootstrap_missing);
        assert!(r.mcp_missing);
        assert!(!r.is_clean());
    }

    #[test]
    fn clean_home_after_sync_is_clean_for_mcp() {
        let dir = tempdir().unwrap();
        // Create the parent dir so adapter runs (Skipped otherwise).
        fs::create_dir_all(dir.path().join(".vibe")).unwrap();
        let s = spec(dir.path());
        // Apply real sync via adapter so the file lives at the right path.
        let path = McpTarget::Vibe.config_path_for_home(dir.path());
        let _ = crate::infect::mcp::adapters::vibe::sync(&path, &s, false);

        // Bootstrap also needs a marker to be considered clean.
        let bp = dir.path().join(McpTarget::Vibe.bootstrap_rel_path());
        if let Some(parent) = bp.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&bp, "intro\n<!-- harvey:infect-global START v9 -->\nbody\n<!-- harvey:infect-global END -->\n")
            .unwrap();

        // Memory symlink to canonical.
        let canonical = dir.path().join("data").join("auto-memory");
        fs::create_dir_all(&canonical).unwrap();
        let mempath = dir.path().join(McpTarget::Vibe.memory_rel_path().unwrap());
        std::os::unix::fs::symlink(&canonical, &mempath).unwrap();
        // Skills canonical.
        let skills_canonical = dir.path().join("skills-shared");
        fs::create_dir_all(&skills_canonical).unwrap();
        let skills_link = dir.path().join(McpTarget::Vibe.skills_rel_path().unwrap());
        std::os::unix::fs::symlink(&skills_canonical, &skills_link).unwrap();

        let r = audit(dir.path(), dir.path(), McpTarget::Vibe, &s);
        assert!(
            r.is_clean(),
            "expected clean drift but got {:?}",
            r.issues_human()
        );
    }

    #[test]
    fn detects_dead_skills_symlink() {
        let dir = tempdir().unwrap();
        let dead_target = dir.path().join("does-not-exist");
        let skills_link = dir.path().join(McpTarget::Vibe.skills_rel_path().unwrap());
        fs::create_dir_all(skills_link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&dead_target, &skills_link).unwrap();

        let r = audit(dir.path(), dir.path(), McpTarget::Vibe, &spec(dir.path()));
        assert!(r.skills_broken, "report: {:?}", r.issues_human());
    }

    #[test]
    fn detects_recursive_symlink_in_memory() {
        let dir = tempdir().unwrap();
        let canonical = dir.path().join("data").join("auto-memory");
        fs::create_dir_all(&canonical).unwrap();
        let recursive = canonical.join("auto-memory");
        std::os::unix::fs::symlink(&canonical, &recursive).unwrap();
        // Need memory symlink set up so the audit visits the canonical dir.
        let mempath = dir.path().join(McpTarget::Vibe.memory_rel_path().unwrap());
        fs::create_dir_all(mempath.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&canonical, &mempath).unwrap();

        let r = audit(dir.path(), dir.path(), McpTarget::Vibe, &spec(dir.path()));
        assert!(
            r.recursive_symlink_in_memory,
            "report: {:?}",
            r.issues_human()
        );
    }

    #[test]
    fn repair_recreates_dead_skills_symlink() {
        let dir = tempdir().unwrap();
        let dead = dir.path().join("dead");
        let skills_link = dir.path().join(McpTarget::Vibe.skills_rel_path().unwrap());
        fs::create_dir_all(skills_link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&dead, &skills_link).unwrap();
        // Canonical exists.
        let canonical = dir.path().join("skills-shared");
        fs::create_dir_all(&canonical).unwrap();

        let mut drift = DriftReport::default();
        drift.skills_broken = true;
        let actions = repair_symlinks(dir.path(), dir.path(), McpTarget::Vibe, &drift);
        assert!(actions.iter().any(|a| a.contains("repointed")));

        let r = audit(dir.path(), dir.path(), McpTarget::Vibe, &spec(dir.path()));
        assert!(!r.skills_broken, "report after repair: {:?}", r.issues_human());
    }

    #[test]
    fn repair_removes_recursive_symlink() {
        let dir = tempdir().unwrap();
        let canonical = dir.path().join("data").join("auto-memory");
        fs::create_dir_all(&canonical).unwrap();
        let recursive = canonical.join("auto-memory");
        std::os::unix::fs::symlink(&canonical, &recursive).unwrap();

        let mut drift = DriftReport::default();
        drift.recursive_symlink_in_memory = true;
        let actions = repair_symlinks(dir.path(), dir.path(), McpTarget::Vibe, &drift);
        assert!(actions.iter().any(|a| a.contains("removed recursive")));
        assert!(!recursive.exists());
    }

    #[test]
    fn detects_missing_gym_hook_for_claude() {
        // Claude installed (~/.claude exists), hook absent.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let r = audit(dir.path(), dir.path(), McpTarget::Claude, &spec(dir.path()));
        assert!(
            r.gym_hook_missing,
            "expected gym_hook_missing, got: {:?}",
            r.issues_human()
        );
        assert!(r.issues_human().contains(&"gym-hook-missing"));
    }

    #[test]
    fn gym_hook_clean_after_infect_installs_hook() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude/hooks")).unwrap();
        // Write canonical content.
        std::fs::write(
            dir.path().join(".claude/hooks/harvey-gym-error.js"),
            crate::infect::hooks::GYM_HOOK_SOURCE,
        )
        .unwrap();
        let r = audit(dir.path(), dir.path(), McpTarget::Claude, &spec(dir.path()));
        assert!(!r.gym_hook_missing);
        assert!(!r.gym_hook_divergent);
    }

    #[test]
    fn detects_divergent_gym_hook() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude/hooks")).unwrap();
        std::fs::write(
            dir.path().join(".claude/hooks/harvey-gym-error.js"),
            b"// stale content",
        )
        .unwrap();
        let r = audit(dir.path(), dir.path(), McpTarget::Claude, &spec(dir.path()));
        assert!(r.gym_hook_divergent);
    }

    #[test]
    fn gym_hook_not_audited_for_cli_without_hook_surface() {
        // Codex has no HookSlot — hook fields always false.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".codex")).unwrap();
        let r = audit(dir.path(), dir.path(), McpTarget::Codex, &spec(dir.path()));
        assert!(!r.gym_hook_missing);
        assert!(!r.gym_hook_divergent);
    }

    #[test]
    fn issue_count_matches_human_list() {
        let mut r = DriftReport::default();
        r.mcp_missing = true;
        r.skills_broken = true;
        r.recursive_symlink_in_memory = true;
        assert_eq!(r.issue_count(), 3);
        assert_eq!(r.issues_human().len(), 3);
    }
}
