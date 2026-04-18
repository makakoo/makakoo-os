//! JSON-mcpServers adapter — covers Claude, Gemini, Qwen, Cursor, OpenCode.
//!
//! Five CLIs share the same JSON-object-keyed-by-server-name shape, with
//! one minor variation: OpenCode uses `mcp` instead of `mcpServers` as
//! the parent key. This adapter handles both via the `is_opencode` flag.
//!
//! Design choices:
//!   * `serde_json::Value` round-trip — unrelated keys (Claude project
//!     state, Cursor's GitKraken entry, etc.) survive untouched.
//!   * Atomic write via tmp+rename so a crashed write never leaves a
//!     half-truncated user config.
//!   * Idempotent — a second run with the same spec returns
//!     `SyncOutcome::Unchanged` without rewriting the file (mtime
//!     preserved).
//!   * Permission preservation — files that already had restrictive
//!     mode bits keep them.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde_json::{json, Map, Value};

use crate::infect::mcp::{ChangeKind, McpServerSpec, McpTarget, SyncOutcome};

const SERVER_NAME: &str = "harvey";

/// Sync the JSON config at `path`. `is_opencode` switches the parent
/// key from `mcpServers` to `mcp`.
pub fn sync(
    _target: &McpTarget,
    path: &Path,
    spec: &McpServerSpec,
    dry_run: bool,
    is_opencode: bool,
) -> SyncOutcome {
    let parent_key = if is_opencode { "mcp" } else { "mcpServers" };

    // Read the existing config — start from empty `{}` if the file
    // doesn't yet exist (CLI installed but never wrote a config).
    let mut root = match read_json(path) {
        Ok(v) => v,
        Err(e) => {
            return SyncOutcome::Error {
                message: format!("read {}: {e}", path.display()),
            }
        }
    };

    let desired = build_server_value(spec);
    let outcome_kind = upsert_server(&mut root, parent_key, SERVER_NAME, desired.clone());

    if matches!(outcome_kind, UpsertOutcome::Unchanged) {
        return SyncOutcome::Unchanged;
    }
    if dry_run {
        return SyncOutcome::WouldChange {
            kind: match outcome_kind {
                UpsertOutcome::Added => ChangeKind::Add,
                UpsertOutcome::Updated => ChangeKind::Update,
                UpsertOutcome::Unchanged => unreachable!(),
            },
        };
    }

    if let Err(e) = write_atomic_pretty(path, &root) {
        return SyncOutcome::Error {
            message: format!("write {}: {e}", path.display()),
        };
    }
    match outcome_kind {
        UpsertOutcome::Added => SyncOutcome::Added,
        UpsertOutcome::Updated => SyncOutcome::Updated,
        UpsertOutcome::Unchanged => unreachable!(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpsertOutcome {
    Added,
    Updated,
    Unchanged,
}

/// Insert/replace `name → desired` inside `root[parent_key]`. Creates
/// the parent object if absent. Returns whether a write is needed.
fn upsert_server(
    root: &mut Value,
    parent_key: &str,
    name: &str,
    desired: Value,
) -> UpsertOutcome {
    if !root.is_object() {
        // Hostile config that isn't a JSON object — treat as fresh.
        *root = Value::Object(Map::new());
    }
    let obj = root.as_object_mut().unwrap();
    let parent = obj
        .entry(parent_key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !parent.is_object() {
        *parent = Value::Object(Map::new());
    }
    let parent_obj = parent.as_object_mut().unwrap();
    match parent_obj.get(name) {
        Some(existing) if existing == &desired => UpsertOutcome::Unchanged,
        Some(_) => {
            parent_obj.insert(name.to_string(), desired);
            UpsertOutcome::Updated
        }
        None => {
            parent_obj.insert(name.to_string(), desired);
            UpsertOutcome::Added
        }
    }
}

/// Convert a [`McpServerSpec`] into the JSON shape the CLIs expect.
fn build_server_value(spec: &McpServerSpec) -> Value {
    let mut env = Map::new();
    for (k, v) in &spec.env {
        env.insert(k.clone(), Value::String(v.clone()));
    }
    let mut entry = Map::new();
    entry.insert("command".to_string(), Value::String(spec.command.clone()));
    entry.insert(
        "args".to_string(),
        Value::Array(spec.args.iter().map(|a| Value::String(a.clone())).collect()),
    );
    entry.insert("env".to_string(), Value::Object(env));
    if let Some(prompt) = &spec.prompt {
        // Most JSON-mcpServers CLIs (Cursor, Claude Code) ignore unknown
        // keys; Gemini exposes `description`. Stick the prompt under
        // `description` for readability — schema-tolerant CLIs won't mind.
        entry.insert("description".to_string(), Value::String(prompt.clone()));
    }
    Value::Object(entry)
}

fn read_json(path: &Path) -> std::io::Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let body = fs::read_to_string(path)?;
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

fn write_atomic_pretty(path: &Path, root: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(root)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all().ok();
    }

    // Preserve the existing file mode if there was one (.claude.json
    // sometimes carries 600).
    #[cfg(unix)]
    {
        if let Ok(meta) = fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode));
        }
    }

    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn spec() -> McpServerSpec {
        let mut env = BTreeMap::new();
        env.insert("MAKAKOO_HOME".to_string(), "/h".to_string());
        env.insert("HARVEY_HOME".to_string(), "/h".to_string());
        env.insert("PYTHONPATH".to_string(), "/h/harvey-os".to_string());
        McpServerSpec {
            name: "harvey".to_string(),
            command: "/opt/cargo/bin/makakoo-mcp".to_string(),
            args: vec![],
            env,
            prompt: Some("desc".to_string()),
        }
    }

    #[test]
    fn add_to_empty_file_creates_mcpServers_with_harvey() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let outcome = sync(&McpTarget::Gemini, &path, &spec(), false, false);
        assert_eq!(outcome, SyncOutcome::Added);

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let harvey = &v["mcpServers"]["harvey"];
        assert_eq!(harvey["command"], "/opt/cargo/bin/makakoo-mcp");
        assert_eq!(harvey["env"]["MAKAKOO_HOME"], "/h");
    }

    #[test]
    fn add_into_existing_servers_preserves_others() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        // Cursor's GitKraken-equivalent fixture.
        fs::write(
            &path,
            r#"{"mcpServers":{"GitKraken":{"command":"/usr/bin/gk","args":[]}}}"#,
        )
        .unwrap();

        let outcome = sync(&McpTarget::Cursor, &path, &spec(), false, false);
        assert_eq!(outcome, SyncOutcome::Added);

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["mcpServers"].get("GitKraken").is_some());
        assert!(v["mcpServers"].get("harvey").is_some());
    }

    #[test]
    fn second_run_is_unchanged() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let _ = sync(&McpTarget::Gemini, &path, &spec(), false, false);
        let outcome = sync(&McpTarget::Gemini, &path, &spec(), false, false);
        assert_eq!(outcome, SyncOutcome::Unchanged);
    }

    #[test]
    fn stale_command_path_triggers_update() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"harvey":{"command":"/old/path/makakoo-mcp","args":[],"env":{}}}}"#,
        )
        .unwrap();
        let outcome = sync(&McpTarget::Gemini, &path, &spec(), false, false);
        assert_eq!(outcome, SyncOutcome::Updated);
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["harvey"]["command"], "/opt/cargo/bin/makakoo-mcp");
    }

    #[test]
    fn dry_run_returns_would_change_and_writes_nothing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let outcome = sync(&McpTarget::Gemini, &path, &spec(), true, false);
        assert_eq!(outcome, SyncOutcome::WouldChange { kind: ChangeKind::Add });
        assert!(!path.exists());
    }

    #[test]
    fn opencode_uses_mcp_parent_key_not_mcpServers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.json");
        let outcome = sync(&McpTarget::OpenCode, &path, &spec(), false, true);
        assert_eq!(outcome, SyncOutcome::Added);

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v.get("mcp").is_some());
        assert!(v.get("mcpServers").is_none());
        assert!(v["mcp"].get("harvey").is_some());
    }

    #[test]
    fn unknown_top_level_keys_round_trip_intact() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"editor":{"theme":"dark"},"mcpServers":{}}"#,
        )
        .unwrap();
        let _ = sync(&McpTarget::Gemini, &path, &spec(), false, false);
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["editor"]["theme"], "dark");
        assert!(v["mcpServers"].get("harvey").is_some());
    }

    #[test]
    fn malformed_json_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "not json {{{").unwrap();
        let outcome = sync(&McpTarget::Gemini, &path, &spec(), false, false);
        assert!(matches!(outcome, SyncOutcome::Error { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn preserves_existing_file_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let _ = sync(&McpTarget::Gemini, &path, &spec(), false, false);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
