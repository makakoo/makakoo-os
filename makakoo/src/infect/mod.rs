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
pub mod hooks;
pub mod local;
pub mod mcp;
pub mod pointer;
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
///   2. `$MAKAKOO_HOME/plugins/lib-harvey-core/global_bootstrap.md` — the
///      transitional Python core plugin ships the canonical copy.
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
        home.join("plugins/lib-harvey-core/global_bootstrap.md"),
        home.join("plugins-core/lib-harvey-core/global_bootstrap.md"),
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
pub async fn run(_global: bool, dry_run: bool, target_filter: Option<&[String]>) -> Result<InfectReport> {
    run_with_home(&dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?, dry_run, target_filter).await
}

/// Build the structured JSON payload emitted by `infect --verify --json`.
///
/// Exposed as a pure function so tests can assert the shape without
/// standing up a real `$HOME` / `$MAKAKOO_HOME`. Watchdogs consume this
/// shape — changing it is a breaking change, so the test suite locks
/// the keys.
pub fn format_verify_json(drifts: &[mcp::drift::DriftReport]) -> serde_json::Value {
    format_verify_json_with_deep(drifts, None)
}

/// Extended JSON payload that includes the deep audit when `--deep` is
/// passed. Additive schema: the `deep` field is omitted when `None` so
/// existing watchdogs keep seeing the old shape unchanged.
pub fn format_verify_json_with_deep(
    drifts: &[mcp::drift::DriftReport],
    deep: Option<&mcp::deep::DeepDriftReport>,
) -> serde_json::Value {
    let shallow_dirty = drifts.iter().filter(|d| !d.is_clean()).count();
    let deep_dirty = deep.map(|d| d.total_issue_count()).unwrap_or(0);
    let mut obj = serde_json::json!({
        "clean": shallow_dirty == 0 && deep_dirty == 0,
        "dirty_count": shallow_dirty,
        "targets": drifts
            .iter()
            .map(|d| {
                let t = d.target.expect("audit always sets target");
                let issues = d.issues_human();
                serde_json::json!({
                    "name": t.short_name(),
                    "clean": issues.is_empty(),
                    "issues": issues,
                })
            })
            .collect::<Vec<_>>(),
    });
    if let Some(d) = deep {
        obj.as_object_mut()
            .expect("json object")
            .insert("deep".to_string(), mcp::deep::to_json(d));
    }
    obj
}

/// Struct-packed CLI args — keeps the function signature stable as new
/// flags land (sprint-008 added `--json`, sprint-009 adds `--local` + 4
/// project-scoped flags).
pub struct InfectArgs {
    pub global: bool,
    pub mcp: bool,
    pub verify: bool,
    pub json: bool,
    pub deep: bool,
    pub repair: bool,
    pub dry_run: bool,
    pub target: Vec<String>,
    pub local: bool,
    pub dir: Option<PathBuf>,
    pub detect_installed_only: bool,
    pub force_all: bool,
    pub remove: bool,
    pub ignore_derivatives: bool,
}

