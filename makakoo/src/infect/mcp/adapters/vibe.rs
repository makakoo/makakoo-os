//! Vibe TOML adapter — `[[mcp_servers]]` array-of-tables format.
//!
//! Vibe stores MCP servers as an array of tables, discriminated by a
//! `transport` field. Each entry is matched on its `name` key. We
//! always write `transport = "stdio"` for harvey and add the prompt
//! hint that vibe surfaces as a tool description suffix.
//!
//! Sibling array entries (other transports/servers) are preserved.

use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

use crate::infect::mcp::{ChangeKind, McpServerSpec, SyncOutcome};

const SERVER_NAME: &str = "harvey";

pub fn sync(path: &Path, spec: &McpServerSpec, dry_run: bool) -> SyncOutcome {
    let mut doc = match read_doc(path) {
        Ok(d) => d,
        Err(e) => {
            return SyncOutcome::Error {
                message: format!("read {}: {e}", path.display()),
            }
        }
    };

    let before = render_harvey(&doc);
    upsert_harvey(&mut doc, spec);
    let after = render_harvey(&doc);

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

fn read_doc(path: &Path) -> std::io::Result<DocumentMut> {
    let body = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    body.parse::<DocumentMut>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// Stringify the existing harvey array-entry for "did anything change?"
/// comparison. Walks `[[mcp_servers]]` looking for the entry where
/// `name = "harvey"`.
fn render_harvey(doc: &DocumentMut) -> Option<String> {
    let item = doc.get("mcp_servers")?;
    let arr = item.as_array_of_tables()?;
    for table in arr.iter() {
        if let Some(name) = table.get("name").and_then(|n| n.as_str()) {
            if name == SERVER_NAME {
                return Some(table.to_string());
            }
        }
    }
    None
}

/// Insert/replace the harvey entry inside `[[mcp_servers]]`, creating
/// the array if needed. Other entries are preserved untouched.
fn upsert_harvey(doc: &mut DocumentMut, spec: &McpServerSpec) {
    if doc.get("mcp_servers").is_none() {
        doc["mcp_servers"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let arr = doc["mcp_servers"]
        .as_array_of_tables_mut()
        .expect("mcp_servers must be array-of-tables for vibe");

    // Find existing harvey index, if any.
    let idx = (0..arr.len()).find(|i| {
        arr.get(*i)
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            == Some(SERVER_NAME)
    });

    let table = build_harvey_table(spec);

    match idx {
        Some(i) => {
            // Replace in-place by clearing + repopulating the table at
            // index `i`. `ArrayOfTables::get_mut` returns `Option<&mut Table>`;
            // mutating it preserves position in the surrounding array.
            if let Some(slot) = arr.get_mut(i) {
                slot.clear();
                for (k, v) in table.iter() {
                    slot.insert(k, v.clone());
                }
            }
        }
        None => arr.push(table),
    }
}

fn build_harvey_table(spec: &McpServerSpec) -> Table {
    let mut t = Table::new();
    t.insert("transport", value("stdio"));
    t.insert("name", value(SERVER_NAME));
    t.insert("command", value(spec.command.as_str()));
    let mut args = Array::new();
    for a in &spec.args {
        args.push(a.as_str());
    }
    t.insert("args", value(args));
    if let Some(prompt) = &spec.prompt {
        t.insert("prompt", value(prompt.as_str()));
    }

    // Nested env subtable. Use a dotted-key style inline so the
    // resulting toml has [mcp_servers.env] under the array entry.
    let mut env = Table::new();
    env.set_implicit(false);
    for (k, v) in &spec.env {
        env.insert(k.as_str(), value(v.as_str()));
    }
    t.insert("env", Item::Table(env));
    t
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
            prompt: Some("desc hint".to_string()),
        }
    }

    #[test]
    fn add_to_empty_file_creates_array_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let outcome = sync(&path, &spec(), false);
        assert_eq!(outcome, SyncOutcome::Added);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("[[mcp_servers]]"));
        assert!(body.contains(r#"name = "harvey""#));
        assert!(body.contains(r#"transport = "stdio""#));
        assert!(body.contains(r#"command = "/opt/cargo/bin/makakoo-mcp""#));
        assert!(body.contains(r#"MAKAKOO_HOME = "/h""#));
    }

    #[test]
    fn add_into_existing_array_preserves_other_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"[[mcp_servers]]
transport = "stdio"
name = "other"
command = "/usr/local/bin/other-mcp"

[mcp_servers.env]
FOO = "bar"
"#,
        )
        .unwrap();

        let outcome = sync(&path, &spec(), false);
        assert_eq!(outcome, SyncOutcome::Added);
        let body = fs::read_to_string(&path).unwrap();

        // Both entries present.
        let count = body.matches("[[mcp_servers]]").count();
        assert_eq!(count, 2, "should have 2 [[mcp_servers]] entries; got: {body}");
        assert!(body.contains(r#"name = "other""#));
        assert!(body.contains(r#"name = "harvey""#));
        assert!(body.contains(r#"FOO = "bar""#));
    }

    #[test]
    fn second_run_is_unchanged() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let _ = sync(&path, &spec(), false);
        let outcome = sync(&path, &spec(), false);
        assert_eq!(outcome, SyncOutcome::Unchanged);
    }

    #[test]
    fn stale_command_path_triggers_update() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"[[mcp_servers]]
transport = "stdio"
name = "harvey"
command = "/old/path/makakoo-mcp"
args = []

[mcp_servers.env]
MAKAKOO_HOME = "/h"
"#,
        )
        .unwrap();
        let outcome = sync(&path, &spec(), false);
        assert_eq!(outcome, SyncOutcome::Updated);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(r#"command = "/opt/cargo/bin/makakoo-mcp""#));
        assert!(!body.contains("/old/path"));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let outcome = sync(&path, &spec(), true);
        assert_eq!(outcome, SyncOutcome::WouldChange { kind: ChangeKind::Add });
        assert!(!path.exists());
    }

    #[test]
    fn round_trip_preserves_unrelated_top_level_keys() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"active_model = "glm-5"
api_timeout = 3000.0

[[providers]]
name = "ail"
api_base = "http://localhost:18080/v1"
"#,
        )
        .unwrap();

        let _ = sync(&path, &spec(), false);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(r#"active_model = "glm-5""#));
        assert!(body.contains("api_timeout = 3000"));
        assert!(body.contains("[[providers]]"));
        assert!(body.contains(r#"name = "ail""#));
        assert!(body.contains("[[mcp_servers]]"));
    }

    #[test]
    fn malformed_toml_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "not = valid = toml [[[").unwrap();
        let outcome = sync(&path, &spec(), false);
        assert!(matches!(outcome, SyncOutcome::Error { .. }));
    }

    #[test]
    fn writes_prompt_hint_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let _ = sync(&path, &spec(), false);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(r#"prompt = "desc hint""#));
    }
}
