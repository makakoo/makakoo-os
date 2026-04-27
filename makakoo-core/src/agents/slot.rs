//! Agent slot — the canonical TOML registry record for one
//! Makakoo subagent.
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS Q4: every slot lives at
//! `~/MAKAKOO/config/agents/<slot_id>.toml`.  No second registry
//! under `~/MAKAKOO/agents/` (the existing `AgentScaffold` path
//! is legacy and stays for backward compatibility but does NOT
//! double as the slot registry).
//!
//! Schema fields mirror the locked Q9 example: slot-level
//! identity / scope / persona, plus a `[[transport]]` array (zero
//! to many) of `TransportEntry` blocks.  `allowed_users` is
//! per-transport only (Q7 simplified — no slot-level superset).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::transport::config::{validate_transport_list, TransportEntry};
use crate::{MakakooError, Result};

/// One agent slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSlot {
    /// Slot identifier — must be ASCII alphanumeric + hyphen +
    /// underscore, 1–64 chars, and MUST equal the TOML filename
    /// stem (`<slot_id>.toml`).
    pub slot_id: String,

    /// Display name. Shown in `makakoo agent list` Name column,
    /// in Telegram avatars, etc.
    #[serde(default)]
    pub name: String,

    /// Per-agent persona snippet that rides on top of the
    /// canonical bootstrap.  `None` (or omitted) inherits the
    /// `HARVEY_SYSTEM_PROMPT` fallback (used by the migrated
    /// harveychat slot).
    #[serde(default)]
    pub persona: Option<String>,

    /// Whether the slot inherits the baseline tool surface.  When
    /// `true`, `tools` is additive on top of baseline; when
    /// `false`, only `tools` are exposed (Q6 least-privilege).
    #[serde(default)]
    pub inherit_baseline: bool,

    /// Allowed filesystem read/write paths (the per-agent scope).
    #[serde(default)]
    pub allowed_paths: Vec<String>,

    /// Forbidden paths — additive override on top of `allowed_paths`.
    #[serde(default)]
    pub forbidden_paths: Vec<String>,

    /// Whitelist of tool names this slot may invoke.
    #[serde(default)]
    pub tools: Vec<String>,

    /// Process model.  v1 only `"supervised_pair"` (one Rust
    /// transport runtime + one Python gateway, supervised by
    /// LaunchAgent/systemd).
    #[serde(default = "default_process_mode")]
    pub process_mode: String,

    /// Zero-or-more chat transports attached to this slot.
    #[serde(default, rename = "transport")]
    pub transports: Vec<TransportEntry>,

    /// Phase 4 of v2-mega: per-slot LLM override. Locked Q4 schema:
    ///
    /// ```toml
    /// [llm.override]
    /// model            = "claude-opus-4-7"
    /// max_tokens       = 8192
    /// temperature      = 0.7
    /// reasoning_effort = "medium"
    /// ```
    ///
    /// Resolution: per-call args > slot.toml [llm.override] > makakoo
    /// system defaults.
    #[serde(default, rename = "llm")]
    pub llm: Option<LlmSection>,
}

/// Container that wraps the `[llm.inherit]` (docs-only) and
/// `[llm.override]` sections so TOML parsing matches the locked
/// schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmSection {
    #[serde(default)]
    pub inherit: Option<crate::agents::llm_override::LlmInherit>,
    #[serde(default, rename = "override")]
    pub overrides: Option<crate::agents::llm_override::LlmOverride>,
}

impl LlmSection {
    /// Convenience accessor: returns the override block if either
    /// the section itself or the inner override is `Some` and
    /// non-empty.
    pub fn effective_override(&self) -> Option<crate::agents::llm_override::LlmOverride> {
        let over = self.overrides.clone()?;
        if over.is_empty() {
            None
        } else {
            Some(over)
        }
    }
}

fn default_process_mode() -> String {
    "supervised_pair".into()
}