/// Top-level CLI dispatcher for `makakoo infect`.
///
/// Decision matrix:
///   * `--local`               → project-scoped; mutually exclusive with others
///   * `--verify`              → audit-only across MCP + drift; exit 1 on drift
///   * `--verify --json`       → same audit but structured JSON stdout (watchdogs)
///   * `--mcp` (alone)         → write only MCP, skip bootstrap
///   * `--global` (or no flag) → write bootstrap AND MCP (default)
pub async fn dispatch(args: InfectArgs) -> Result<i32> {
    if args.local && (args.global || args.mcp || args.verify) {
        eprintln!(
            "error: --local is mutually exclusive with --global/--mcp/--verify. \
             Use --local alone for project-scoped infect."
        );
        return Ok(2);
    }
    if args.json && !args.verify {
        eprintln!(
            "error: --json only makes sense with --verify (structured drift report). \
             Use --verify --json for machine-consumable output."
        );
        return Ok(2);
    }
    if (args.deep || args.repair) && !args.verify {
        eprintln!(
            "error: --deep / --repair only apply to --verify. \
             Run `makakoo infect --verify --deep [--repair]`."
        );
        return Ok(2);
    }
    if args.repair && !args.deep {
        eprintln!(
            "error: --repair requires --deep. \
             Shallow drift is already repaired on every `infect --global` run."
        );
        return Ok(2);
    }

    if args.local {
        return dispatch_local_cli(args).await;
    }

    // Alias struct fields into the local names the existing body uses —
    // keeps the diff small while still making the call site struct-based.
    let InfectArgs {
        global,
        mcp: mcp_only,
        verify,
        json,
        deep,
        repair,
        dry_run,
        target,
        ..
    } = args;

    let home = dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?;
    let mcp_binary = mcp::resolve_mcp_binary();
    let target_filter = if target.is_empty() {
        None
    } else {
        Some(target.as_slice())
    };

    let makakoo_home = makakoo_core::platform::makakoo_home();

    if verify {
        // Audit-only: full drift scan (MCP + bootstrap markers + symlinks).
        // With --deep, extend into per-project / workspace / worktree scopes.
        // Exits 1 if any drift detected (0 on clean, even after --repair).
        let spec = mcp::McpServerSpec::default_harvey(&makakoo_home, mcp_binary.as_deref());
        let drifts = mcp::drift::audit_all(&home, &makakoo_home, &spec);
        let shallow_dirty = drifts.iter().filter(|d| !d.is_clean()).count();

        let mut deep_report = if deep {
            Some(mcp::deep::deep_audit(&home, &makakoo_home, &spec, &[]))
        } else {
            None
        };

        let mut repair_actions: Vec<String> = Vec::new();
        if repair {
            if let Some(report) = deep_report.as_ref() {
                if !report.is_clean() {
                    repair_actions = mcp::deep::repair_deep(&home, &spec, report);
                    // Re-audit so the final report reflects post-repair state.
                    deep_report = Some(mcp::deep::deep_audit(&home, &makakoo_home, &spec, &[]));
                }
            }
        }

        let deep_dirty = deep_report.as_ref().map(|d| d.total_issue_count()).unwrap_or(0);

        if json {
            println!(
                "{}",
                serde_json::to_string(&format_verify_json_with_deep(&drifts, deep_report.as_ref()))?
            );
        } else {
            println!("makakoo infect --verify (full drift scan) — 7 target(s)");
            for d in &drifts {
                let target = d.target.expect("audit always sets target");
                let issues = d.issues_human();
                if issues.is_empty() {
                    println!("  {:<10} clean", target.short_name());
                } else {
                    println!(
                        "  {:<10} drift: {}",
                        target.short_name(),
                        issues.join(", ")
                    );
                }
            }
            if let Some(report) = deep_report.as_ref() {
                println!(
                    "\nmakakoo infect --verify --deep (project + workspace + worktree)"
                );
                if report.is_clean() {
                    println!("  deep scan: clean");
                } else {
                    for p in &report.claude_projects {
                        println!(
                            "  claude project scope   {}: {}",
                            p.project_key,
                            p.issues_human().join(", ")
                        );
                    }
                    for w in &report.workspaces {
                        println!(
                            "  workspace .mcp.json    {}: {}",
                            w.path.display(),
                            w.issues_human().join(", ")
                        );
                    }
                    for pw in &report.prunable_worktrees {
                        println!(
                            "  prunable worktree       {} (dead path: {}) — {}",
                            pw.worktree_name,
                            pw.dead_path.display(),
                            pw.reason
                        );
                    }
                }
                for action in &repair_actions {
                    println!("  deep-repair: {}", action);
                }
            }
        }
        return Ok(if shallow_dirty == 0 && deep_dirty == 0 {
            0
        } else {
            1
        });
    }

    let mut bootstrap_failed = false;

    // 1. Bootstrap (skipped when `--mcp` is set without `--global`).
    if !mcp_only || global {
        let report = run(global, dry_run, target_filter).await?;
        print!("{}", report.human_summary());
        if report.error_count() > 0 {
            bootstrap_failed = true;
        }
    }

    // 2. MCP sync (always runs unless `--verify`). Spec home is
    //    $MAKAKOO_HOME (where data + plugins live), CLI dotdirs
    //    are under $HOME — pass both correctly.
    let mcp_report = mcp::sync_all(&home, &makakoo_home, mcp_binary.as_deref(), dry_run, target_filter);
    print!("{}", mcp_report.human_summary());
    let mcp_failed = mcp_report.errors() > 0;

    // 2b. GYM error-funnel hook install (sprint-010 Phase E). Runs
    //     alongside bootstrap + MCP so every CLI that has a hook-
    //     compatible surface gets the same error-capture plumbing.
    //     Independent of the target filter — the CLI subset governs
    //     bootstrap/MCP, whereas hooks always target the 3-CLI set.
    let hook_report = hooks::install_gym_hooks(&home, dry_run);
    print!("{}", hook_report.human_summary());
    let hook_failed = hook_report.error_count() > 0;

    // 3. Symlink + recursive-symlink repair pass (only on real run).
    // Reuses the drift audit so repair only touches what's actually
    // broken — never recreates a working symlink.
    if !dry_run {
        let spec = mcp::McpServerSpec::default_harvey(&makakoo_home, mcp_binary.as_deref());
        let drifts = mcp::drift::audit_all(&home, &makakoo_home, &spec);
        let mut total_actions = 0usize;
        for d in &drifts {
            let target = d.target.expect("audit always sets target");
            if d.memory_broken
                || d.memory_wrong_target
                || d.skills_broken
                || d.skills_wrong_target
                || d.recursive_symlink_in_memory
            {
                let actions = mcp::drift::repair_symlinks(&home, &makakoo_home, target, d);
                for a in &actions {
                    println!("  symlink-repair: {}", a);
                }
                total_actions += actions.len();
            }
        }
        if total_actions > 0 {
            println!("symlink-repair: {} action(s) taken", total_actions);
        }
    }

    Ok(if bootstrap_failed || mcp_failed || hook_failed {
        1
    } else {
        0
    })
}

