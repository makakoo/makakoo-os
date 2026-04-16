//! `makakoo migrate` — prepare an existing `$MAKAKOO_HOME` for kernel use.
//!
//! Spec: `spec/SPRINT-MAKAKOO-OS-MASTER.md §7 Phase H`. Sebastian's
//! install predates the Rust kernel's filesystem contract — his
//! `~/MAKAKOO/` has `data/` and `agents/` and `harvey-os/` but no
//! `plugins/`, `state/`, `run/`, or `logs/` dirs. The new kernel needs
//! those to operate.
//!
//! **H/1 scope (this module):** non-destructive directory scaffolding.
//! Create any missing kernel dir under `$MAKAKOO_HOME` + write a
//! `config/migration.json` marker with timestamp. Print an audit
//! showing what already exists, what was created, and what the kernel
//! will NOT touch (Brain, agents, harvey-os, data, ad-hoc state dirs
//! left by Python agents).
//!
//! **Deferred to later H slices:**
//! - Full plugin migration (skills/agents → plugin-manifest form)
//! - `distros/sebastian.toml` that reproduces the full install
//! - State-dir compat symlinks (each plugin migration decides its own
//!   policy in the slice that moves it)
//! - Submodule retirement via `git subtree merge`
//!
//! The core guarantee: re-running `makakoo migrate` is always safe.
//! Never overwrites existing files. Never deletes anything. Never
//! touches user content.

use std::path::{Path, PathBuf};

use chrono::Utc;
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};

use crate::context::CliContext;
use crate::output;

/// Canonical kernel dirs that H/1 ensures exist.
const KERNEL_DIRS: &[&str] = &[
    "plugins",
    "state",
    "run",
    "run/plugins",
    "logs",
    "config",
];