impl AgentSlot {
    /// Schema-level validation. No I/O, no network.
    pub fn validate(&self) -> Result<()> {
        validate_slot_id(&self.slot_id)?;
        if self.process_mode != "supervised_pair" {
            return Err(MakakooError::InvalidInput(format!(
                "slot '{}': only process_mode = \"supervised_pair\" is supported in v1",
                self.slot_id
            )));
        }
        validate_transport_list(&self.transports)?;
        Ok(())
    }

    /// Returns `true` if the slot has at least one enabled
    /// `[[transport]]` block.  `makakoo agent list` reports
    /// `UNCONFIGURED` for slots that fail this check.
    pub fn is_configured(&self) -> bool {
        self.transports.iter().any(|t| t.enabled)
    }

    /// Convenience: list of `(transport_id, kind)` pairs sorted
    /// by transport_id.  Used by `agent list` / `agent show`.
    pub fn transport_summary(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .transports
            .iter()
            .map(|t| (t.id.clone(), t.kind.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// `agent show` redacts every field that could carry a
    /// credential.  This produces a deep-copy with the secret
    /// fields cleared so the caller can pretty-print without
    /// leaking.
    pub fn redacted(&self) -> AgentSlot {
        let mut clone = self.clone();
        for t in clone.transports.iter_mut() {
            t.secret_ref = t.secret_ref.as_ref().map(|_| "<redacted>".into());
            t.secret_env = t.secret_env.as_ref().map(|_| "<redacted>".into());
            t.inline_secret_dev = t
                .inline_secret_dev
                .as_ref()
                .map(|_| "<redacted>".into());
            t.app_token_ref = t.app_token_ref.as_ref().map(|_| "<redacted>".into());
            t.app_token_env = t.app_token_env.as_ref().map(|_| "<redacted>".into());
            t.inline_app_token_dev = t
                .inline_app_token_dev
                .as_ref()
                .map(|_| "<redacted>".into());
        }
        clone
    }

    /// Parse a slot from a TOML file. Validates the schema and
    /// asserts that the filename stem matches `slot_id`.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let slot: Self = toml::from_str(&raw).map_err(|e| {
            MakakooError::Config(format!("agent slot {} parse: {}", path.display(), e))
        })?;
        slot.validate()?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                MakakooError::InvalidInput(format!(
                    "agent slot path '{}' has no filename stem",
                    path.display()
                ))
            })?;
        if stem != slot.slot_id {
            return Err(MakakooError::InvalidInput(format!(
                "agent slot filename '{}' must equal slot_id '{}' (filename and slot_id must match)",
                stem, slot.slot_id
            )));
        }
        Ok(slot)
    }
}

/// `slot_id` must be ASCII letters / digits / `-` / `_`, 1–64 chars.
pub fn validate_slot_id(slot_id: &str) -> Result<()> {
    if slot_id.is_empty() {
        return Err(MakakooError::InvalidInput(
            "slot_id must not be empty".into(),
        ));
    }
    if slot_id.len() > 64 {
        return Err(MakakooError::InvalidInput(format!(
            "slot_id '{}' exceeds 64 chars",
            slot_id
        )));
    }
    for c in slot_id.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(MakakooError::InvalidInput(format!(
                "slot_id '{}' contains invalid char '{}'; allowed: a-z A-Z 0-9 - _",
                slot_id, c
            )));
        }
    }
    Ok(())
}

/// Canonical registry directory: `<makakoo_home>/config/agents`.
pub fn registry_dir(makakoo_home: &Path) -> PathBuf {
    makakoo_home.join("config").join("agents")
}

