//! Project-scoped infect — `.harvey/context.md` + per-CLI derivatives.
//!
//! Sibling of the global infect path in `infect/mod.rs`. Same upsert-marker
//! pattern as `writer.rs`, but parameterised for project-local markers and
//! targeting CLI-native per-project files that each CLI already reads on
//! session start (`CLAUDE.md`, `GEMINI.md`, `AGENTS.md`, `QWEN.md`,
//! `.cursor/rules/makakoo.mdc`, `.vibe/context.md`).
//!
//! Canonical source of truth: `<project>/.harvey/context.md` — the user edits
//! this one file; derivatives are regenerated from it. `.harvey/.gitignore`
//! tracks context.md and ignores all derivatives so the repo stores the
//! source, not the artifacts.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use regex::Regex;

use crate::infect::writer::atomic_write;

/// Marker constants for the `harvey:infect-local` block. Parallel to the
/// `BLOCK_START`/`BLOCK_END` pair for `infect-global` in `slots.rs` but
/// isolated here so a global-version bump doesn't ripple into local.
pub const LOCAL_BLOCK_VERSION: &str = "1";
pub const LOCAL_BLOCK_START: &str = "<!-- harvey:infect-local START v1 -->";
pub const LOCAL_BLOCK_END: &str = "<!-- harvey:infect-local END -->";

/// Match any prior-version local block (v1, v2, ...) so future version bumps
/// upgrade in place without manual cleanup.
fn local_block_regex() -> Regex {
    Regex::new(
        r"(?s)\n*<!--\s*harvey:infect-local\s+START\s+v[^\s>]+\s*-->.*?<!--\s*harvey:infect-local\s+END\s*-->\n*",
    )
    .expect("infect-local block regex is valid")
}

/// Match the global block — used only for ordering: when both blocks exist in
/// a derivative file, local must render below global.
fn global_block_regex() -> Regex {
    Regex::new(
        r"(?s)\n*<!--\s*harvey:infect-global\s+START\s+v[^\s>]+\s*-->.*?<!--\s*harvey:infect-global\s+END\s*-->\n*",
    )
    .expect("infect-global block regex is valid")
}

/// Per-derivative outcome. Mirrors `SlotStatus` from the global path but
/// scoped narrowly — we never need the JSON-array variants here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalStatus {
    /// File didn't exist or had no prior local block; we appended.
    Installed,
    /// Existing local block found and replaced with new content.
    Updated,
    /// Identical content already present; no bytes written.
    Unchanged,
    /// `--remove` stripped a prior block; file kept.
    Removed,
    /// `--remove` ran but there was nothing to strip.
    NothingToRemove,
    /// `--detect-installed-only` ruled this target out.
    SkippedNotInstalled,
}

#[derive(Debug, Clone)]
pub struct LocalWrite {
    pub path: PathBuf,
    pub status: LocalStatus,
}

#[derive(Debug, Default)]
pub struct LocalReport {
    pub project_root: PathBuf,
    pub writes: Vec<LocalWrite>,
    pub context_path: PathBuf,
    pub gitignore_created: bool,
    pub context_created: bool,
    pub root_gitignore_status: Option<LocalStatus>,
    pub dry_run: bool,
    pub dry_run_diff: Option<String>,
}

impl LocalReport {
    pub fn human_summary(&self) -> String {
        let prefix = if self.dry_run { "[dry-run] " } else { "" };
        let mut lines = vec![format!(
            "{prefix}makakoo infect --local — project root: {}",
            self.project_root.display()
        )];
        if self.context_created {
            lines.push(format!(
                "  {prefix}created {}",
                self.context_path.display()
            ));
        } else {
            lines.push(format!(
                "  {prefix}using existing {}",
                self.context_path.display()
            ));
        }
        if self.gitignore_created {
            lines.push(format!(
                "  {prefix}created .harvey/.gitignore"
            ));
        }
        if let Some(status) = &self.root_gitignore_status {
            lines.push(format!("  {prefix}root .gitignore {:?}", status));
        }
        for w in &self.writes {
            lines.push(format!(
                "  {prefix}{:<40} {:?}",
                w.path
                    .strip_prefix(&self.project_root)
                    .unwrap_or(&w.path)
                    .display(),
                w.status
            ));
        }
        lines.join("\n") + "\n"
    }
}

