//! Runtime-registered custom CLI hosts.
//!
//! The built-in host roster (`slots::SLOTS` for bootstrap, `mcp::McpTarget`
//! for MCP) is compiled into the binary. This module lets a user register
//! ADDITIONAL AI CLIs at runtime with `makakoo cli add <name>`, persisted
//! to `$MAKAKOO_HOME/config/cli_hosts.json`, and merged into every
//! `makakoo infect` run — no recompile.
//!
//! A custom host is pure data: a markdown bootstrap slot (path) and,
//! optionally, an MCP config (path + one of the format primitives the
//! adapters already implement). The bootstrap slot is always
//! markdown-with-markers — the universal `AGENTS.md`/`CLAUDE.md`/`GEMINI.md`
//! instruction-file convention every agent CLI except OpenCode follows,
//! and OpenCode is already a built-in.
//!
//! Merge policy: a custom host whose `name` collides with a built-in slot
//! is dropped on load (built-ins win) so the registry can never shadow or
//! double-write a first-class host.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::slots::SLOTS;
use super::writer::{self, SlotStatus, SlotWriteResult};
use crate::infect::mcp::McpFormat;

/// Registry location under `$MAKAKOO_HOME`.
pub const REGISTRY_REL: &str = "config/cli_hosts.json";

/// MCP config primitive a custom host declares. Maps 1:1 onto the
/// adapters in `mcp::adapters`. Kebab-case on the wire so a registry
/// entry reads `"mcp_format": "toml-simple"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CustomMcpFormat {
    /// `{ "mcpServers": { "harvey": {...} } }` — Claude/Gemini/Qwen/Cursor.
    JsonMcpServers,
    /// OpenCode's `{ "mcp": { "harvey": {...} } }`.
    JsonOpencode,
    /// Codex-style `[mcp_servers.harvey]` with `env_vars` forwarding.
    TomlCodex,
    /// Vibe's `[[mcp_servers]]` array-of-tables.
    TomlVibe,
    /// Plain `[mcp_servers.harvey]` with `enabled`/`env` — the Grok schema.
    TomlSimple,
}

/// One runtime-registered CLI host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomHost {
    /// Short host name, e.g. `"grok"`. Must not collide with a built-in.
    pub name: String,
    /// Bootstrap slot path relative to `$HOME` (markdown), e.g.
    /// `".grok/AGENTS.md"`.
    pub bootstrap_path: String,
    /// Optional MCP config path relative to `$HOME`, e.g.
    /// `".grok/config.toml"`. Omit for bootstrap-only hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_path: Option<String>,
    /// MCP format primitive. Required iff `mcp_path` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_format: Option<CustomMcpFormat>,
    /// Binary name for detection. Defaults to `name` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
}

impl CustomHost {
    /// Translate the declared MCP format into the internal `McpFormat`
    /// the sync dispatch understands. `None` for bootstrap-only hosts.
    pub fn mcp_format_enum(&self) -> Option<McpFormat> {
        Some(match self.mcp_format? {
            CustomMcpFormat::JsonMcpServers => McpFormat::JsonMcpServers,
            CustomMcpFormat::JsonOpencode => McpFormat::JsonOpencode,
            CustomMcpFormat::TomlCodex => McpFormat::TomlInlineTable,
            CustomMcpFormat::TomlVibe => McpFormat::TomlArrayOfTables,
            CustomMcpFormat::TomlSimple => McpFormat::TomlSimple,
        })
    }

    /// Binary name used for detection — explicit `binary` or the host name.
    pub fn binary_name(&self) -> &str {
        self.binary.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Registry {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    hosts: Vec<CustomHost>,
}

fn default_version() -> u32 {
    1
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            version: 1,
            hosts: Vec::new(),
        }
    }
}

/// Absolute path to the custom-host registry under `$MAKAKOO_HOME`.
pub fn registry_path(makakoo_home: &Path) -> PathBuf {
    makakoo_home.join(REGISTRY_REL)
}