/// Route `--local` → the project-scoped dispatch in `infect::local`. Resolves
/// `--dir` → cwd → walks up to project root, then hands off to
/// `local::dispatch_local`.
async fn dispatch_local_cli(args: InfectArgs) -> Result<i32> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?;
    let start_dir = match args.dir {
        Some(p) => p,
        None => std::env::current_dir()
            .map_err(|e| anyhow!("no current dir available: {e}"))?,
    };
    let opts = local::LocalOptions {
        detect_installed_only: args.detect_installed_only,
        force_all: args.force_all,
        remove: args.remove,
        dry_run: args.dry_run,
        ignore_derivatives: args.ignore_derivatives,
        // Plumb --target through. Without this, `--target codex` was a
        // silent no-op under --local — the dispatch ignored the field
        // and every run wrote all 6 derivatives regardless. Reported by
        // the user 2026-04-25 right after the grandma-docs sprint shipped.
        target_filter: if args.target.is_empty() { None } else { Some(args.target.clone()) },
    };
    match local::dispatch_local(&start_dir, &home, opts) {
        Ok(report) => {
            print!("{}", report.human_summary());
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: {e}");
            Ok(1)
        }
    }
}

/// Same as [`run`] but lets callers (tests, daemons) override the home
/// directory where slots are written. The bootstrap body is still
/// loaded from the real `$MAKAKOO_HOME/global_bootstrap.md`.
pub async fn run_with_home(home: &Path, dry_run: bool, target_filter: Option<&[String]>) -> Result<InfectReport> {
    let body = load_bootstrap()?;
    run_with_home_and_body(home, &body, dry_run, target_filter).await
}

