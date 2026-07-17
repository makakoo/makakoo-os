//! Plain-TOML `[mcp_servers.<name>]` adapter — the "grok" schema.
//!
//! Same `[mcp_servers.X]` inline-table family as Codex, but WITHOUT
//! Codex's two idiosyncrasies:
//!   * no `env_vars` secret-forwarding list — Grok has no such mechanism,
//!     so we don't emit it,
//!   * no `model_instructions_file` — Grok discovers `~/.grok/AGENTS.md`
//!     natively, so the bootstrap pointer already reaches it.
//!
//! What we DO emit mirrors Grok's documented schema: `command`, `args`,
//! `enabled = true`, `description`, and a nested `[mcp_servers.<name>.env]`
//! table. This is the generic primitive any TOML-mcp CLI that follows the
//! grok/modelcontextprotocol convention can reuse via a `cli_hosts.json`
//! entry — no new code per host.
//!
//! Like every adapter we upsert ONLY the `harvey` server and preserve all
//! other tables (peer servers, `[marketplace]`, user keys) byte-for-byte
//! via `toml_edit`.

use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::infect::mcp::{ChangeKind, McpServerSpec, SyncOutcome};

const SERVER_KEY: &str = "harvey";

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

/// Canonical string of the managed `[mcp_servers.harvey]` fields, for the
/// "did anything change?" short-circuit. We compare only fields we own so
/// a user hand-adding `startup_timeout_sec` doesn't force a rewrite.
fn render_harvey(doc: &DocumentMut) -> Option<String> {
    let h = doc.get("mcp_servers")?.as_table()?.get(SERVER_KEY)?.as_table()?;
    let command = h.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let args = h
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(","))
        .unwrap_or_default();
    let description = h.get("description").and_then(|v| v.as_str()).unwrap_or("");
    let enabled = h.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let mut env_pairs: Vec<String> = Vec::new();
    if let Some(env) = h.get("env").and_then(|v| v.as_table()) {
        for (k, v) in env.iter() {
            if let Some(val) = v.as_str() {
                env_pairs.push(format!("{k}={val}"));
            }
        }
    }
    env_pairs.sort();
    Some(format!(
        "cmd={command}|args={args}|desc={description}|enabled={enabled}|env={}",
        env_pairs.join(";")
    ))
}

/// Insert/replace `[mcp_servers.harvey]` + `[mcp_servers.harvey.env]`,
/// preserving peer servers and any user-added keys on the harvey table.
fn upsert_harvey(doc: &mut DocumentMut, spec: &McpServerSpec) {
    if doc.get("mcp_servers").is_none() {
        doc["mcp_servers"] = Item::Table(Table::new());
    }
    let parent = doc["mcp_servers"]
        .as_table_mut()
        .expect("mcp_servers must be a table");
    parent.set_implicit(true);

    if parent.get(SERVER_KEY).is_none() {
        parent.insert(SERVER_KEY, Item::Table(Table::new()));
    }
    let harvey = parent
        .get_mut(SERVER_KEY)
        .and_then(|i| i.as_table_mut())
        .expect("harvey entry must be a table");

    harvey.insert("command", value(spec.command.as_str()));
    let mut args = Array::new();
    for a in &spec.args {
        args.push(a.as_str());
    }
    harvey.insert("args", value(args));
    harvey.insert("enabled", value(true));

    if let Some(desc) = &spec.prompt {
        harvey.insert("description", value(desc.as_str()));
    } else {
        harvey.remove("description");
    }

    // Replace the whole env subtable so removed vars don't linger.
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
        McpServerSpec {
            name: "harvey".to_string(),
            command: "/opt/makakoo-mcp".to_string(),
            args: vec![],
            env,
            forward_env: vec!["AIL_API_KEY".to_string()],
            prompt: Some("desc".to_string()),
        }
    }

    #[test]
    fn add_to_empty_creates_grok_shape() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(sync(&path, &spec(), false), SyncOutcome::Added);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("[mcp_servers.harvey]"));
        assert!(body.contains(r#"command = "/opt/makakoo-mcp""#));
        assert!(body.contains("enabled = true"));
        assert!(body.contains("[mcp_servers.harvey.env]"));
        assert!(body.contains(r#"MAKAKOO_HOME = "/h""#));
        // Grok schema has NO env_vars forwarding list.
        assert!(!body.contains("env_vars"));
    }

    #[test]
    fn second_run_unchanged() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let _ = sync(&path, &spec(), false);
        assert_eq!(sync(&path, &spec(), false), SyncOutcome::Unchanged);
    }

    #[test]
    fn preexisting_native_grok_block_is_unchanged() {
        // The hand-added block already on Sebastian's machine must not
        // churn on the first managed run.
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"[cli]
installer = "internal"

[mcp_servers.harvey]
command = "/opt/makakoo-mcp"
args = []
description = "desc"
enabled = true

[mcp_servers.harvey.env]
HARVEY_HOME = "/h"
MAKAKOO_HOME = "/h"

[marketplace]
official_marketplace_auto_installed = true
"#,
        )
        .unwrap();
        assert_eq!(sync(&path, &spec(), false), SyncOutcome::Unchanged);
    }

    #[test]
    fn stale_command_triggers_update_and_preserves_peers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"[mcp_servers.other]
command = "/usr/bin/other"

[mcp_servers.harvey]
command = "python3"
args = ["/old.py"]

[mcp_servers.harvey.env]
HARVEY_HOME = "/h"

[marketplace]
foo = true
"#,
        )
        .unwrap();
        assert_eq!(sync(&path, &spec(), false), SyncOutcome::Updated);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(r#"command = "/opt/makakoo-mcp""#));
        assert!(!body.contains("python3"));
        assert!(body.contains("[mcp_servers.other]"));
        assert!(body.contains("[marketplace]"));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(
            sync(&path, &spec(), true),
            SyncOutcome::WouldChange { kind: ChangeKind::Add }
        );
        assert!(!path.exists());
    }

    #[test]
    fn malformed_toml_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "bad = = =").unwrap();
        assert!(matches!(sync(&path, &spec(), false), SyncOutcome::Error { .. }));
    }
}
