//! Tier-B `harvey_infect_local` handler — project-scoped infect from chat.
//!
//! Callable by the model when the user says "make this project Harvey-aware"
//! or similar. Shells out to the same `makakoo infect --local` binary the
//! shell user would run — zero logic duplication with `makakoo/src/infect/
//! local.rs`; the MCP handler is a thin, safety-gated invoker.
//!
//! Security: path must be absolute, must exist as a directory, must
//! canonicalise inside `$HOME` (Phase B refuses system-root targets — use
//! the CLI directly if you really mean to infect outside your home).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

pub struct HarveyInfectLocalHandler {
    ctx: Arc<ToolContext>,
}

impl HarveyInfectLocalHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

/// Validate + canonicalise the caller-supplied path. Returns the resolved
/// absolute path on success, or a user-facing error string.
///
/// Rules:
/// - Must be absolute (no relative paths — ambiguity disallowed in MCP)
/// - Must exist as a directory
/// - After canonicalisation, must be inside `$HOME`
fn validate_path(raw: &str, home: &Path) -> Result<PathBuf, String> {
    let as_path = Path::new(raw);
    if !as_path.is_absolute() {
        return Err(format!(
            "path must be absolute; got relative path {raw:?}"
        ));
    }
    let canon = std::fs::canonicalize(as_path).map_err(|e| {
        format!("path {raw:?} does not resolve: {e}")
    })?;
    if !canon.is_dir() {
        return Err(format!(
            "path {} exists but is not a directory",
            canon.display()
        ));
    }
    let home_canon = std::fs::canonicalize(home)
        .unwrap_or_else(|_| home.to_path_buf());
    if !canon.starts_with(&home_canon) {
        return Err(format!(
            "path {} is outside $HOME ({}); use the shell `makakoo infect --local` \
             directly for system-root projects",
            canon.display(),
            home_canon.display()
        ));
    }
    Ok(canon)
}

/// Resolve the `makakoo` binary we should shell to. Mirrors the watchdog-infect
/// resolution rule from sprint-008: PATH first, `~/.cargo/bin/makakoo` fallback.
fn resolve_makakoo_binary() -> Option<PathBuf> {
    if let Ok(output) = std::process::Command::new("which").arg("makakoo").output() {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout);
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(PathBuf::from(trimmed));
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let fallback = home.join(".cargo/bin/makakoo");
        if fallback.exists() {
            return Some(fallback);
        }
    }
    None
}

/// If `rules` is provided, pre-write or append it into `<path>/.harvey/context.md`
/// before invoking the CLI. This is the one semantic addition the MCP surface
/// has over the shell flag (the shell flag always uses either existing content
/// or the starter template).
fn apply_rules_to_context(project_path: &Path, rules: &str) -> std::io::Result<()> {
    let harvey_dir = project_path.join(".harvey");
    std::fs::create_dir_all(&harvey_dir)?;
    let context_path = harvey_dir.join("context.md");
    if context_path.exists() {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut existing = std::fs::read_to_string(&context_path)?;
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&format!("\n## Update {today}\n\n"));
        existing.push_str(rules.trim_end());
        existing.push('\n');
        std::fs::write(&context_path, existing)?;
    } else {
        std::fs::write(
            &context_path,
            format!("# Harvey — project rules\n\n{}\n", rules.trim_end()),
        )?;
    }
    Ok(())
}

#[async_trait]
impl ToolHandler for HarveyInfectLocalHandler {
    fn name(&self) -> &str {
        "harvey_infect_local"
    }

