//! Deep drift audit — the three scopes `infect --verify` used to miss.
//!
//! `drift.rs` catches the top-level CLI slot. This module catches:
//!
//!   * `~/.claude.json` → `projects[*].mcpServers.harvey` — per-project
//!     MCP registrations Claude writes when you run `claude mcp add`
//!     inside a specific directory. Survive the global rewrite.
//!   * `.mcp.json` files at workspace roots and Claude worktrees —
//!     Claude reads these as project-local overrides. Not touched by the
//!     global `sync_all` pipeline.
//!   * `git worktree` records pointing at dead directories — prunable
//!     metadata the verify subcommand was silent about.
//!
//! Shape of the fix mirrors Sebastian's ad-hoc Python one-shot from
//! 2026-04-19 (found 6 zombies after `infect --verify` reported 7/7
//! clean). Every rewrite is idempotent: running twice produces one change
//! on the first run and zero on the second.
//!
//! # Search scope
//!
//! The walker is deliberately narrow — it does **not** recurse into
//! arbitrary subtrees (node_modules, venvs, target/). Paths checked:
//!
//!   * `$MAKAKOO_HOME/.mcp.json`
//!   * `$MAKAKOO_HOME/.claude/worktrees/*/.mcp.json`
//!   * extra roots passed in via [`audit_workspace_mcp_files`]
//!
//! That's enough for every zombie we've seen. Wider walks can be added
//! per caller as they surface.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Map, Value};

use crate::infect::mcp::McpServerSpec;

/// Stale `harvey` entry inside `~/.claude.json` → `projects[*].mcpServers`.
/// Per-project scopes are invisible to the top-level verify path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDrift {
    /// Project key as stored in `~/.claude.json` (e.g. `/Users/sebastian/HARVEY`).
    pub project_key: String,
    /// Canonical location we'd rewrite against.
    pub claude_json_path: PathBuf,
    /// `command` field in the project's `harvey` entry doesn't match canonical.
    pub command_stale: bool,
    /// `args` field differs from canonical `[]`.
    pub args_stale: bool,
    /// Env keys whose values contain a dead `/Users/sebastian/HARVEY` path
    /// OR disagree with the canonical spec.
    pub zombie_env_keys: Vec<String>,
}

impl ProjectDrift {
    pub fn is_clean(&self) -> bool {
        !self.command_stale && !self.args_stale && self.zombie_env_keys.is_empty()
    }

    pub fn issues_human(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.command_stale {
            out.push("project-scope-command-stale");
        }
        if self.args_stale {
            out.push("project-scope-args-stale");
        }
        if !self.zombie_env_keys.is_empty() {
            out.push("project-scope-env-stale");
        }
        out
    }
}

/// Stale `.mcp.json` at a workspace root or Claude worktree. Claude
/// reads these as project-local MCP overrides that shadow the global
/// `~/.claude.json` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDrift {
    /// Absolute path to the `.mcp.json` file.
    pub path: PathBuf,
    pub command_stale: bool,
    pub args_stale: bool,
    pub zombie_env_keys: Vec<String>,
}

impl WorkspaceDrift {
    pub fn is_clean(&self) -> bool {
        !self.command_stale && !self.args_stale && self.zombie_env_keys.is_empty()
    }

    pub fn issues_human(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.command_stale {
            out.push("workspace-command-stale");
        }
        if self.args_stale {
            out.push("workspace-args-stale");
        }
        if !self.zombie_env_keys.is_empty() {
            out.push("workspace-env-stale");
        }
        out
    }
}

/// A `git worktree` record pointing at a directory that no longer exists
/// (marked `prunable` by `git worktree list --porcelain`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunableWorktree {
    /// Repository that owns the stale record.
    pub repo: PathBuf,
    /// Branch / worktree name inside the repo.
    pub worktree_name: String,
    /// The dead path the record still points at.
    pub dead_path: PathBuf,
    /// `prunable` reason emitted by git (e.g. `gitdir file points to non-existent location`).
    pub reason: String,
}

/// Combined deep-audit result. Empty vecs everywhere = clean.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeepDriftReport {
    pub claude_projects: Vec<ProjectDrift>,
    pub workspaces: Vec<WorkspaceDrift>,
    pub prunable_worktrees: Vec<PrunableWorktree>,
}