/// Ad-hoc directories we know Sebastian's Python agents currently
/// populate under `data/`. These are NOT migrated by H/1 — listing
/// them here is purely diagnostic so the audit output shows what
/// exists vs. what a future slice will re-home.
const KNOWN_AD_HOC_STATE_DIRS: &[&str] = &[
    "data/arbitrage-agent",
    "data/canary",
    "data/career-manager",
    "data/autoimprover",
    "data/cognitive",
    "data/data-predictor",
    "data/chat",
    "data/auto-memory",
    "data/hackernews",
    "data/nursery.json",
    "data/buddy.json",
    "data/superbrain.db",
    "data/Brain",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationMarker {
    pub migrated_at: chrono::DateTime<Utc>,
    pub schema_version: u32,
    pub created_dirs: Vec<String>,
    pub notes: Vec<String>,
}

pub async fn run(ctx: &CliContext, dry_run: bool) -> anyhow::Result<i32> {
    let home = ctx.home();
    output::print_info(format!(
        "makakoo migrate — target: {}",
        home.display()
    ));
    if dry_run {
        println!("{}", "[dry-run] no changes will be made".yellow());
    }
    println!();

    let audit = audit_current_state(home);
    print_audit(&audit);

    println!();
    print_plan(&audit, dry_run);

    if dry_run {
        println!();
        output::print_info("--dry-run: no changes made.");
        return Ok(0);
    }

    let mut created: Vec<String> = Vec::new();
    for dir in &audit.missing_kernel_dirs {
        let path = home.join(dir);
        match std::fs::create_dir_all(&path) {
            Ok(_) => {
                created.push(dir.clone());
                println!("  {} {}", "created".green(), path.display());
            }
            Err(e) => {
                output::print_error(format!(
                    "failed to create {}: {e}",
                    path.display()
                ));
                return Ok(1);
            }
        }
    }

    let marker = MigrationMarker {
        migrated_at: Utc::now(),
        schema_version: 1,
        created_dirs: created.clone(),
        notes: vec![
            "H/1 scaffolding only — kernel dirs ensured, no plugin migration".into(),
            "Pre-existing data/, agents/, harvey-os/ left untouched".into(),
        ],
    };
    let marker_path = home.join("config/migration.json");
    let rendered = serde_json::to_string_pretty(&marker)?;
    if let Err(e) = std::fs::write(&marker_path, rendered + "\n") {
        output::print_warn(format!(
            "failed to write migration marker: {e}"
        ));
    }

    println!();
    println!("{}", "migration complete".green().bold());
    if created.is_empty() {
        println!("  no new dirs — this install was already migrated");
    } else {
        println!("  created {} kernel dir(s)", created.len());
    }
    println!("  marker: {}", marker_path.display());

    println!();
    println!(
        "{}",
        "next steps:".cyan().bold()
    );
    println!("  1. makakoo distro install core   # materialize the 4 plugins-core manifests");
    println!("  2. makakoo install               # daemon + infect CLI hosts");
    println!();
    output::print_info(
        "Pre-existing agent state dirs under data/ stay where they are until each \
         plugin gets migrated in its own H/N slice. See spec §7 Phase H.",
    );

    Ok(0)
}

/// Snapshot of `$MAKAKOO_HOME` state: what kernel dirs exist, what's
/// missing, which known ad-hoc state dirs are present.
#[derive(Debug, Clone)]
pub struct HomeAudit {
    pub home: PathBuf,
    pub home_exists: bool,
    pub present_kernel_dirs: Vec<&'static str>,
    pub missing_kernel_dirs: Vec<String>,
    pub known_ad_hoc_present: Vec<&'static str>,
    pub brain_journal_count: usize,
    pub brain_pages_count: usize,
    pub superbrain_present: bool,
    pub existing_plugins_count: usize,
    pub existing_marker: Option<MigrationMarker>,
}

fn audit_current_state(home: &Path) -> HomeAudit {
    let home_exists = home.is_dir();

    let mut present_kernel_dirs: Vec<&'static str> = Vec::new();
    let mut missing_kernel_dirs: Vec<String> = Vec::new();
    for dir in KERNEL_DIRS {
        if home.join(dir).is_dir() {
            present_kernel_dirs.push(dir);
        } else {
            missing_kernel_dirs.push((*dir).to_string());
        }
    }

    let known_ad_hoc_present: Vec<&'static str> = KNOWN_AD_HOC_STATE_DIRS
        .iter()
        .copied()
        .filter(|d| home.join(d).exists())
        .collect();

    let brain_journal_count = count_files(&home.join("data/Brain/journals"), "md");
    let brain_pages_count = count_files(&home.join("data/Brain/pages"), "md");
    let superbrain_present = home.join("data/superbrain.db").exists();

    let existing_plugins_count = if home.join("plugins").is_dir() {
        std::fs::read_dir(home.join("plugins"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter(|e| {
                        !e.file_name()
                            .to_string_lossy()
                            .starts_with('.')
                    })
                    .count()
            })
            .unwrap_or(0)
    } else {
        0
    };

    let marker_path = home.join("config/migration.json");
    let existing_marker = if marker_path.is_file() {
        std::fs::read_to_string(&marker_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    } else {
        None
    };

    HomeAudit {
        home: home.to_path_buf(),
        home_exists,
        present_kernel_dirs,
        missing_kernel_dirs,
        known_ad_hoc_present,
        brain_journal_count,
        brain_pages_count,
        superbrain_present,
        existing_plugins_count,
        existing_marker,
    }
}

fn count_files(dir: &Path, ext: &str) -> usize {
    if !dir.is_dir() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension().and_then(|x| x.to_str()) == Some(ext)
                })
                .count()
        })
        .unwrap_or(0)
}

fn print_audit(a: &HomeAudit) {
    println!("{}", "current state".cyan().bold());
    if !a.home_exists {
        output::print_warn(format!("{} does not exist", a.home.display()));
        return;
    }

    println!(
        "  Brain:         {} journal(s), {} page(s){}",
        a.brain_journal_count,
        a.brain_pages_count,
        if a.superbrain_present {
            ", superbrain.db ✓"
        } else {
            ", superbrain.db ✗"
        }
    );

    if a.existing_plugins_count > 0 {
        println!(
            "  plugins dir:   {} registered",
            a.existing_plugins_count
        );
    } else {
        println!("  plugins dir:   (empty or missing)");
    }

    if !a.known_ad_hoc_present.is_empty() {
        println!(
            "  ad-hoc state:  {} known dir(s) under data/ (untouched)",
            a.known_ad_hoc_present.len()
        );
    }

    println!("\n  kernel dirs:");
    for d in KERNEL_DIRS {
        let status = if a.present_kernel_dirs.contains(d) {
            "✓".green().to_string()
        } else {
            "✗ missing".red().to_string()
        };
        println!("    {status} {}", a.home.join(d).display());
    }

    if let Some(ref m) = a.existing_marker {
        println!(
            "\n  {}",
            format!(
                "previously migrated at {} (schema v{}, {} dirs created then)",
                m.migrated_at.to_rfc3339(),
                m.schema_version,
                m.created_dirs.len()
            )
            .dark_grey()
        );
    }
}