/// Load registered custom hosts, dropping any that collide with a
/// built-in slot name or have an empty name. Never panics: a missing or
/// malformed registry yields an empty list so infect keeps working.
pub fn load(makakoo_home: &Path) -> Vec<CustomHost> {
    let body = match std::fs::read_to_string(registry_path(makakoo_home)) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let reg: Registry = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let builtin: HashSet<&str> = SLOTS.iter().map(|s| s.name).collect();
    let mut seen: HashSet<String> = HashSet::new();
    reg.hosts
        .into_iter()
        .filter(|h| {
            !h.name.trim().is_empty()
                && !builtin.contains(h.name.as_str())
                && seen.insert(h.name.clone())
        })
        .collect()
}

/// Persist the full host list atomically. Used by `makakoo cli add/remove`.
pub fn save(makakoo_home: &Path, hosts: &[CustomHost]) -> std::io::Result<()> {
    let path = registry_path(makakoo_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let reg = Registry {
        version: 1,
        hosts: hosts.to_vec(),
    };
    let body = serde_json::to_string_pretty(&reg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{body}\n"))?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Write the bootstrap pointer into a custom host's markdown slot. Reuses
/// the shared marker-block upsert so idempotency + in-place version
/// upgrades work exactly as they do for built-in slots.
pub fn write_bootstrap(
    host: &CustomHost,
    home: &Path,
    pointer_body: &str,
    dry_run: bool,
) -> SlotWriteResult {
    let path = home.join(&host.bootstrap_path);
    let name: Cow<'static, str> = Cow::Owned(host.name.clone());
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let new_block = writer::render_markdown_block(pointer_body);
    let (new_text, status, prior_version) = writer::upsert_markdown_block(&existing, &new_block);

    if matches!(status, SlotStatus::Unchanged) || dry_run {
        let final_status = if dry_run && !matches!(status, SlotStatus::Unchanged) {
            SlotStatus::DryRun
        } else {
            status
        };
        return SlotWriteResult {
            slot_name: name,
            path,
            status: final_status,
            prior_version,
        };
    }

    match writer::atomic_write(&path, &new_text) {
        Ok(_) => SlotWriteResult {
            slot_name: name,
            path,
            status,
            prior_version,
        },
        Err(e) => SlotWriteResult {
            slot_name: name,
            path,
            status: SlotStatus::Error(format!("{e:#}")),
            prior_version,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn grok() -> CustomHost {
        CustomHost {
            name: "grok".into(),
            bootstrap_path: ".grok/AGENTS.md".into(),
            mcp_path: Some(".grok/config.toml".into()),
            mcp_format: Some(CustomMcpFormat::TomlSimple),
            binary: None,
        }
    }

    #[test]
    fn load_missing_registry_is_empty() {
        let dir = tempdir().unwrap();
        assert!(load(dir.path()).is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        save(dir.path(), &[grok()]).unwrap();
        let hosts = load(dir.path());
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "grok");
        assert_eq!(hosts[0].mcp_format_enum(), Some(McpFormat::TomlSimple));
        assert_eq!(hosts[0].binary_name(), "grok");
    }

    #[test]
    fn builtin_name_collision_is_dropped() {
        let dir = tempdir().unwrap();
        let mut clash = grok();
        clash.name = "claude".into(); // collides with a built-in slot
        save(dir.path(), &[clash, grok()]).unwrap();
        let hosts = load(dir.path());
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "grok");
    }

    #[test]
    fn corrupt_registry_yields_empty() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(registry_path(dir.path()), "{ not json").unwrap();
        assert!(load(dir.path()).is_empty());
    }

    #[test]
    fn write_bootstrap_upserts_marker_block() {
        let dir = tempdir().unwrap();
        let r = write_bootstrap(&grok(), dir.path(), "POINTER BODY", false);
        assert_eq!(r.slot_name, "grok");
        let body = std::fs::read_to_string(dir.path().join(".grok/AGENTS.md")).unwrap();
        assert!(body.contains("harvey:infect-global START"));
        assert!(body.contains("POINTER BODY"));
        // Second run is a no-op.
        let r2 = write_bootstrap(&grok(), dir.path(), "POINTER BODY", false);
        assert!(matches!(r2.status, SlotStatus::Unchanged));
    }

    #[test]
    fn bootstrap_only_host_has_no_mcp_format() {
        let h = CustomHost {
            name: "foo".into(),
            bootstrap_path: ".foo/AGENTS.md".into(),
            mcp_path: None,
            mcp_format: None,
            binary: None,
        };
        assert!(h.mcp_format_enum().is_none());
    }
}