/// Walk upward from `start` looking for a directory that contains `.git/`
/// or `.harvey/`. Falls back to `start` itself if neither is found.
pub fn find_project_root(start: &Path) -> PathBuf {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join(".git").exists() || cur.join(".harvey").exists() {
            return cur;
        }
        match cur.parent() {
            Some(p) if p != cur => cur = p.to_path_buf(),
            _ => return start.to_path_buf(),
        }
    }
}

/// The six derivative file targets. `cli_key` is the `~/.<cli_key>/` dotdir
/// we probe for `--detect-installed-only`; the AGENTS.md entry covers two
/// CLIs so it has two probe keys.
#[derive(Debug, Clone, Copy)]
pub struct DerivativeTarget {
    pub relative: &'static str,
    pub probe_dotdirs: &'static [&'static str],
    pub label: &'static str,
}

pub const DERIVATIVE_TARGETS: &[DerivativeTarget] = &[
    DerivativeTarget {
        relative: "CLAUDE.md",
        probe_dotdirs: &[".claude"],
        label: "claude",
    },
    DerivativeTarget {
        relative: "GEMINI.md",
        probe_dotdirs: &[".gemini"],
        label: "gemini",
    },
    DerivativeTarget {
        relative: "AGENTS.md",
        probe_dotdirs: &[".codex", ".config/opencode"],
        label: "codex+opencode",
    },
    DerivativeTarget {
        relative: "QWEN.md",
        probe_dotdirs: &[".qwen"],
        label: "qwen",
    },
    DerivativeTarget {
        relative: ".cursor/rules/makakoo.mdc",
        probe_dotdirs: &[".cursor"],
        label: "cursor",
    },
    DerivativeTarget {
        relative: ".vibe/context.md",
        probe_dotdirs: &[".vibe"],
        label: "vibe",
    },
];

/// Starter template written to `.harvey/context.md` when the file is absent.
const STARTER_TEMPLATE: &str = "# Harvey — project rules

Project-specific rules that layer on top of your global Harvey bootstrap.
Edit this file freely; rerun `makakoo infect --local` to propagate changes
into per-CLI derivatives (`CLAUDE.md`, `GEMINI.md`, etc.).

## Rules