fn print_plan(a: &HomeAudit, dry_run: bool) {
    let header = if dry_run {
        "migration plan (dry-run)".yellow().bold()
    } else {
        "migration plan".cyan().bold()
    };
    println!("{header}");
    if a.missing_kernel_dirs.is_empty() {
        println!("  {}", "all kernel dirs present — nothing to create".dark_grey());
        return;
    }
    println!("  will create {} kernel dir(s):", a.missing_kernel_dirs.len());
    for d in &a.missing_kernel_dirs {
        println!("    - {}/{}", a.home.display(), d);
    }
    println!(
        "\n  will {}:",
        "NOT touch".red().bold()
    );
    println!("    - data/Brain/         (journals + pages)");
    println!("    - data/superbrain.db  (FTS + vector index)");
    println!("    - data/<plugin>/      (ad-hoc Python agent state)");
    println!("    - agents/             (submodules, retired in later H slice)");
    println!("    - harvey-os/          (Python runtime, retired in later H slice)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn audit_empty_home_shows_all_kernel_dirs_missing() {
        let tmp = TempDir::new().unwrap();
        let a = audit_current_state(tmp.path());
        assert!(a.home_exists);
        assert_eq!(a.missing_kernel_dirs.len(), KERNEL_DIRS.len());
        assert!(a.present_kernel_dirs.is_empty());
        assert_eq!(a.brain_journal_count, 0);
        assert!(!a.superbrain_present);
    }

    #[test]
    fn audit_full_home_shows_all_kernel_dirs_present() {
        let tmp = TempDir::new().unwrap();
        for d in KERNEL_DIRS {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        let a = audit_current_state(tmp.path());
        assert!(a.missing_kernel_dirs.is_empty());
        assert_eq!(a.present_kernel_dirs.len(), KERNEL_DIRS.len());
    }

    #[test]
    fn audit_detects_brain_journals() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("data/Brain/journals")).unwrap();
        std::fs::write(
            tmp.path().join("data/Brain/journals/2026_04_16.md"),
            "- hi",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("data/Brain/journals/2026_04_15.md"),
            "- hi",
        )
        .unwrap();
        let a = audit_current_state(tmp.path());
        assert_eq!(a.brain_journal_count, 2);
    }

    #[test]
    fn audit_detects_ad_hoc_dirs() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("data/arbitrage-agent")).unwrap();
        std::fs::create_dir_all(tmp.path().join("data/canary")).unwrap();
        let a = audit_current_state(tmp.path());
        assert!(a.known_ad_hoc_present.contains(&"data/arbitrage-agent"));
        assert!(a.known_ad_hoc_present.contains(&"data/canary"));
    }

    #[test]
    fn audit_detects_existing_plugins() {
        let tmp = TempDir::new().unwrap();
        for p in ["alpha", "beta"] {
            let d = tmp.path().join("plugins").join(p);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("plugin.toml"), "[plugin]\nname = \"x\"").unwrap();
        }
        // Dotfile dir should be ignored.
        std::fs::create_dir_all(tmp.path().join("plugins/.stage")).unwrap();
        let a = audit_current_state(tmp.path());
        assert_eq!(a.existing_plugins_count, 2);
    }

    #[test]
    fn audit_loads_prior_migration_marker() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("config")).unwrap();
        let marker = MigrationMarker {
            migrated_at: Utc::now(),
            schema_version: 1,
            created_dirs: vec!["plugins".into()],
            notes: vec!["test".into()],
        };
        std::fs::write(
            tmp.path().join("config/migration.json"),
            serde_json::to_string_pretty(&marker).unwrap(),
        )
        .unwrap();
        let a = audit_current_state(tmp.path());
        assert!(a.existing_marker.is_some());
        let m = a.existing_marker.unwrap();
        assert_eq!(m.schema_version, 1);
    }
}