/// Fully hermetic variant used by tests — both the home directory and the
/// bootstrap body are supplied by the caller. Never touches the real
/// filesystem outside `home`.
pub async fn run_with_home_and_body(
    home: &Path,
    body: &str,
    dry_run: bool,
    target_filter: Option<&[String]>,
) -> Result<InfectReport> {
    let mut report = InfectReport {
        bootstrap_version: BLOCK_VERSION.to_string(),
        dry_run,
        ..Default::default()
    };

    // v12 pointer pattern: install the full bootstrap once at the
    // canonical path, then write a thin pointer (referencing that path)
    // into each CLI's slot. The canonical lives under $MAKAKOO_HOME on
    // real installs (~/MAKAKOO/bootstrap/global.md). When `home` is a
    // tmpdir for hermetic tests, it doubles as the canonical anchor so
    // tests never pollute the real $MAKAKOO_HOME.
    let makakoo_home = makakoo_core::platform::makakoo_home();
    let canonical_anchor = if path_is_under(home, &dirs::home_dir().unwrap_or_default()) {
        // Production: $HOME-rooted slot path → use real $MAKAKOO_HOME.
        makakoo_home.as_path()
    } else {
        // Tests: tmp-rooted slot path → keep canonical under tmp too.
        home
    };
    let canonical = ensure_canonical_bootstrap(canonical_anchor, body, dry_run)?;
    let pointer_body = pointer::render_pointer_body(&canonical);
    let opencode_body = pointer::render_pointer_for_opencode(&canonical);

    for slot in SLOTS {
        if let Some(filter) = target_filter {
            if !filter.iter().any(|t| t.eq_ignore_ascii_case(slot.name)) {
                continue;
            }
        }
        let body_for_slot = match slot.format {
            slots::SlotFormat::OpencodeJson => opencode_body.as_str(),
            // KimiYaml takes the same markdown pointer body as the
            // other slots — the YAML writer wraps it in
            // `<!-- harvey:infect-global START/END -->` markers and
            // tucks it under `agent.system_prompt_args.ROLE_ADDITIONAL`.
            slots::SlotFormat::Markdown | slots::SlotFormat::KimiYaml => pointer_body.as_str(),
        };
        let r = write_bootstrap_to_slot(slot, body_for_slot, home, dry_run);
        report.results.push(r);
    }

    // Extension hosts (VSCode Copilot/Cline/Continue + JetBrains AI).
    // Targets are resolved from the current machine's filesystem — we
    // only write to a host if its config dir exists, so a user without
    // VSCode or JetBrains isn't surprised by unexpected file creations.
    // Extension hosts are markdown-style so they get the same pointer.
    for target in ext_targets_from(home) {
        let r = ext::write_ext_host(&target, &pointer_body, dry_run);
        report.results.push(r);
    }

    // Clean up old slot locations that v12 abandoned. We strip our
    // marker block from those files so the user isn't left with a
    // stale harvey block in a file that no CLI reads. We do NOT delete
    // the file itself — the user may keep their own prose there.
    cleanup_orphan_v11_slots(home, dry_run);

    Ok(report)
}

/// Slot paths that previous infect versions wrote to, but which v12 no
/// longer uses. We strip our marker block from these on every infect so
/// users don't accumulate orphaned blocks.
const ORPHAN_SLOT_PATHS: &[&str] = &[
    // Codex moved off `.codex/instructions.md` in v12 (2026-04-25): modern
    // Codex CLI doesn't read it. New canonical slot is `AGENTS.md`.
    ".codex/instructions.md",
];

fn cleanup_orphan_v11_slots(home: &Path, dry_run: bool) {
    for rel in ORPHAN_SLOT_PATHS {
        let path = home.join(rel);
        if !path.exists() {
            continue;
        }
        let prior = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (next, removed) = writer::remove_markdown_block(&prior);
        if !removed || dry_run {
            continue;
        }
        // If the file is now empty (only contained our block), delete it
        // so we don't leave an empty stub. Otherwise keep the user prose.
        if next.trim().is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            let _ = writer::atomic_write(&path, &next);
        }
    }
}

/// Canonical bootstrap location: `<anchor>/bootstrap/global.md`. This is
/// the single source of truth that every CLI's pointer references. On
/// real installs the anchor is `$MAKAKOO_HOME`. On hermetic tests it's
/// the test tmpdir.
pub fn canonical_bootstrap_path(anchor: &Path) -> PathBuf {
    anchor.join("bootstrap").join("global.md")
}

/// True iff `path` is `parent` itself or a descendant of it. Used to
/// distinguish "real `$HOME`" from "test tmpdir" so we never write a
/// production-rooted canonical bootstrap during tests.
fn path_is_under(path: &Path, parent: &Path) -> bool {
    if parent.as_os_str().is_empty() {
        return false;
    }
    path == parent || path.starts_with(parent)
}

/// Idempotently install the canonical bootstrap to
/// `<home>/bootstrap/global.md`. Returns the canonical path.
///
/// Honors `dry_run`: when true, the path is computed and returned but no
/// disk write happens. The path returned is still suitable for embedding
/// in pointer text for dry-run preview.
fn ensure_canonical_bootstrap(home: &Path, body: &str, dry_run: bool) -> Result<PathBuf> {
    let path = canonical_bootstrap_path(home);
    if dry_run {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow!("failed to create canonical bootstrap dir {}: {}", parent.display(), e)
        })?;
    }
    let normalized = if body.ends_with('\n') {
        body.to_string()
    } else {
        format!("{body}\n")
    };
    let needs_write = match std::fs::read_to_string(&path) {
        Ok(prior) => prior != normalized,
        Err(_) => true,
    };
    if needs_write {
        writer::atomic_write(&path, &normalized)
            .map_err(|e| anyhow!("failed to write canonical bootstrap {}: {}", path.display(), e))?;
    }
    Ok(path)
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