    fn description(&self) -> &str {
        "Initialize or update project-scoped Harvey rules. Writes \
         .harvey/context.md (canonical source) and regenerates per-CLI \
         project files (CLAUDE.md, GEMINI.md, AGENTS.md, QWEN.md, \
         .cursor/rules/makakoo.mdc, .vibe/context.md). Each CLI reads its \
         own file when a session starts in that directory. Absolute path \
         inside $HOME required."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the project directory. Must be inside $HOME."
                },
                "rules": {
                    "type": "string",
                    "description": "Optional. Initial content for .harvey/context.md. If the file exists, appended under a dated heading; never overwrites."
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Preview what would be written without touching any files.",
                    "default": false
                },
                "detect_installed_only": {
                    "type": "boolean",
                    "description": "Only write derivatives for CLIs with ~/.<cli>/ present. Default writes all 6.",
                    "default": false
                }
            }
        })
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let raw_path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing required `path`"))?;

        let home = dirs::home_dir().ok_or_else(|| {
            RpcError::internal("no $HOME available to validate path against")
        })?;

        let project_path = validate_path(raw_path, &home)
            .map_err(|e| RpcError::invalid_params(&e))?;

        let rules = params.get("rules").and_then(|v| v.as_str());
        let dry_run = params
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let detect_only = params
            .get("detect_installed_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Apply rules (real writes only — don't mutate on dry-run previews).
        if let Some(r) = rules {
            if !dry_run {
                apply_rules_to_context(&project_path, r).map_err(|e| {
                    RpcError::internal(&format!(
                        "failed to write .harvey/context.md: {e}"
                    ))
                })?;
            }
        }

        let makakoo = resolve_makakoo_binary().ok_or_else(|| {
            RpcError::internal(
                "`makakoo` binary not found on PATH or at ~/.cargo/bin; \
                 install via `cargo install --path makakoo` and retry",
            )
        })?;

        let mut cmd = Command::new(&makakoo);
        cmd.arg("infect").arg("--local");
        cmd.arg("--dir").arg(&project_path);
        if dry_run {
            cmd.arg("--dry-run");
        }
        if detect_only {
            cmd.arg("--detect-installed-only");
        }

        let output = cmd.output().await.map_err(|e| {
            RpcError::internal(&format!("failed to spawn makakoo: {e}"))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        // Touch ctx.home so the empty-test path counts as a field use and
        // future fields on ToolContext can be referenced here.
        let _ = self.ctx.home.clone();

        if exit_code != 0 {
            return Ok(json!({
                "ok": false,
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr,
                "project_path": project_path.display().to_string(),
            }));
        }

        Ok(json!({
            "ok": true,
            "exit_code": exit_code,
            "project_path": project_path.display().to_string(),
            "context_path": project_path.join(".harvey/context.md").display().to_string(),
            "dry_run": dry_run,
            "output": stdout,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_handler() -> HarveyInfectLocalHandler {
        let tmp = TempDir::new().unwrap();
        let ctx = Arc::new(ToolContext::empty(tmp.path().to_path_buf()));
        HarveyInfectLocalHandler::new(ctx)
    }

    // --- Path validation -------------------------------------------------

    #[test]
    fn validate_path_rejects_relative() {
        let tmp = TempDir::new().unwrap();
        let err = validate_path("./foo", tmp.path()).unwrap_err();
        assert!(err.contains("absolute"));
    }

    #[test]
    fn validate_path_rejects_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let err = validate_path(missing.to_str().unwrap(), tmp.path()).unwrap_err();
        assert!(err.contains("does not resolve"));
    }

    #[test]
    fn validate_path_rejects_file_not_dir() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a-file");
        std::fs::write(&file, "x").unwrap();
        let err = validate_path(file.to_str().unwrap(), tmp.path()).unwrap_err();
        assert!(err.contains("not a directory"));
    }

    #[test]
    fn validate_path_rejects_outside_home() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let err = validate_path(outside.to_str().unwrap(), &home).unwrap_err();
        assert!(err.contains("outside $HOME"));
    }

    #[test]
    fn validate_path_accepts_inside_home() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let inside = home.join("projects/myrepo");
        std::fs::create_dir_all(&inside).unwrap();
        let ok = validate_path(inside.to_str().unwrap(), &home).unwrap();
        assert!(ok.starts_with(std::fs::canonicalize(&home).unwrap()));
    }

    // --- Path-traversal defence -----------------------------------------

    #[test]
    fn validate_path_blocks_traversal_out_of_home() {
        // canonicalize resolves `..` — if the result lands outside $HOME,
        // the "outside $HOME" check fires. Prove the traversal is blocked.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let probe = home.join("../other");
        let err = validate_path(probe.to_str().unwrap(), &home).unwrap_err();
        assert!(err.contains("outside $HOME"), "unexpected error: {err}");
    }

    // --- Rules injection -------------------------------------------------

    #[test]
    fn apply_rules_creates_fresh_context_with_template_prefix() {
        let tmp = TempDir::new().unwrap();
        apply_rules_to_context(tmp.path(), "- never use Jenkins\n").unwrap();
        let content = std::fs::read_to_string(tmp.path().join(".harvey/context.md")).unwrap();
        assert!(content.contains("Harvey — project rules"));
        assert!(content.contains("never use Jenkins"));
    }

    #[test]
    fn apply_rules_appends_dated_section_to_existing_context() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".harvey")).unwrap();
        std::fs::write(
            tmp.path().join(".harvey/context.md"),
            "# My rules\n\n- original one\n",
        )
        .unwrap();
        apply_rules_to_context(tmp.path(), "- added later\n").unwrap();
        let content = std::fs::read_to_string(tmp.path().join(".harvey/context.md")).unwrap();
        assert!(content.contains("- original one"));
        assert!(content.contains("- added later"));
        assert!(content.contains("## Update "));
    }

    // --- MCP-level schema + error paths ---------------------------------

    #[test]
    fn handler_metadata() {
        let h = make_handler();
        assert_eq!(h.name(), "harvey_infect_local");
        let desc = h.description();
        assert!(desc.contains("project"));
        assert!(desc.to_lowercase().contains("cli"));
        let schema = h.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "path");
    }

    #[tokio::test]
    async fn call_missing_path_returns_invalid_params() {
        let h = make_handler();
        let err = h.call(json!({})).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.to_lowercase().contains("path") || msg.contains("-32602"));
    }

    #[tokio::test]
    async fn call_rejects_relative_path() {
        let h = make_handler();
        let err = h.call(json!({"path": "./foo"})).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("absolute") || msg.contains("-32602"));
    }
}