impl DeepDriftReport {
    pub fn is_clean(&self) -> bool {
        self.claude_projects.iter().all(|p| p.is_clean())
            && self.workspaces.iter().all(|w| w.is_clean())
            && self.prunable_worktrees.is_empty()
    }

    pub fn total_issue_count(&self) -> usize {
        self.claude_projects.iter().filter(|p| !p.is_clean()).count()
            + self.workspaces.iter().filter(|w| !w.is_clean()).count()
            + self.prunable_worktrees.len()
    }
}

/// Audit every deep scope at once. `home` is OS-level `$HOME`,
/// `makakoo_home` is `$MAKAKOO_HOME`. Extra search roots can be added
/// for callers that know about additional workspace roots.
pub fn deep_audit(
    home: &Path,
    makakoo_home: &Path,
    spec: &McpServerSpec,
    extra_roots: &[PathBuf],
) -> DeepDriftReport {
    let mut report = DeepDriftReport::default();

    let claude_json = home.join(".claude.json");
    report.claude_projects = audit_claude_projects(&claude_json, spec);

    let mut roots = vec![makakoo_home.to_path_buf()];
    roots.extend_from_slice(extra_roots);
    report.workspaces = audit_workspace_mcp_files(&roots, spec);

    report.prunable_worktrees = audit_prunable_worktrees(makakoo_home);
    report
}

/// Walk `projects[*].mcpServers.harvey` inside `~/.claude.json` and
/// flag zombie commands / args / env values.
pub fn audit_claude_projects(claude_json: &Path, spec: &McpServerSpec) -> Vec<ProjectDrift> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(claude_json) else {
        return out;
    };
    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    let Some(projects) = root.get("projects").and_then(|v| v.as_object()) else {
        return out;
    };
    for (key, cfg) in projects {
        let Some(mcp) = cfg.get("mcpServers").and_then(|v| v.as_object()) else {
            continue;
        };
        let Some(harvey) = mcp.get("harvey").and_then(|v| v.as_object()) else {
            continue;
        };
        let drift = diff_against_spec_json(
            key.clone(),
            claude_json.to_path_buf(),
            harvey,
            spec,
        );
        if !drift.is_clean() {
            out.push(drift);
        }
    }
    out
}

/// Apply the canonical spec to every zombie project scope. Returns a list
/// of human-readable actions taken. Idempotent: a second call with the
/// same input produces zero actions (drift vec is computed fresh).
pub fn repair_claude_projects(
    claude_json: &Path,
    spec: &McpServerSpec,
    drifts: &[ProjectDrift],
) -> std::io::Result<Vec<String>> {
    if drifts.is_empty() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(claude_json)?;
    let mut root: Value = serde_json::from_str(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let Some(projects) = root.get_mut("projects").and_then(|v| v.as_object_mut()) else {
        return Ok(Vec::new());
    };
    let mut actions = Vec::new();
    for drift in drifts {
        let Some(cfg) = projects.get_mut(&drift.project_key).and_then(|v| v.as_object_mut()) else {
            continue;
        };
        let Some(mcp) = cfg.get_mut("mcpServers").and_then(|v| v.as_object_mut()) else {
            continue;
        };
        // Force full canonical rewrite — simpler and cheaper than surgical
        // sub-key patching, and still deterministic because spec.env is a
        // BTreeMap.
        mcp.insert("harvey".to_string(), canonical_harvey_value(spec));
        actions.push(format!(
            "rewrote project-scope harvey entry for {}",
            drift.project_key
        ));
    }
    if !actions.is_empty() {
        let pretty = serde_json::to_string_pretty(&root)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(claude_json, pretty)?;
    }
    Ok(actions)
}

/// Scan every known workspace root for `.mcp.json` files and flag zombies.
///
/// Narrow scope: only `<root>/.mcp.json` and `<root>/.claude/worktrees/*/.mcp.json`.
/// Arbitrary-subdir recursion is out of scope (leads into node_modules).
pub fn audit_workspace_mcp_files(roots: &[PathBuf], spec: &McpServerSpec) -> Vec<WorkspaceDrift> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for root in roots {
        for path in discover_workspace_mcp_paths(root) {
            let canonical = path.canonicalize().unwrap_or(path.clone());
            if !seen.insert(canonical) {
                continue;
            }
            if let Some(drift) = audit_one_workspace_mcp(&path, spec) {
                if !drift.is_clean() {
                    out.push(drift);
                }
            }
        }
    }
    out
}

