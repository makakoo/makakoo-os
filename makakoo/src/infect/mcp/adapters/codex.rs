//! Codex TOML adapter — `[mcp_servers.<name>]` inline-table format.
//!
//! Codex stores each MCP server as a sub-table under `[mcp_servers.X]`
//! with nested `[mcp_servers.X.env]`. We use `toml_edit` so reads +
//! writes preserve comments, blank lines, and key ordering.
//!
//! Other config sections (`[features]`, `[mcp_servers.GitKraken]`,
//! per-tool approval rules under `[mcp_servers.harvey.tools.*]`) are
//! left untouched — we only ever upsert under `[mcp_servers.harvey]`
//! itself.

use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::infect::mcp::{ChangeKind, McpServerSpec, SyncOutcome};

const SERVER_KEY: &str = "harvey";

pub fn sync(
    path: &Path,
    spec: &McpServerSpec,
    dry_run: bool,
    model_instructions_file: Option<&Path>,
) -> SyncOutcome {
    let mut doc = match read_doc(path) {
        Ok(d) => d,
        Err(e) => {
            return SyncOutcome::Error {
                message: format!("read {}: {e}", path.display()),
            }
        }
    };

    // Snapshot the current `harvey` block + model_instructions_file key
    // for an already-equal short-circuit.
    let before = render_managed(&doc);
    upsert_harvey(&mut doc, spec);
    if let Some(p) = model_instructions_file {
        upsert_model_instructions_file(&mut doc, p);
    }
    let after = render_managed(&doc);

    let kind = match (before.as_deref(), after.as_deref()) {
        (None, _) => ChangeKind::Add,
        (Some(prev), Some(now)) if prev == now => return SyncOutcome::Unchanged,
        (Some(_), _) => ChangeKind::Update,
    };

    if dry_run {
        return SyncOutcome::WouldChange { kind };
    }

    if let Err(e) = write_atomic(path, &doc.to_string()) {
        return SyncOutcome::Error {
            message: format!("write {}: {e}", path.display()),
        };
    }

    match kind {
        ChangeKind::Add => SyncOutcome::Added,
        ChangeKind::Update => SyncOutcome::Updated,
    }
}

/// Render the parts of the doc that infect manages (harvey MCP block +
/// model_instructions_file), so the change-detection short-circuit
/// doesn't trip on user-edited keys (`personality`, `model`, etc.).
fn render_managed(doc: &DocumentMut) -> Option<String> {
    let harvey = render_harvey(doc).unwrap_or_default();
    let mif = doc
        .get("model_instructions_file")
        .map(|v| v.to_string())
        .unwrap_or_default();
    if harvey.is_empty() && mif.is_empty() {
        None
    } else {
        Some(format!("{harvey}|{mif}"))
    }
}

/// Set `model_instructions_file = "<path>"` at the top level of the
/// Codex config. This is Codex's official knob for loading custom
/// system instructions every session, replacing the deprecated
/// `experimental_instructions_file`.
fn upsert_model_instructions_file(doc: &mut DocumentMut, p: &Path) {
    let path_str = p.display().to_string();
    doc["model_instructions_file"] = value(path_str);
}