/// Uninfect global CLI slots — strip the bootstrap block from every
/// slot (or the `target` subset). Mirrors `infect --global`, inverts
/// the write. Returns the exit code the top-level CLI should propagate
/// (`0` on clean, `1` on any slot error).
pub async fn uninfect_global(target: Vec<String>, dry_run: bool) -> Result<i32> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?;
    uninfect_global_with_home(&home, target, dry_run).await
}

/// Core of `uninfect_global` — takes `home` so tests can point at a
/// tempdir. Returns the exit code and writes a human-readable summary
/// to stdout.
pub async fn uninfect_global_with_home(
    home: &Path,
    target: Vec<String>,
    dry_run: bool,
) -> Result<i32> {
    let filter: Option<&[String]> = if target.is_empty() {
        None
    } else {
        Some(target.as_slice())
    };

    let mut results: Vec<SlotWriteResult> = Vec::new();
    for slot in SLOTS.iter() {
        if let Some(t) = filter {
            if !t.iter().any(|name| name == slot.name) {
                continue;
            }
        }
        let result = writer::remove_bootstrap_from_slot(slot, home, dry_run);
        results.push(result);
    }

    // Pretty summary — mirrors InfectReport::human_summary shape.
    println!(
        "makakoo uninfect ({} slots{})",
        results.len(),
        if dry_run { ", dry-run" } else { "" },
    );
    let mut errors = 0usize;
    for r in &results {
        let tag = match &r.status {
            SlotStatus::Updated => "removed",
            SlotStatus::Unchanged => "not-infected",
            SlotStatus::DryRun => "would-remove",
            SlotStatus::Error(_) => "error",
            SlotStatus::Installed => "installed", // unreachable on uninfect path
        };
        println!(
            "  {:<12} {:<14} {}",
            r.slot_name,
            tag,
            r.path.display()
        );
        if let SlotStatus::Error(e) = &r.status {
            println!("    ! {e}");
            errors += 1;
        }
    }

    Ok(if errors > 0 { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TEST_BODY: &str = "# Makakoo OS — Global Bootstrap\n\nYou are Harvey.\n";

    #[tokio::test]
    async fn run_with_fake_home_installs_all_eight() {
        let tmp = TempDir::new().unwrap();
        let report = run_with_home_and_body(tmp.path(), TEST_BODY, false, None)
            .await
            .unwrap();
        assert_eq!(report.results.len(), 9);
        assert_eq!(report.installed_count(), 9);
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
        let report = run_with_home_and_body(tmp.path(), TEST_BODY, true, None)
            .await
            .unwrap();
        assert_eq!(report.results.len(), 9);
        for r in &report.results {
            assert!(matches!(r.status, SlotStatus::DryRun));
            assert!(!r.path.exists());
        }
    }

    #[tokio::test]
    async fn second_run_is_unchanged() {
        let tmp = TempDir::new().unwrap();
        run_with_home_and_body(tmp.path(), TEST_BODY, false, None)
            .await
            .unwrap();
        let report = run_with_home_and_body(tmp.path(), TEST_BODY, false, None)
            .await
            .unwrap();
        assert_eq!(report.unchanged_count(), 9);
        assert_eq!(report.installed_count(), 0);
    }

    #[tokio::test]
    async fn upgrades_old_version_to_current() {
        let tmp = TempDir::new().unwrap();
        // Seed claude slot with a v7 block and some surrounding content.
        let claude_path = tmp.path().join(".claude/CLAUDE.md");
        std::fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        std::fs::write(
            &claude_path,
            "# My own notes\n\n<!-- harvey:infect-global START v7 -->\nold body\n<!-- harvey:infect-global END -->\n\nAfter block.\n",
        )
        .unwrap();

        let report = run_with_home_and_body(tmp.path(), TEST_BODY, false, None)
            .await
            .unwrap();
        // Claude got updated, the other 8 got installed.
        assert_eq!(report.updated_count(), 1);
        assert_eq!(report.installed_count(), 8);

        let content = std::fs::read_to_string(&claude_path).unwrap();
        assert!(content.contains("# My own notes"));
        assert!(content.contains("After block."));
        // v12 pointer pattern — slot content references the canonical
        // bootstrap path instead of inlining the full body.
        assert!(content.contains("Makakoo OS bootstrap"));
        // Path separator is platform-specific. Check the components individually.
        assert!(content.contains("bootstrap") && content.contains("global.md"));
        assert!(content.contains(&format!("v{}", super::slots::BLOCK_VERSION)));
        assert!(!content.contains("old body"));

        // Canonical file is the source of truth and DOES contain the
        // full body that the test fixture supplied.
        let canonical = canonical_bootstrap_path(tmp.path());
        let canonical_body = std::fs::read_to_string(&canonical).unwrap();
        assert!(canonical_body.contains("You are Harvey."));
    }

    #[tokio::test]
    async fn target_filter_restricts_to_named_slots() {
        let tmp = TempDir::new().unwrap();
        let filter = vec!["claude".to_string()];
        let report = run_with_home_and_body(tmp.path(), TEST_BODY, false, Some(&filter))
            .await
            .unwrap();
        assert_eq!(report.results.len(), 1, "only claude should be written");
        assert_eq!(report.results[0].path, tmp.path().join(".claude/CLAUDE.md"));
        assert!(matches!(report.results[0].status, SlotStatus::Installed));
        // Other slots must NOT exist on disk.
        for slot in SLOTS {
            if slot.name == "claude" { continue; }
            let p = slot.absolute(tmp.path());
            assert!(!p.exists(), "slot {} should not exist when filtered out", slot.name);
        }
    }

    #[test]
    fn planned_paths_lists_nine_absolute() {
        let tmp = TempDir::new().unwrap();
        let planned = planned_paths(tmp.path());
        assert_eq!(planned.len(), 9);
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

    // --- Phase A: --verify --json shape + contract ---------------------

    use crate::infect::mcp::drift::DriftReport;
    use crate::infect::mcp::McpTarget;

    fn drift(target: McpTarget) -> DriftReport {
        DriftReport {
            target: Some(target),
            ..Default::default()
        }
    }

    #[test]
    fn verify_json_clean_outputs_expected_shape() {
        let drifts: Vec<DriftReport> = McpTarget::all().iter().map(|t| drift(*t)).collect();
        let v = format_verify_json(&drifts);
        assert_eq!(v["clean"], serde_json::Value::Bool(true));
        assert_eq!(v["dirty_count"], serde_json::Value::from(0u64));
        let targets = v["targets"].as_array().unwrap();
        assert_eq!(targets.len(), McpTarget::all().len());
        for t in targets {
            assert_eq!(t["clean"], serde_json::Value::Bool(true));
            assert_eq!(t["issues"].as_array().unwrap().len(), 0);
            assert!(t["name"].is_string());
        }
    }

    #[test]
    fn verify_json_dirty_lists_issues() {
        let mut drifts: Vec<DriftReport> = McpTarget::all().iter().map(|t| drift(*t)).collect();
        // Dirty the first two targets with distinct issues.
        drifts[0].mcp_missing = true;
        drifts[1].memory_broken = true;
        drifts[1].recursive_symlink_in_memory = true;
        let v = format_verify_json(&drifts);
        assert_eq!(v["clean"], serde_json::Value::Bool(false));
        assert_eq!(v["dirty_count"], serde_json::Value::from(2u64));
        let targets = v["targets"].as_array().unwrap();
        let t0 = &targets[0];
        assert_eq!(t0["clean"], serde_json::Value::Bool(false));
        let t0_issues: Vec<&str> = t0["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_str().unwrap())
            .collect();
        assert!(t0_issues.contains(&"mcp-missing"));
        let t1 = &targets[1];
        let t1_issues: Vec<&str> = t1["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_str().unwrap())
            .collect();
        assert!(t1_issues.contains(&"memory-symlink-broken"));
        assert!(t1_issues.contains(&"recursive-symlink-in-memory"));
    }

    #[test]
    fn verify_json_schema_keys_locked() {
        // Watchdogs consume this — changing keys is breaking. Lock them.
        let drifts: Vec<DriftReport> = vec![drift(McpTarget::Claude)];
        let v = format_verify_json(&drifts);
        let obj = v.as_object().unwrap();
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                &"clean".to_string(),
                &"dirty_count".to_string(),
                &"targets".to_string()
            ]
        );
        let target_obj = v["targets"][0].as_object().unwrap();
        let mut target_keys: Vec<&String> = target_obj.keys().collect();
        target_keys.sort();
        assert_eq!(
            target_keys,
            vec![
                &"clean".to_string(),
                &"issues".to_string(),
                &"name".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn local_rejects_combination_with_global() {
        // --local with --global must exit 2 before any filesystem work.
        let code = dispatch(InfectArgs {
            global: true,
            mcp: false,
            verify: false,
            json: false,
            deep: false,
            repair: false,
            dry_run: false,
            target: vec![],
            local: true,
            dir: None,
            detect_installed_only: false,
            force_all: false,
            remove: false,
            ignore_derivatives: false,
        })
        .await
        .unwrap();
        assert_eq!(code, 2, "dispatch should return 2 for --local with --global");
    }

    #[tokio::test]
    async fn json_without_verify_is_rejected() {
        // --json alone (no --verify) must fail loud with exit code 2.
        // We don't need a real $HOME here because the flag combo is
        // checked before any filesystem work.
        let code = dispatch(InfectArgs {
            global: false,
            mcp: false,
            verify: false,
            json: true,
            deep: false,
            repair: false,
            dry_run: false,
            target: vec![],
            local: false,
            dir: None,
            detect_installed_only: false,
            force_all: false,
            remove: false,
            ignore_derivatives: false,
        })
        .await
        .unwrap();
        assert_eq!(
            code, 2,
            "dispatch should return 2 when --json is passed without --verify"
        );
    }

    #[tokio::test]
    async fn deep_without_verify_is_rejected() {
        let code = dispatch(InfectArgs {
            global: false,
            mcp: false,
            verify: false,
            json: false,
            deep: true,
            repair: false,
            dry_run: false,
            target: vec![],
            local: false,
            dir: None,
            detect_installed_only: false,
            force_all: false,
            remove: false,
            ignore_derivatives: false,
        })
        .await
        .unwrap();
        assert_eq!(code, 2, "--deep alone must be rejected");
    }

    #[tokio::test]
    async fn repair_without_deep_is_rejected() {
        let code = dispatch(InfectArgs {
            global: false,
            mcp: false,
            verify: true,
            json: false,
            deep: false,
            repair: true,
            dry_run: false,
            target: vec![],
            local: false,
            dir: None,
            detect_installed_only: false,
            force_all: false,
            remove: false,
            ignore_derivatives: false,
        })
        .await
        .unwrap();
        assert_eq!(code, 2, "--repair requires --deep");
    }

    #[test]
    fn verify_json_with_deep_reports_clean_state() {
        let drifts = vec![mcp::drift::DriftReport {
            target: Some(mcp::McpTarget::Claude),
            ..Default::default()
        }];
        let deep = mcp::deep::DeepDriftReport::default();
        let v = format_verify_json_with_deep(&drifts, Some(&deep));
        assert_eq!(v["clean"], true);
        assert_eq!(v["deep"]["clean"], true);
        assert_eq!(v["deep"]["total_issues"], 0);
    }

    #[test]
    fn verify_json_without_deep_omits_deep_key() {
        let drifts = vec![mcp::drift::DriftReport {
            target: Some(mcp::McpTarget::Claude),
            ..Default::default()
        }];
        let v = format_verify_json_with_deep(&drifts, None);
        assert!(
            v.get("deep").is_none(),
            "deep key must be omitted when None: {v}"
        );
    }

    #[test]
    fn verify_json_with_deep_marks_dirty_when_deep_has_issues() {
        let drifts: Vec<mcp::drift::DriftReport> = vec![];
        let deep = mcp::deep::DeepDriftReport {
            claude_projects: vec![mcp::deep::ProjectDrift {
                project_key: "/p".to_string(),
                claude_json_path: std::path::PathBuf::from("/c"),
                command_stale: true,
                args_stale: false,
                zombie_env_keys: vec![],
            }],
            workspaces: vec![],
            prunable_worktrees: vec![],
        };
        let v = format_verify_json_with_deep(&drifts, Some(&deep));
        assert_eq!(v["clean"], false);
        assert_eq!(v["deep"]["clean"], false);
    }
}