/// Canonical TOML path for a slot: `<makakoo_home>/config/agents/<slot_id>.toml`.
pub fn slot_path(makakoo_home: &Path, slot_id: &str) -> PathBuf {
    registry_dir(makakoo_home).join(format!("{}.toml", slot_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::config::{TelegramConfig, TransportConfig};

    fn telegram_block(id: &str) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "telegram".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: Some("123:abc".into()),
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: vec!["746496145".into()],
            config: TransportConfig::Telegram(TelegramConfig::default()),
        }
    }

    fn slot(id: &str) -> AgentSlot {
        AgentSlot {
            slot_id: id.into(),
            name: "Test".into(),
            persona: None,
            inherit_baseline: true,
            allowed_paths: vec![],
            forbidden_paths: vec![],
            tools: vec![],
            process_mode: default_process_mode(),
            transports: vec![telegram_block("telegram-main")],
            llm: None,
        }
    }

    #[test]
    fn slot_id_alphanumeric_required() {
        assert!(validate_slot_id("harveychat").is_ok());
        assert!(validate_slot_id("agent-1").is_ok());
        assert!(validate_slot_id("agent_1").is_ok());
        assert!(validate_slot_id("agent.1").is_err());
        assert!(validate_slot_id("agent 1").is_err());
        assert!(validate_slot_id("").is_err());
    }

    #[test]
    fn slot_id_length_capped() {
        let too_long = "a".repeat(65);
        assert!(validate_slot_id(&too_long).is_err());
    }

    #[test]
    fn validate_passes_minimum_slot() {
        slot("harveychat").validate().unwrap();
    }

    #[test]
    fn validate_rejects_unsupported_process_mode() {
        let mut s = slot("harveychat");
        s.process_mode = "multiplexed".into();
        let err = s.validate().unwrap_err();
        assert!(format!("{err}").contains("supervised_pair"));
    }

    #[test]
    fn is_configured_false_without_enabled_transport() {
        let mut s = slot("harveychat");
        s.transports.iter_mut().for_each(|t| t.enabled = false);
        assert!(!s.is_configured());
    }

    #[test]
    fn redacted_clears_secret_fields() {
        let s = slot("harveychat");
        let r = s.redacted();
        for t in &r.transports {
            assert!(t.inline_secret_dev.as_deref() == Some("<redacted>"));
        }
    }

    #[test]
    fn load_from_file_rejects_filename_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-harveychat.toml");
        let raw = r#"
slot_id = "harveychat"
name = "Olibia"
process_mode = "supervised_pair"

[[transport]]
id = "telegram-main"
kind = "telegram"
enabled = true
inline_secret_dev = "123:abc"
allowed_users = ["1"]

[transport.config]
"#;
        std::fs::write(&path, raw).unwrap();
        let err = AgentSlot::load_from_file(&path).unwrap_err();
        assert!(format!("{err}").contains("filename"));
    }

    #[test]
    fn load_from_file_round_trip_with_telegram_and_slack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secretary.toml");
        let raw = r#"
slot_id = "secretary"
name = "Secretary"
persona = "Sharp professional secretary"
inherit_baseline = false
allowed_paths = ["~/MAKAKOO/data/secretary/"]
forbidden_paths = ["~/CV/"]
tools = ["email", "calendar"]
process_mode = "supervised_pair"

[[transport]]
id = "telegram-main"
kind = "telegram"
enabled = true
secret_ref = "agent/secretary/telegram-main/bot_token"
secret_env = "SECRETARY_TELEGRAM_MAIN_TOKEN"
allowed_users = ["746496145"]

[transport.config]
polling_timeout_seconds = 30
allowed_chat_ids = ["746496145"]
support_thread = true

[[transport]]
id = "slack-main"
kind = "slack"
enabled = true
secret_ref = "agent/secretary/slack-main/bot_token"
app_token_ref = "agent/secretary/slack-main/app_token"
allowed_users = ["U0123ABCD"]

[transport.config]
team_id = "T0123ABCD"
mode = "socket"
dm_only = true
support_thread = true
"#;
        std::fs::write(&path, raw).unwrap();
        let slot = AgentSlot::load_from_file(&path).unwrap();
        assert_eq!(slot.slot_id, "secretary");
        assert_eq!(slot.transports.len(), 2);
        assert_eq!(slot.transport_summary().len(), 2);
        assert!(slot.is_configured());
    }

    #[test]
    fn slot_path_layout() {
        let p = slot_path(Path::new("/tmp/makakoo"), "harveychat");
        assert_eq!(
            p,
            PathBuf::from("/tmp/makakoo/config/agents/harveychat.toml")
        );
    }
}