- (add project-specific rules here — e.g. \"this repo uses yarn, not npm\")

## Context

- (add project context here — what this repo is, who it's for, key constraints)
";

/// Marker-bracketed lines that `--ignore-derivatives` upserts into the
/// PROJECT ROOT `.gitignore`. Uses `#`-prefixed markers (gitignore comment
/// syntax) rather than HTML comments — `<!--` would be interpreted as an
/// ignore pattern.
const GITIGNORE_MARKER_START: &str = "# harvey:infect-local START v1";
const GITIGNORE_MARKER_END: &str = "# harvey:infect-local END";
const ROOT_GITIGNORE_BODY: &str = "\
# harvey:infect-local START v1
# Derivatives regenerated from .harvey/context.md by `makakoo infect --local`.
# Re-run `makakoo infect --local` after clone to rebuild these files.
/CLAUDE.md
/GEMINI.md
/AGENTS.md
/QWEN.md
/.cursor/rules/makakoo.mdc
/.vibe/context.md
# harvey:infect-local END
";

/// Regex for upsert/strip on the root `.gitignore` block. Parallel to
/// `local_block_regex()` but matches the `#`-prefixed gitignore syntax.
fn gitignore_block_regex() -> Regex {
    Regex::new(
        r"(?ms)\n*^#\s*harvey:infect-local\s+START\s+v[^\s]+\s*$.*?^#\s*harvey:infect-local\s+END\s*$\n*",
    )
    .expect("gitignore block regex is valid")
}

/// Default `.harvey/.gitignore` content: track context.md only.
const GITIGNORE_BODY: &str = "# Auto-generated by `makakoo infect --local`.
# Track only the canonical source; derivatives regenerate on each clone via
# `makakoo infect --local`.

# Track this
!context.md

# Ignore everything else under .harvey/
*
!.gitignore
";

/// Top-level options for a local-infect run.
#[derive(Debug, Clone, Default)]
pub struct LocalOptions {
    pub detect_installed_only: bool,
    pub force_all: bool, // explicit alias for default behaviour; clarifies CI intent
    pub remove: bool,
    pub dry_run: bool,
    /// Upsert a `harvey:infect-local`-bracketed block into the project
    /// root `.gitignore` listing the six derivative paths. Opt-in —
    /// default leaves the user's `.gitignore` untouched so they can
    /// choose to commit derivatives (collaborators without makakoo see
    /// the project rules natively).
    pub ignore_derivatives: bool,
    /// CLI scope filter (mirrors `--target` for `--global`). Tokens like
    /// `codex`, `claude`, `gemini`, `opencode`, `qwen`, `cursor`, `vibe`.
    /// `None` (default) writes every derivative; `Some(list)` writes only
    /// those whose label matches a token. The `codex+opencode` target
    /// (AGENTS.md) matches either `codex` or `opencode`.
    pub target_filter: Option<Vec<String>>,
}

/// Entry point. `dir` is the user-supplied directory (or cwd); we walk up to
/// find the project root. `home` is `$HOME` for dotdir probing + the safety
/// guard that refuses `project_root == $HOME`.
pub fn dispatch_local(dir: &Path, home: &Path, opts: LocalOptions) -> Result<LocalReport> {
    let project_root = find_project_root(dir);

    // Hard guard — infecting $HOME turns every shell session into a Harvey
    // project session, which is almost never what the user meant.
    if project_root == home {
        bail!(
            "refusing to infect $HOME ({}). Create or cd into a subdirectory first.",
            home.display()
        );
    }

    let harvey_dir = project_root.join(".harvey");
    let context_path = harvey_dir.join("context.md");
    let gitignore_path = harvey_dir.join(".gitignore");

    let mut report = LocalReport {
        project_root: project_root.clone(),
        context_path: context_path.clone(),
        dry_run: opts.dry_run,
        ..Default::default()
    };

    if opts.remove {
        // Strip the root .gitignore block unconditionally — it might have
        // been written by a prior `--ignore-derivatives` run we're now
        // undoing.
        let gi = strip_root_gitignore(&project_root, opts.dry_run)?;
        report.root_gitignore_status = Some(gi);
        return remove_local_blocks(&project_root, opts.dry_run, report);
    }

    // --- Scaffold .harvey/ on first run ----------------------------------
    if !harvey_dir.exists() && !opts.dry_run {
        fs::create_dir_all(&harvey_dir)?;
    }

    let context_body = if context_path.exists() {
        fs::read_to_string(&context_path)?
    } else {
        if !opts.dry_run {
            atomic_write(&context_path, STARTER_TEMPLATE)?;
        }
        report.context_created = true;
        STARTER_TEMPLATE.to_string()
    };

    if !gitignore_path.exists() {
        if !opts.dry_run {
            atomic_write(&gitignore_path, GITIGNORE_BODY)?;
        }
        report.gitignore_created = true;
    }

    // --- Render the local block body from context.md ---------------------
    let local_block = render_local_block(&context_body);

    // --- Write each derivative -------------------------------------------
    let targets = select_targets(&opts, home);
    for (target, included) in targets {
        let path = project_root.join(target.relative);
        if !included {
            report.writes.push(LocalWrite {
                path,
                status: LocalStatus::SkippedNotInstalled,
            });
            continue;
        }

        let prior = if path.exists() {
            fs::read_to_string(&path).unwrap_or_default()
        } else {
            String::new()
        };
        let (next, status) = upsert_local_block(&prior, &local_block);
        if status != LocalStatus::Unchanged && !opts.dry_run {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).ok();
            }
            atomic_write(&path, &next)?;
        }
        report.writes.push(LocalWrite { path, status });
    }

    // Opt-in: upsert the project root `.gitignore` block so the derivative
    // files stop showing as untracked. User-invasive by design — never
    // default-on.
    if opts.ignore_derivatives {
        let gi = upsert_root_gitignore(&project_root, opts.dry_run)?;
        report.root_gitignore_status = Some(gi);
    }

    if opts.dry_run {
        // Defer structured diff until we need it; plain summary is enough
        // for Phase A's dry-run gate.
        report.dry_run_diff = Some(report.human_summary());
    }

    Ok(report)
}