fn read_doc(path: &Path) -> std::io::Result<DocumentMut> {
    let body = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    body.parse::<DocumentMut>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// Render the current `[mcp_servers.harvey]` subtree as a normalised
/// string for "did anything change?" comparison.
fn render_harvey(doc: &DocumentMut) -> Option<String> {
    let servers = doc.get("mcp_servers")?;
    let table = servers.as_table()?;
    let harvey = table.get(SERVER_KEY)?;
    Some(harvey.to_string())
}

/// Insert/replace `[mcp_servers.harvey]` and `[mcp_servers.harvey.env]`.
/// Preserves any peer servers (GitKraken, etc.) and any per-tool
/// approval tables under `[mcp_servers.harvey.tools.*]`.
fn upsert_harvey(doc: &mut DocumentMut, spec: &McpServerSpec) {
    // Ensure parent table exists.
    if doc.get("mcp_servers").is_none() {
        doc["mcp_servers"] = Item::Table(Table::new());
    }
    let parent = doc["mcp_servers"]
        .as_table_mut()
        .expect("mcp_servers must be a table");
    parent.set_implicit(true);

    // Ensure the harvey table exists.
    if parent.get(SERVER_KEY).is_none() {
        parent.insert(SERVER_KEY, Item::Table(Table::new()));
    }
    let harvey = parent
        .get_mut(SERVER_KEY)
        .and_then(|i| i.as_table_mut())
        .expect("harvey entry must be a table");

    // Top-level keys.
    harvey.insert("command", value(spec.command.as_str()));
    let mut args = Array::new();
    for a in &spec.args {
        args.push(a.as_str());
    }
    harvey.insert("args", value(args));

    // Description = the prompt hint, if any.
    if let Some(desc) = &spec.prompt {
        harvey.insert("description", value(desc.as_str()));
    } else {
        harvey.remove("description");
    }

    // Nested env subtable. Replace the whole subtable so removed env
    // vars don't linger from a previous spec.
    let mut env_table = Table::new();
    for (k, v) in &spec.env {
        env_table.insert(k.as_str(), value(v.as_str()));
    }
    harvey.insert("env", Item::Table(env_table));
}

fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(body.as_bytes())?;
        if !body.ends_with('\n') {
            f.write_all(b"\n")?;
        }
        f.sync_all().ok();
    }
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
    fn add_to_empty_file_creates_block() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let outcome = sync(&path, &spec(), false, None);
        assert_eq!(outcome, SyncOutcome::Added);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("[mcp_servers.harvey]"));
        assert!(body.contains(r#"command = "/opt/cargo/bin/makakoo-mcp""#));
        assert!(body.contains("[mcp_servers.harvey.env]"));
        assert!(body.contains(r#"MAKAKOO_HOME = "/h""#));
    }

    #[test]
    fn second_run_is_unchanged() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let _ = sync(&path, &spec(), false, None);
        let outcome = sync(&path, &spec(), false, None);
        assert_eq!(outcome, SyncOutcome::Unchanged);
    }

    #[test]
    fn stale_command_path_triggers_update() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"# user config
[mcp_servers.harvey]
command = "python3"
args = ["/old/path/harvey_mcp.py"]

[mcp_servers.harvey.env]
HARVEY_HOME = "/h"
"#,
        )
        .unwrap();
        let outcome = sync(&path, &spec(), false, None);
        assert_eq!(outcome, SyncOutcome::Updated);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(r#"command = "/opt/cargo/bin/makakoo-mcp""#));
        assert!(!body.contains("python3"));
        // User comment must survive.
        assert!(body.contains("# user config"));
    }

    #[test]
    fn other_servers_and_tool_tables_preserved() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"[mcp_servers.GitKraken]
command = "/usr/bin/gk"
args = ["mcp"]

[mcp_servers.harvey]
command = "/old/makakoo-mcp"
args = []

[mcp_servers.harvey.env]
HARVEY_HOME = "/h"

[mcp_servers.harvey.tools.nursery_status]
approval_mode = "approve"
"#,
        )
        .unwrap();

        let outcome = sync(&path, &spec(), false, None);
        assert_eq!(outcome, SyncOutcome::Updated);
        let body = fs::read_to_string(&path).unwrap();

        // Sibling server intact.
        assert!(body.contains("[mcp_servers.GitKraken]"));
        assert!(body.contains(r#"command = "/usr/bin/gk""#));
        // Per-tool approval rule survived.
        assert!(body.contains("[mcp_servers.harvey.tools.nursery_status]"));
        assert!(body.contains(r#"approval_mode = "approve""#));
        // Harvey command updated.
        assert!(body.contains(r#"command = "/opt/cargo/bin/makakoo-mcp""#));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let outcome = sync(&path, &spec(), true, None);
        assert_eq!(outcome, SyncOutcome::WouldChange { kind: ChangeKind::Add });
        assert!(!path.exists());
    }

    #[test]
    fn malformed_toml_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "not = valid = toml [[[").unwrap();
        let outcome = sync(&path, &spec(), false, None);
        assert!(matches!(outcome, SyncOutcome::Error { .. }));
    }

    #[test]
    fn round_trip_preserves_unrelated_sections() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"[features]
sandbox = true

[ui]
theme = "dark"
"#,
        )
        .unwrap();
        let _ = sync(&path, &spec(), false, None);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("[features]"));
        assert!(body.contains("sandbox = true"));
        assert!(body.contains("[ui]"));
        assert!(body.contains(r#"theme = "dark""#));
        assert!(body.contains("[mcp_servers.harvey]"));
    }
}