/// Apply canonical spec to every zombie `.mcp.json`. Idempotent.
pub fn repair_workspace_mcp_files(
    spec: &McpServerSpec,
    drifts: &[WorkspaceDrift],
) -> std::io::Result<Vec<String>> {
    let mut actions = Vec::new();
    for drift in drifts {
        let text = std::fs::read_to_string(&drift.path)?;
        let mut root: Value = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mcp = root
            .get_mut("mcpServers")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{} missing mcpServers", drift.path.display()),
                )
            })?;
        mcp.insert("harvey".to_string(), canonical_harvey_value(spec));
        let pretty = serde_json::to_string_pretty(&root)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&drift.path, pretty + "\n")?;
        actions.push(format!("rewrote {}", drift.path.display()));
    }
    Ok(actions)
}

/// Parse `git worktree list --porcelain` inside `repo` and return every
/// record flagged `prunable`. Silent when the repo isn't a git repo or
/// git isn't on PATH.
pub fn audit_prunable_worktrees(repo: &Path) -> Vec<PrunableWorktree> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("worktree")
        .arg("list")
        .arg("--porcelain")
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_worktree_porcelain(repo, &String::from_utf8_lossy(&output.stdout))
}

/// Call `git worktree prune` inside `repo`. Non-destructive: git only
/// removes metadata pointing at non-existent paths.
pub fn repair_prunable_worktrees(prunable: &[PrunableWorktree]) -> Vec<String> {
    let mut actions = Vec::new();
    let mut seen_repos = std::collections::BTreeSet::new();
    for p in prunable {
        if !seen_repos.insert(p.repo.clone()) {
            continue;
        }
        let status = Command::new("git")
            .arg("-C")
            .arg(&p.repo)
            .arg("worktree")
            .arg("prune")
            .status();
        match status {
            Ok(s) if s.success() => {
                actions.push(format!("pruned stale worktree records in {}", p.repo.display()));
            }
            Ok(s) => {
                actions.push(format!(
                    "FAILED to prune worktree in {} (exit {})",
                    p.repo.display(),
                    s.code().unwrap_or(-1)
                ));
            }
            Err(e) => {
                actions.push(format!("FAILED to prune worktree in {}: {}", p.repo.display(), e));
            }
        }
    }
    actions
}

/// One-shot deep repair — fix every zombie the audit found.
pub fn repair_deep(
    home: &Path,
    spec: &McpServerSpec,
    report: &DeepDriftReport,
) -> Vec<String> {
    let mut actions = Vec::new();
    let claude_json = home.join(".claude.json");
    if let Ok(a) = repair_claude_projects(&claude_json, spec, &report.claude_projects) {
        actions.extend(a);
    }
    if let Ok(a) = repair_workspace_mcp_files(spec, &report.workspaces) {
        actions.extend(a);
    }
    actions.extend(repair_prunable_worktrees(&report.prunable_worktrees));
    actions
}

// ─────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────

fn diff_against_spec_json(
    project_key: String,
    claude_json_path: PathBuf,
    harvey: &Map<String, Value>,
    spec: &McpServerSpec,
) -> ProjectDrift {
    let command_stale = harvey
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s != spec.command)
        .unwrap_or(true);

    let args_stale = match harvey.get("args") {
        Some(Value::Array(a)) => {
            let actual: Vec<String> = a
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            actual != spec.args
        }
        None => !spec.args.is_empty(),
        _ => true,
    };

    let env_obj = harvey
        .get("env")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let zombie_env_keys = zombie_env_keys(&env_obj, &spec.env);

    ProjectDrift {
        project_key,
        claude_json_path,
        command_stale,
        args_stale,
        zombie_env_keys,
    }
}