/// Select derivative targets per options and probed dotdirs. Returns each
/// target paired with whether it should be written on this run.
fn select_targets(
    opts: &LocalOptions,
    home: &Path,
) -> Vec<(&'static DerivativeTarget, bool)> {
    // The `codex+opencode` target's label has both names joined by `+`;
    // match-by-substring is intentional so users can pass `--target codex`
    // OR `--target opencode` and hit the same AGENTS.md entry.
    let target_match = |label: &str| -> bool {
        match &opts.target_filter {
            None => true,
            Some(list) if list.is_empty() => true,
            Some(list) => list.iter().any(|tok| label.contains(tok.as_str())),
        }
    };
    DERIVATIVE_TARGETS
        .iter()
        .map(|t| {
            let label_ok = target_match(t.label);
            let installed_ok = if opts.detect_installed_only {
                t.probe_dotdirs
                    .iter()
                    .any(|d| home.join(d).exists())
            } else {
                true
            };
            (t, label_ok && installed_ok)
        })
        .collect()
}

/// Render the fenced local block around `body`.
pub fn render_local_block(body: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        LOCAL_BLOCK_START,
        body.trim_end(),
        LOCAL_BLOCK_END
    )
}

/// Upsert the local block into `text`. When a `harvey:infect-global` block is
/// also present, the local block is positioned *below* the global one so the
/// "later overrides earlier" conflict-resolution semantics align with the
/// intent "local overrides global".
pub fn upsert_local_block(text: &str, new_block: &str) -> (String, LocalStatus) {
    let re = local_block_regex();
    if let Some(m) = re.find(text) {
        // In-place replace.
        let prior = &text[m.start()..m.end()];
        if prior.trim() == new_block.trim() {
            return (text.to_string(), LocalStatus::Unchanged);
        }
        let before = &text[..m.start()];
        let after = &text[m.end()..];
        let mut out = String::with_capacity(text.len() + new_block.len());
        out.push_str(before);
        if !before.ends_with('\n') && !before.is_empty() {
            out.push('\n');
        }
        out.push_str(new_block);
        out.push_str(after);
        return (out, LocalStatus::Updated);
    }

    // No existing local block. If a global block is present, append *after*
    // the last global-block occurrence so local rules land later in the
    // file. Otherwise append at the bottom.
    let g_re = global_block_regex();
    let mut out = text.to_string();
    if let Some(m) = g_re.find_iter(text).last() {
        let before = &text[..m.end()];
        let after = &text[m.end()..];
        out = String::with_capacity(text.len() + new_block.len() + 2);
        out.push_str(before);
        if !before.ends_with("\n\n") {
            if !before.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str(new_block);
        out.push_str(after);
    } else {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(new_block);
    }
    (out, LocalStatus::Installed)
}

/// Upsert the `harvey:infect-local` block into the project root
/// `.gitignore`. Creates the file if absent. Preserves user content outside
/// the marker range. Returns whether a write would be needed.
fn upsert_root_gitignore(project_root: &Path, dry_run: bool) -> Result<LocalStatus> {
    let path = project_root.join(".gitignore");
    let prior = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let re = gitignore_block_regex();
    let (next, status) = if let Some(m) = re.find(&prior) {
        let existing = &prior[m.start()..m.end()];
        if existing.trim() == ROOT_GITIGNORE_BODY.trim() {
            (prior.clone(), LocalStatus::Unchanged)
        } else {
            let before = &prior[..m.start()];
            let after = &prior[m.end()..];
            let mut out = String::with_capacity(prior.len() + ROOT_GITIGNORE_BODY.len());
            out.push_str(before);
            if !before.ends_with('\n') && !before.is_empty() {
                out.push('\n');
            }
            out.push_str(ROOT_GITIGNORE_BODY);
            out.push_str(after);
            (out, LocalStatus::Updated)
        }
    } else {
        let mut out = prior.clone();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(ROOT_GITIGNORE_BODY);
        (out, LocalStatus::Installed)
    };
    if status != LocalStatus::Unchanged && !dry_run {
        crate::infect::writer::atomic_write(&path, &next)?;
    }
    Ok(status)
}

/// Strip the `harvey:infect-local` block from the project root `.gitignore`.
/// Called unconditionally from the remove path — the block might have been
/// added by a prior `--ignore-derivatives` run.
fn strip_root_gitignore(project_root: &Path, dry_run: bool) -> Result<LocalStatus> {
    let path = project_root.join(".gitignore");
    if !path.exists() {
        return Ok(LocalStatus::NothingToRemove);
    }
    let prior = fs::read_to_string(&path)?;
    let re = gitignore_block_regex();
    let stripped = re.replace_all(&prior, "").to_string();
    if stripped == prior {
        return Ok(LocalStatus::NothingToRemove);
    }
    if !dry_run {
        crate::infect::writer::atomic_write(&path, &stripped)?;
    }
    Ok(LocalStatus::Removed)
}

/// `--remove`: strip all harvey:infect-local blocks from every derivative in
/// the project. Leaves `.harvey/context.md` and `.harvey/.gitignore` alone —
/// those are the user's source of truth, not generated artefacts.
fn remove_local_blocks(
    project_root: &Path,
    dry_run: bool,
    mut report: LocalReport,
) -> Result<LocalReport> {
    let re = local_block_regex();
    for target in DERIVATIVE_TARGETS {
        let path = project_root.join(target.relative);
        if !path.exists() {
            continue;
        }
        let prior = fs::read_to_string(&path)
            .map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
        let stripped = re.replace_all(&prior, "").to_string();
        if stripped == prior {
            report.writes.push(LocalWrite {
                path,
                status: LocalStatus::NothingToRemove,
            });
            continue;
        }
        if !dry_run {
            atomic_write(&path, &stripped)?;
        }
        report.writes.push(LocalWrite {
            path,
            status: LocalStatus::Removed,
        });
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_dotdirs(home: &Path, which: &[&str]) {
        for name in which {
            fs::create_dir_all(home.join(name)).unwrap();
        }
    }

    fn all_dotdirs() -> Vec<&'static str> {
        vec![
            ".claude",
            ".gemini",
            ".codex",
            ".config/opencode",
            ".qwen",
            ".cursor",
            ".vibe",
        ]
    }

    // --- find_project_root -----------------------------------------------

    #[test]
    fn find_project_root_walks_up_to_git() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        let nested = root.join("src/components");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(find_project_root(&nested), root);
    }

    #[test]
    fn find_project_root_walks_up_to_harvey() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        let nested = root.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(root.join(".harvey")).unwrap();
        assert_eq!(find_project_root(&nested), root);
    }

    #[test]
    fn find_project_root_falls_back_to_start_when_neither() {
        let tmp = TempDir::new().unwrap();
        let lonely = tmp.path().join("no-markers");
        fs::create_dir_all(&lonely).unwrap();
        assert_eq!(find_project_root(&lonely), lonely);
    }

    // --- dispatch_local defaults -----------------------------------------

    #[test]
    fn fresh_dir_creates_context_and_all_six_derivatives() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        // No dotdirs seeded — default behaviour still writes all 6.
        let r = dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        assert!(r.context_created);
        assert!(r.gitignore_created);
        assert!(proj.join(".harvey/context.md").exists());
        assert!(proj.join(".harvey/.gitignore").exists());
        for t in DERIVATIVE_TARGETS {
            assert!(
                proj.join(t.relative).exists(),
                "expected derivative {} to be written by default",
                t.relative
            );
        }
        // 6 writes; all Installed.
        assert_eq!(r.writes.len(), DERIVATIVE_TARGETS.len());
        for w in &r.writes {
            assert_eq!(w.status, LocalStatus::Installed);
        }
    }

    #[test]
    fn detect_installed_only_narrows_targets() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        seed_dotdirs(&home, &[".claude"]);
        let r = dispatch_local(
            &proj,
            &home,
            LocalOptions {
                detect_installed_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        let written: Vec<&str> = r
            .writes
            .iter()
            .filter(|w| w.status == LocalStatus::Installed)
            .map(|w| w.path.file_name().unwrap().to_str().unwrap())
            .collect();
        let skipped_count = r
            .writes
            .iter()
            .filter(|w| w.status == LocalStatus::SkippedNotInstalled)
            .count();
        assert!(written.contains(&"CLAUDE.md"));
        assert!(!written.contains(&"GEMINI.md"));
        assert!(!written.contains(&"QWEN.md"));
        assert_eq!(skipped_count, DERIVATIVE_TARGETS.len() - 1);
    }

    #[test]
    fn detect_installed_only_picks_up_agents_via_either_cli() {
        // AGENTS.md is shared by codex + opencode. Seeding either dotdir
        // should enable it.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        seed_dotdirs(&home, &[".config/opencode"]);
        let r = dispatch_local(
            &proj,
            &home,
            LocalOptions {
                detect_installed_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(proj.join("AGENTS.md").exists());
    }

    // --- context.md handling ---------------------------------------------

    #[test]
    fn respects_existing_context_file() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(proj.join(".harvey")).unwrap();
        fs::create_dir_all(&home).unwrap();
        let custom = "# custom project rules\n- never use Jenkins\n";
        fs::write(proj.join(".harvey/context.md"), custom).unwrap();
        let r = dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        assert!(!r.context_created);
        // Context file byte-for-byte untouched.
        assert_eq!(
            fs::read_to_string(proj.join(".harvey/context.md")).unwrap(),
            custom
        );
        // Derivatives contain the custom rule.
        let claude = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        assert!(claude.contains("never use Jenkins"));
        assert!(claude.contains(LOCAL_BLOCK_START));
    }

    #[test]
    fn preserves_surrounding_user_content_in_derivative() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        let own = "# my own notes\nshould survive\n";
        fs::write(proj.join("CLAUDE.md"), own).unwrap();
        dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        let after = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        assert!(after.contains("# my own notes"));
        assert!(after.contains("should survive"));
        assert!(after.contains(LOCAL_BLOCK_START));
        assert!(after.contains(LOCAL_BLOCK_END));
    }

    // --- Block ordering + upsert -----------------------------------------

    #[test]
    fn global_block_ordered_before_local_when_both_present() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        let prior = "# Top\n\n<!-- harvey:infect-global START v9 -->\nglobal body\n<!-- harvey:infect-global END -->\n\nUser notes after global.\n";
        fs::write(proj.join("CLAUDE.md"), prior).unwrap();
        dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        let after = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        let g_pos = after.find("harvey:infect-global START").unwrap();
        let l_pos = after.find(LOCAL_BLOCK_START).unwrap();
        assert!(
            g_pos < l_pos,
            "global block must precede local block; got g={g_pos} l={l_pos}"
        );
        // Surrounding content preserved.
        assert!(after.contains("# Top"));
        assert!(after.contains("User notes after global."));
    }

    #[test]
    fn second_run_is_unchanged() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        let r2 = dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        assert!(!r2.context_created, "second run shouldn't recreate context");
        assert!(!r2.gitignore_created);
        for w in &r2.writes {
            assert_eq!(w.status, LocalStatus::Unchanged, "second run should be idempotent; {:?}", w);
        }
    }

    #[test]
    fn upsert_replaces_prior_block_in_place() {
        let prior = "# Top\n\n<!-- harvey:infect-local START v1 -->\nold body\n<!-- harvey:infect-local END -->\n\n# Bottom\n";
        let new_block = render_local_block("new body");
        let (next, status) = upsert_local_block(prior, &new_block);
        assert_eq!(status, LocalStatus::Updated);
        assert!(next.contains("# Top"));
        assert!(next.contains("# Bottom"));
        assert!(next.contains("new body"));
        assert!(!next.contains("old body"));
    }

    // --- dry run + remove + safety ---------------------------------------

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        let r = dispatch_local(
            &proj,
            &home,
            LocalOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(r.dry_run);
        assert!(r.dry_run_diff.is_some());
        // Nothing on disk.
        assert!(!proj.join(".harvey").exists());
        for t in DERIVATIVE_TARGETS {
            assert!(!proj.join(t.relative).exists(), "{} should not exist", t.relative);
        }
    }

    #[test]
    fn remove_flag_strips_marker_blocks_only() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        // First install.
        dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        let context_before = fs::read_to_string(proj.join(".harvey/context.md")).unwrap();
        // Then remove.
        dispatch_local(
            &proj,
            &home,
            LocalOptions {
                remove: true,
                ..Default::default()
            },
        )
        .unwrap();
        // Derivatives have the block stripped.
        let claude = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        assert!(!claude.contains(LOCAL_BLOCK_START));
        assert!(!claude.contains(LOCAL_BLOCK_END));
        // Source of truth untouched.
        assert_eq!(
            fs::read_to_string(proj.join(".harvey/context.md")).unwrap(),
            context_before
        );
        assert!(proj.join(".harvey/.gitignore").exists());
    }

    #[test]
    fn refuses_when_project_root_equals_home() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        // No `.git` or `.harvey` anywhere, so find_project_root returns
        // home itself when dir == home.
        let err = dispatch_local(&home, &home, LocalOptions::default()).unwrap_err();
        assert!(
            err.to_string().contains("refusing to infect $HOME"),
            "unexpected error: {err}"
        );
    }

    // --- --ignore-derivatives: root .gitignore auto-manage -------------

    fn opts_with_ignore_derivatives() -> LocalOptions {
        LocalOptions {
            ignore_derivatives: true,
            ..Default::default()
        }
    }

    #[test]
    fn ignore_derivatives_creates_root_gitignore_when_absent() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        let r = dispatch_local(&proj, &home, opts_with_ignore_derivatives()).unwrap();
        assert_eq!(r.root_gitignore_status, Some(LocalStatus::Installed));
        let gi = fs::read_to_string(proj.join(".gitignore")).unwrap();
        assert!(gi.contains("# harvey:infect-local START v1"));
        assert!(gi.contains("/CLAUDE.md"));
        assert!(gi.contains("/.cursor/rules/makakoo.mdc"));
        assert!(gi.contains("# harvey:infect-local END"));
    }

    #[test]
    fn ignore_derivatives_appends_to_existing_root_gitignore_preserving_user_content() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        let existing = "# user-owned rules\ntarget/\n.DS_Store\n";
        fs::write(proj.join(".gitignore"), existing).unwrap();
        let r = dispatch_local(&proj, &home, opts_with_ignore_derivatives()).unwrap();
        assert_eq!(r.root_gitignore_status, Some(LocalStatus::Installed));
        let gi = fs::read_to_string(proj.join(".gitignore")).unwrap();
        assert!(gi.starts_with("# user-owned rules"));
        assert!(gi.contains("target/"));
        assert!(gi.contains(".DS_Store"));
        assert!(gi.contains("# harvey:infect-local START v1"));
        assert!(gi.contains("/CLAUDE.md"));
    }

    #[test]
    fn ignore_derivatives_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        dispatch_local(&proj, &home, opts_with_ignore_derivatives()).unwrap();
        let r2 = dispatch_local(&proj, &home, opts_with_ignore_derivatives()).unwrap();
        assert_eq!(r2.root_gitignore_status, Some(LocalStatus::Unchanged));
    }

    #[test]
    fn default_run_does_not_touch_root_gitignore() {
        // Opt-in: if the flag is off, we never create or modify the root
        // .gitignore. User's choice whether to commit derivatives.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        let r = dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        assert!(r.root_gitignore_status.is_none());
        assert!(!proj.join(".gitignore").exists());
    }

    #[test]
    fn remove_strips_root_gitignore_block_if_present() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        // Install with the flag.
        dispatch_local(&proj, &home, opts_with_ignore_derivatives()).unwrap();
        let gi_before = fs::read_to_string(proj.join(".gitignore")).unwrap();
        assert!(gi_before.contains("/CLAUDE.md"));
        // Remove.
        let r = dispatch_local(
            &proj,
            &home,
            LocalOptions {
                remove: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.root_gitignore_status, Some(LocalStatus::Removed));
        let gi_after = fs::read_to_string(proj.join(".gitignore")).unwrap();
        assert!(!gi_after.contains("# harvey:infect-local START"));
        assert!(!gi_after.contains("/CLAUDE.md"));
    }

    #[test]
    fn remove_preserves_user_content_in_root_gitignore() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        let user_body = "# user rules\n*.log\n";
        fs::write(proj.join(".gitignore"), user_body).unwrap();
        dispatch_local(&proj, &home, opts_with_ignore_derivatives()).unwrap();
        // Then remove.
        dispatch_local(
            &proj,
            &home,
            LocalOptions {
                remove: true,
                ..Default::default()
            },
        )
        .unwrap();
        let gi = fs::read_to_string(proj.join(".gitignore")).unwrap();
        assert!(gi.contains("# user rules"));
        assert!(gi.contains("*.log"));
        assert!(!gi.contains("# harvey:infect-local"));
    }

    #[test]
    fn remove_when_no_root_gitignore_block_is_nothing_to_remove() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(proj.join(".gitignore"), "target/\n").unwrap();
        let r = dispatch_local(
            &proj,
            &home,
            LocalOptions {
                remove: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.root_gitignore_status, Some(LocalStatus::NothingToRemove));
        // User content untouched.
        let gi = fs::read_to_string(proj.join(".gitignore")).unwrap();
        assert_eq!(gi, "target/\n");
    }

    #[test]
    fn gitignore_scaffolded_on_first_run_only() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("repo");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&home).unwrap();
        let r1 = dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        assert!(r1.gitignore_created);
        let gi_before = fs::read_to_string(proj.join(".harvey/.gitignore")).unwrap();
        // User edits it — our re-run shouldn't clobber.
        fs::write(proj.join(".harvey/.gitignore"), format!("{gi_before}# user added\n")).unwrap();
        let r2 = dispatch_local(&proj, &home, LocalOptions::default()).unwrap();
        assert!(!r2.gitignore_created);
        let gi_after = fs::read_to_string(proj.join(".harvey/.gitignore")).unwrap();
        assert!(gi_after.contains("# user added"));
    }
}
