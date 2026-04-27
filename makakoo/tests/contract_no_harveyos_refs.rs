//! Contract test: no new `harvey-os/` refs may land in the active source tree.
//!
//! Post-retirement sweep (2026-04-20, v0.2 Phase A.0) removed 160+ stale
//! references to the archived `harvey-os/` directory. This test guards
//! against reintroduction — anything matching the literal string `harvey-os`
//! must sit in the allowlist below.
//!
//! The allowlist covers three legitimate categories:
//!   1. Retirement comments explaining the historical context.
//!   2. Synthetic brand strings (`gym@harvey-os.local` commit trailers,
//!      wikilink taxonomy tokens carried forward from the Brain).
//!   3. Test fixtures that assert on the literal string.
//!
//! Anything else is a regression. Fix the offending file rather than
//! expanding the allowlist.

use std::fs;
use std::path::{Path, PathBuf};

const NEEDLE: &str = "harvey-os";

/// Directories scanned. Keep this list narrow — docs and sprint notes
/// legitimately mention the old tree by name when talking about history.
const SCAN_ROOTS: &[&str] = &[
    "plugins-core",
    "makakoo/src",
    "makakoo-core/src",
    "makakoo-mcp/src",
    "makakoo-client/src",
    "makakoo-platform/src",
];

/// File extensions that must be clean.
const EXTS: &[&str] = &["rs", "py", "toml", "plist", "sh", "json"];

/// Files (or path substrings) allowed to mention `harvey-os` verbatim.
/// Each entry is a substring match against the absolute path.
///
/// Three legitimate categories populate this list:
///   (a) Retirement-era documentation comments in live code.
///   (b) Migration / detection tooling whose whole job is to recognise
///       and rewrite the retired layout (infect/*, commands/migrate.rs,
///       skill_runner regression guards).
///   (c) Synthetic brand strings (`gym@harvey-os.local` commit trailers,
///       wikilink taxonomy tokens carried forward from the Brain) and
///       test fixtures that assert on the literal string.
const ALLOWLIST: &[&str] = &[
    // Retirement-era comments and docstrings live inside these files and
    // describe the historical context. Replace is lossy; keep them.
    "plugins-core/lib-harvey-core/src/core/paths.py",
    "plugins-core/lib-harvey-core/src/core/dispatcher/dispatcher.py",
    "plugins-core/lib-harvey-core/src/core/registry/skill_registry.py",
    "plugins-core/lib-harvey-core/src/core/registry/skill_indexer.py",
    "plugins-core/lib-harvey-core/src/core/gym/approval.py",
    "plugins-core/lib-harvey-core/src/core/gym/cli.py",
    "plugins-core/lib-harvey-core/src/core/gym/flag.py", // SAFE_WIKILINKS taxonomy
    "plugins-core/lib-harvey-core/src/core/orchestration/infect_global.py",
    "plugins-core/skill-dev-skill-manager/src/skill_manager.py",
    "plugins-core/agent-harveychat/src/agent.py",
    "plugins-core/agent-meta-harness-agent/src/run_skill_evaluation.py",
    "plugins-core/mascot-gym/src/gym/approval.py",
    "plugins-core/mascot-gym/src/gym/cli.py",
    "plugins-core/mascot-gym/src/gym/flag.py",
    "plugins-core/skill-meta-memory-retrieval/src/test_memory_retrieval.py",
    "plugins-core/skill-meta-canary/src/rubric.py",
    // Descriptive SKILL.md / AGENT.md / README fragments — Phase A.0 is
    // scoped to code. Docs sweep happens in Phase C / Phase E.
    "SKILL.md",
    "AGENT.md",
    "README.md",
    "ACKNOWLEDGEMENTS.md",
    "AUTO_MEMORY.md",
    "RUBRIC_V0.2_PLAN.md",
    // Sprint docs — this test itself is scaffolded for a queued sprint that
    // mentions the archive extensively.
    "development/",
    // Archive pointers (historical read-only material)
    "/.makakoo/archive/",

    // === Category (b) — migration / detection tooling ===
    // These files exist precisely to recognize and rewrite the retired
    // harvey-os tree. Their job requires keeping the literal string.
    "makakoo/src/infect/",               // every infect adapter has comments about the old layout
    "makakoo/src/commands/migrate.rs",   // the migration CLI
    "makakoo/src/commands/skill.rs",     // still references the legacy path in one comment
    "makakoo/src/skill_runner.rs",       // regression guards assert PYTHONPATH doesn't contain "harvey-os"
    "makakoo/src/cli.rs",                // CLI help text references retirement
    "makakoo-core/src/chat/store.rs",    // doc comment pointing at original Python file
    "makakoo-core/src/chat/router.rs",   // same
    "makakoo-core/src/sancho/handlers.rs", // same
    "makakoo-core/src/memory.rs",        // same
    "makakoo-core/src/event_bus.rs",     // same
    // launchd plist docstring explaining retirement
    "plugins-core/lib-harvey-core/src/core/sancho/sancho.plist",
    // lib-harvey-core's own plugin.toml describes the transition
    "plugins-core/lib-harvey-core/plugin.toml",
];

fn walk(dir: &Path, hits: &mut Vec<(PathBuf, u32)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()).map_or(false, |n| {
                n == "target" || n == "node_modules" || n.starts_with('.')
            }) {
                continue;
            }
            walk(&p, hits);
        } else if p.is_file() {
            let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !EXTS.contains(&ext) {
                continue;
            }
            let Ok(content) = fs::read_to_string(&p) else {
                continue;
            };
            let count = content.matches(NEEDLE).count() as u32;
            if count > 0 {
                hits.push((p, count));
            }
        }
    }
}

fn is_allowlisted(path: &Path) -> bool {
    let s = path.to_string_lossy();
    ALLOWLIST.iter().any(|allow| s.contains(allow))
}

#[test]
fn no_new_harveyos_refs_outside_allowlist() {
    let repo_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("makakoo/ sits inside the workspace root")
        .to_path_buf();

    let mut hits: Vec<(PathBuf, u32)> = Vec::new();
    for root in SCAN_ROOTS {
        walk(&repo_root.join(root), &mut hits);
    }

    let violations: Vec<_> = hits
        .iter()
        .filter(|(p, _)| !is_allowlisted(p))
        .collect();

    if !violations.is_empty() {
        let mut msg = String::from(
            "\n\n❌ harvey-os refs found outside the allowlist. \
             Fix the offending files or — if the ref is truly historical — \
             extend ALLOWLIST in makakoo/tests/contract_no_harveyos_refs.rs\n\n",
        );
        for (p, n) in &violations {
            msg.push_str(&format!(
                "  {} — {} hit(s)\n",
                p.strip_prefix(&repo_root).unwrap_or(p).display(),
                n,
            ));
        }
        panic!("{msg}");
    }
}