fn audit_one_workspace_mcp(path: &Path, spec: &McpServerSpec) -> Option<WorkspaceDrift> {
    let text = std::fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    let mcp = root.get("mcpServers")?.as_object()?;
    let harvey = mcp.get("harvey")?.as_object()?;

    let command_stale = harvey
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s != spec.command)
        .unwrap_or(true);

    let args_stale = match harvey.get("args") {
        Some(Value::Array(a)) => {
            let actual: Vec<String> = a
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            actual != spec.args
        }
        None => !spec.args.is_empty(),
        _ => true,
    };

    let env_obj = harvey
        .get("env")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let zombie_env_keys = zombie_env_keys(&env_obj, &spec.env);

    Some(WorkspaceDrift {
        path: path.to_path_buf(),
        command_stale,
        args_stale,
        zombie_env_keys,
    })
}

/// Any env key whose current value diverges from canonical — or
/// any canonical key missing entirely. Special-cases `PYTHONPATH` as
/// optional (harvey-os Python subdir isn't required for the Rust MCP).
fn zombie_env_keys(actual: &Map<String, Value>, canonical: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for (k, want) in canonical {
        if k == "PYTHONPATH" {
            // Tolerated: Rust MCP doesn't need it, but spec still emits it
            // for Python fallback compat. Only flag if present AND stale.
            match actual.get(k).and_then(|v| v.as_str()) {
                Some(s) if s != want && s.contains("/Users/sebastian/HARVEY") => {
                    out.push(k.clone())
                }
                _ => {}
            }
            continue;
        }
        match actual.get(k).and_then(|v| v.as_str()) {
            Some(s) if s == want => {}
            Some(_) => out.push(k.clone()),
            None => out.push(k.clone()),
        }
    }
    for (k, v) in actual {
        if let Some(s) = v.as_str() {
            if s.contains("/Users/sebastian/HARVEY") && !out.contains(k) {
                out.push(k.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn canonical_harvey_value(spec: &McpServerSpec) -> Value {
    let mut env = Map::new();
    for (k, v) in &spec.env {
        env.insert(k.clone(), Value::String(v.clone()));
    }
    json!({
        "command": spec.command,
        "args": spec.args,
        "env": Value::Object(env),
    })
}

fn discover_workspace_mcp_paths(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let direct = root.join(".mcp.json");
    if direct.is_file() {
        out.push(direct);
    }
    let worktrees = root.join(".claude").join("worktrees");
    if worktrees.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&worktrees) {
            for entry in entries.flatten() {
                let p = entry.path().join(".mcp.json");
                if p.is_file() {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn parse_worktree_porcelain(repo: &Path, text: &str) -> Vec<PrunableWorktree> {
    let mut out = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    let mut cur_prunable: Option<String> = None;
    for line in text.lines() {
        if line.is_empty() {
            if let (Some(path), Some(reason)) = (&cur_path, &cur_prunable) {
                let name = cur_branch
                    .clone()
                    .or_else(|| {
                        path.file_name()
                            .and_then(|s| s.to_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                out.push(PrunableWorktree {
                    repo: repo.to_path_buf(),
                    worktree_name: name,
                    dead_path: path.clone(),
                    reason: reason.clone(),
                });
            }
            cur_path = None;
            cur_branch = None;
            cur_prunable = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            cur_path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
            cur_branch = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("prunable ") {
            cur_prunable = Some(rest.to_string());
        } else if line == "prunable" {
            cur_prunable = Some("prunable".to_string());
        }
    }
    // Trailing record with no blank line terminator.
    if let (Some(path), Some(reason)) = (cur_path, cur_prunable) {
        let name = cur_branch
            .or_else(|| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        out.push(PrunableWorktree {
            repo: repo.to_path_buf(),
            worktree_name: name,
            dead_path: path,
            reason,
        });
    }
    out
}

/// Serialize the deep report as JSON for the `--verify --json` flow.
/// Schema is additive against the shallow payload — consumers that don't
/// know about `deep` simply ignore the field.
pub fn to_json(report: &DeepDriftReport) -> Value {
    json!({
        "clean": report.is_clean(),
        "total_issues": report.total_issue_count(),
        "claude_projects": report
            .claude_projects
            .iter()
            .map(|p| json!({
                "project_key": p.project_key,
                "issues": p.issues_human(),
                "zombie_env_keys": p.zombie_env_keys,
            }))
            .collect::<Vec<_>>(),
        "workspaces": report
            .workspaces
            .iter()
            .map(|w| json!({
                "path": w.path.display().to_string(),
                "issues": w.issues_human(),
                "zombie_env_keys": w.zombie_env_keys,
            }))
            .collect::<Vec<_>>(),
        "prunable_worktrees": report
            .prunable_worktrees
            .iter()
            .map(|p| json!({
                "repo": p.repo.display().to_string(),
                "name": p.worktree_name,
                "dead_path": p.dead_path.display().to_string(),
                "reason": p.reason,
            }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn canonical_spec(home: &Path) -> McpServerSpec {
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
            command: "/Users/sebastian/.cargo/bin/makakoo-mcp".to_string(),
            args: vec![],
            env,
            prompt: None,
        }
    }

    #[test]
    fn zombie_env_detects_dead_harvey_path() {
        let canonical: BTreeMap<String, String> = [
            ("MAKAKOO_HOME", "/Users/sebastian/MAKAKOO"),
            ("HARVEY_HOME", "/Users/sebastian/MAKAKOO"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let mut actual = Map::new();
        actual.insert(
            "HARVEY_HOME".to_string(),
            Value::String("/Users/sebastian/HARVEY".to_string()),
        );
        actual.insert(
            "PYTHONPATH".to_string(),
            Value::String("/Users/sebastian/HARVEY/harvey-os".to_string()),
        );
        let zombies = zombie_env_keys(&actual, &canonical);
        assert!(zombies.contains(&"HARVEY_HOME".to_string()));
        assert!(zombies.contains(&"MAKAKOO_HOME".to_string()));
        assert!(zombies.contains(&"PYTHONPATH".to_string()));
    }

    #[test]
    fn zombie_env_clean_when_canonical_matches() {
        let canonical: BTreeMap<String, String> = [
            ("MAKAKOO_HOME", "/Users/sebastian/MAKAKOO"),
            ("HARVEY_HOME", "/Users/sebastian/MAKAKOO"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let mut actual = Map::new();
        actual.insert(
            "HARVEY_HOME".to_string(),
            Value::String("/Users/sebastian/MAKAKOO".to_string()),
        );
        actual.insert(
            "MAKAKOO_HOME".to_string(),
            Value::String("/Users/sebastian/MAKAKOO".to_string()),
        );
        let zombies = zombie_env_keys(&actual, &canonical);
        assert!(zombies.is_empty(), "got: {:?}", zombies);
    }

    #[test]
    fn audit_claude_projects_flags_dead_python_command() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        let claude_json = dir.path().join(".claude.json");
        fs::write(
            &claude_json,
            r#"{
              "projects": {
                "/Users/sebastian/HARVEY": {
                  "mcpServers": {
                    "harvey": {
                      "command": "python3",
                      "args": ["/Users/sebastian/HARVEY/harvey-os/core/mcp/harvey_mcp.py"],
                      "env": {
                        "HARVEY_HOME": "/Users/sebastian/HARVEY",
                        "PYTHONPATH": "/Users/sebastian/HARVEY/harvey-os"
                      }
                    }
                  }
                },
                "/Users/sebastian/clean": {
                  "mcpServers": {}
                }
              }
            }"#,
        )
        .unwrap();
        let drifts = audit_claude_projects(&claude_json, &spec);
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].project_key, "/Users/sebastian/HARVEY");
        assert!(drifts[0].command_stale);
        assert!(drifts[0].args_stale);
        assert!(drifts[0].zombie_env_keys.contains(&"HARVEY_HOME".to_string()));
    }

    #[test]
    fn audit_claude_projects_missing_file_is_empty_not_error() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        let drifts = audit_claude_projects(&dir.path().join("no-such.json"), &spec);
        assert!(drifts.is_empty());
    }

    #[test]
    fn repair_claude_projects_rewrites_zombie_to_canonical() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        let claude_json = dir.path().join(".claude.json");
        fs::write(
            &claude_json,
            r#"{
              "projects": {
                "/Users/sebastian/HARVEY": {
                  "mcpServers": {
                    "harvey": {
                      "command": "python3",
                      "args": ["dead.py"],
                      "env": {"HARVEY_HOME": "/Users/sebastian/HARVEY"}
                    },
                    "other-server": {"command": "keep-me"}
                  }
                }
              }
            }"#,
        )
        .unwrap();

        let drifts_before = audit_claude_projects(&claude_json, &spec);
        assert_eq!(drifts_before.len(), 1);
        let actions = repair_claude_projects(&claude_json, &spec, &drifts_before).unwrap();
        assert_eq!(actions.len(), 1);

        let after = audit_claude_projects(&claude_json, &spec);
        assert!(after.is_empty(), "post-repair drift: {:?}", after);

        // Other-server preserved, harvey canonicalized.
        let after_json: Value = serde_json::from_str(&fs::read_to_string(&claude_json).unwrap()).unwrap();
        let harvey = &after_json["projects"]["/Users/sebastian/HARVEY"]["mcpServers"]["harvey"];
        assert_eq!(harvey["command"], spec.command);
        assert_eq!(harvey["args"].as_array().unwrap().len(), 0);
        assert_eq!(harvey["env"]["MAKAKOO_HOME"], spec.env["MAKAKOO_HOME"]);
        let other = &after_json["projects"]["/Users/sebastian/HARVEY"]["mcpServers"]["other-server"];
        assert_eq!(other["command"], "keep-me");
    }

    #[test]
    fn repair_is_idempotent() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        let claude_json = dir.path().join(".claude.json");
        fs::write(
            &claude_json,
            r#"{
              "projects": {
                "/a": {
                  "mcpServers": {"harvey": {"command": "python3", "args": ["x"], "env": {}}}
                }
              }
            }"#,
        )
        .unwrap();
        let drifts = audit_claude_projects(&claude_json, &spec);
        let first = repair_claude_projects(&claude_json, &spec, &drifts).unwrap();
        assert!(!first.is_empty());
        let drifts2 = audit_claude_projects(&claude_json, &spec);
        assert!(drifts2.is_empty(), "second audit should be clean");
        let second = repair_claude_projects(&claude_json, &spec, &drifts2).unwrap();
        assert!(second.is_empty(), "second repair should do nothing");
    }

    #[test]
    fn discover_finds_direct_and_worktree_mcp_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".mcp.json"), "{}").unwrap();
        let wt = dir.path().join(".claude/worktrees/foo");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".mcp.json"), "{}").unwrap();
        let found = discover_workspace_mcp_paths(dir.path());
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn audit_workspace_flags_dead_python_mcp() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        fs::write(
            dir.path().join(".mcp.json"),
            r#"{
              "mcpServers": {
                "harvey": {
                  "command": "python3",
                  "args": ["${CLAUDE_PLUGIN_ROOT}/harvey-os/core/mcp/harvey_mcp.py"],
                  "env": {"HARVEY_HOME": "/Users/sebastian/HARVEY"}
                }
              }
            }"#,
        )
        .unwrap();
        let drifts = audit_workspace_mcp_files(&[dir.path().to_path_buf()], &spec);
        assert_eq!(drifts.len(), 1);
        assert!(drifts[0].command_stale);
        assert!(drifts[0].args_stale);
        assert!(!drifts[0].zombie_env_keys.is_empty());
    }

    #[test]
    fn audit_workspace_skips_clean_file() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        let clean = json!({
            "mcpServers": {
                "harvey": {
                    "command": spec.command.clone(),
                    "args": [],
                    "env": {
                        "HARVEY_HOME": spec.env["HARVEY_HOME"].clone(),
                        "MAKAKOO_HOME": spec.env["MAKAKOO_HOME"].clone(),
                    }
                }
            }
        });
        fs::write(
            dir.path().join(".mcp.json"),
            serde_json::to_string(&clean).unwrap(),
        )
        .unwrap();
        let drifts = audit_workspace_mcp_files(&[dir.path().to_path_buf()], &spec);
        assert!(drifts.is_empty(), "unexpected drift: {:?}", drifts);
    }

    #[test]
    fn repair_workspace_mcp_files_rewrites_and_is_idempotent() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        let path = dir.path().join(".mcp.json");
        fs::write(
            &path,
            r#"{
              "mcpServers": {
                "harvey": {"command": "python3", "args": ["dead"], "env": {}}
              }
            }"#,
        )
        .unwrap();
        let drifts = audit_workspace_mcp_files(&[dir.path().to_path_buf()], &spec);
        let actions = repair_workspace_mcp_files(&spec, &drifts).unwrap();
        assert_eq!(actions.len(), 1);
        let second = audit_workspace_mcp_files(&[dir.path().to_path_buf()], &spec);
        assert!(second.is_empty());
    }

    #[test]
    fn parse_worktree_porcelain_extracts_prunable_record() {
        let repo = PathBuf::from("/some/repo");
        let text = "worktree /path/to/live\nHEAD deadbeef\nbranch refs/heads/main\n\nworktree /path/to/dead\nHEAD cafef00d\nbranch refs/heads/elated-mendel\nprunable gitdir file points to non-existent location\n\n";
        let records = parse_worktree_porcelain(&repo, text);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].worktree_name, "elated-mendel");
        assert_eq!(records[0].dead_path, PathBuf::from("/path/to/dead"));
        assert!(records[0].reason.contains("gitdir file"));
    }

    #[test]
    fn parse_worktree_porcelain_handles_trailing_record_no_blank_line() {
        let repo = PathBuf::from("/r");
        let text = "worktree /dead\nprunable reason\n";
        let records = parse_worktree_porcelain(&repo, text);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].dead_path, PathBuf::from("/dead"));
    }

    #[test]
    fn parse_worktree_porcelain_empty_when_no_prunable() {
        let repo = PathBuf::from("/r");
        let text = "worktree /live\nHEAD abc\nbranch refs/heads/main\n\n";
        let records = parse_worktree_porcelain(&repo, text);
        assert!(records.is_empty());
    }

    #[test]
    fn to_json_shape_contains_every_deep_scope() {
        let report = DeepDriftReport {
            claude_projects: vec![ProjectDrift {
                project_key: "/p".to_string(),
                claude_json_path: PathBuf::from("/c.json"),
                command_stale: true,
                args_stale: false,
                zombie_env_keys: vec!["HARVEY_HOME".to_string()],
            }],
            workspaces: vec![WorkspaceDrift {
                path: PathBuf::from("/w/.mcp.json"),
                command_stale: true,
                args_stale: true,
                zombie_env_keys: vec![],
            }],
            prunable_worktrees: vec![PrunableWorktree {
                repo: PathBuf::from("/r"),
                worktree_name: "x".to_string(),
                dead_path: PathBuf::from("/d"),
                reason: "r".to_string(),
            }],
        };
        let v = to_json(&report);
        assert_eq!(v["clean"], false);
        assert_eq!(v["total_issues"], 3);
        assert_eq!(v["claude_projects"][0]["project_key"], "/p");
        assert_eq!(v["workspaces"][0]["path"], "/w/.mcp.json");
        assert_eq!(v["prunable_worktrees"][0]["name"], "x");
    }

    #[test]
    fn deep_audit_empty_home_is_clean() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        let report = deep_audit(dir.path(), dir.path(), &spec, &[]);
        assert!(report.is_clean());
        assert_eq!(report.total_issue_count(), 0);
    }

    #[test]
    fn deep_audit_composes_all_three_scopes() {
        let dir = tempdir().unwrap();
        let spec = canonical_spec(dir.path());
        // Zombie claude.json project scope.
        let claude = dir.path().join(".claude.json");
        fs::write(
            &claude,
            r#"{"projects":{"/p":{"mcpServers":{"harvey":{"command":"python3","args":[],"env":{}}}}}}"#,
        )
        .unwrap();
        // Zombie workspace .mcp.json inside fake MAKAKOO home.
        let mhome = dir.path().join("MAKAKOO");
        fs::create_dir_all(&mhome).unwrap();
        fs::write(
            mhome.join(".mcp.json"),
            r#"{"mcpServers":{"harvey":{"command":"python3","args":[],"env":{}}}}"#,
        )
        .unwrap();
        let report = deep_audit(dir.path(), &mhome, &spec, &[]);
        assert_eq!(report.claude_projects.len(), 1);
        assert_eq!(report.workspaces.len(), 1);
        // Prunable worktrees depends on `git` existing + tempdir being a
        // repo; we don't seed one, so expect empty.
        assert!(report.prunable_worktrees.is_empty());
        assert!(!report.is_clean());
    }
}
