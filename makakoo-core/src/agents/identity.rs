//! Per-agent identity resolution.
//!
//! Phase 3 deliverable.  Reads `MAKAKOO_AGENT_SLOT` from the
//! process environment (or an explicit `--slot` override) and
//! loads the matching `~/MAKAKOO/config/agents/<slot>.toml`.
//!
//! Locked exit semantics: if the env var is set but no TOML file
//! matches, the gateway/CLI exits with code 64 (UNIX `EX_USAGE`)
//! and the operator-facing message documented in the Phase 2
//! criteria. The exit-code semantics are exposed via a
//! `Result<AgentIdentity, IdentityError>` that callers can map to
//! `process::exit(64)`.
//!
//! `MAKAKOO_AGENT_SLOT` is the ONLY runtime slot env var (Q3
//! locked — `AGENT_SLOT_ID` is rejected).

use std::path::Path;

use thiserror::Error;

use crate::agents::slot::{slot_path, AgentSlot};

/// Canonical env var name (Q3 locked).
pub const ENV_VAR: &str = "MAKAKOO_AGENT_SLOT";

/// Locked exit code for "unknown agent slot".
pub const EX_USAGE: i32 = 64;

/// The resolved identity carried by a running gateway process or
/// CLI invocation.  Held in the existing `harvey_agent_id`
/// `ContextVar` (Python) and equivalent `tracing` span fields
/// (Rust) so structured logs always carry the agent id.
#[derive(Debug, Clone)]
pub struct AgentIdentity {
    pub slot_id: String,
    pub slot: AgentSlot,
}

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("environment variable `MAKAKOO_AGENT_SLOT` is not set")]
    EnvVarMissing,

    #[error(
        "Agent slot '{slot_id}' not found at \
         {makakoo_home}/config/agents/{slot_id}.toml. Run \
         'makakoo agent create {slot_id}' to create it."
    )]
    SlotNotFound {
        slot_id: String,
        makakoo_home: String,
    },

    #[error("agent slot '{slot_id}' parse failed: {source}")]
    SlotParse {
        slot_id: String,
        #[source]
        source: crate::MakakooError,
    },
}

impl IdentityError {
    /// Map every error variant to the locked exit code.  All
    /// startup-time identity failures terminate with `EX_USAGE`
    /// (64) so the supervisor can distinguish identity errors
    /// from runtime crashes.
    pub fn exit_code(&self) -> i32 {
        EX_USAGE
    }
}

/// Resolve `MAKAKOO_AGENT_SLOT` from the process environment.
/// Returns `IdentityError::EnvVarMissing` when unset.
pub fn slot_from_env() -> Result<String, IdentityError> {
    std::env::var(ENV_VAR).map_err(|_| IdentityError::EnvVarMissing)
}

/// Load the slot identity for the given `(makakoo_home, slot_id)`
/// pair.  Phase 2 `AgentSlot::load_from_file` does the schema
/// validation; this just translates a `not found` into the
/// locked structured error.
pub fn load_identity(makakoo_home: &Path, slot_id: &str) -> Result<AgentIdentity, IdentityError> {
    let path = slot_path(makakoo_home, slot_id);
    if !path.exists() {
        return Err(IdentityError::SlotNotFound {
            slot_id: slot_id.to_string(),
            makakoo_home: makakoo_home.display().to_string(),
        });
    }
    let slot = AgentSlot::load_from_file(&path).map_err(|e| IdentityError::SlotParse {
        slot_id: slot_id.to_string(),
        source: e,
    })?;
    Ok(AgentIdentity {
        slot_id: slot.slot_id.clone(),
        slot,
    })
}

/// Convenience: resolve env var → load identity in one call.
/// Use the `slot_override` arg for the `--slot <id>` CLI flag,
/// which wins over the env var (locked Q3 semantics).
pub fn resolve(
    makakoo_home: &Path,
    slot_override: Option<&str>,
) -> Result<AgentIdentity, IdentityError> {
    let slot_id = match slot_override {
        Some(s) => s.to_string(),
        None => slot_from_env()?,
    };
    load_identity(makakoo_home, &slot_id)
}

/// Render the per-agent identity block that goes at the top of
/// the system prompt, after the canonical bootstrap and before
/// the persona snippet.  Locked phrasing per Phase 3 criteria.
///
/// `transport_kind` is the kind of the transport the inbound
/// message arrived through (e.g. `"telegram"`, `"slack"`); when
/// the gateway doesn't yet know (e.g. on the first dispatch
/// call), pass `None` and the rendering omits the "arrived via"
/// sentence.
pub fn render_identity_block(identity: &AgentIdentity, transport_kind: Option<&str>) -> String {
    let name = if identity.slot.name.is_empty() {
        identity.slot_id.as_str()
    } else {
        identity.slot.name.as_str()
    };
    let tools = if identity.slot.tools.is_empty() {
        "(baseline)".to_string()
    } else {
        identity.slot.tools.join(", ")
    };
    let paths = if identity.slot.allowed_paths.is_empty() {
        // Empty `allowed_paths` is denied by `check_path` per Q6
        // (least-privilege).  Mirror that wording here so the LLM
        // doesn't read the prompt as "you can write anywhere"
        // when the runtime check rejects every path.
        "(none — least-privilege default)".to_string()
    } else {
        identity.slot.allowed_paths.join(", ")
    };
    let mut block = format!(
        "You are {name}. Your slot id is {slot}.",
        name = name,
        slot = identity.slot_id
    );
    if let Some(tk) = transport_kind {
        block.push_str(&format!(" This message arrived via {}.", tk));
    }
    block.push_str(&format!(
        " Your allowed tools are {tools}. Your allowed paths are {paths}.",
        tools = tools,
        paths = paths
    ));
    block
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::AgentRegistry;
    use crate::transport::config::{TelegramConfig, TransportConfig, TransportEntry};

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
            allowed_users: vec!["1".into()],
            config: TransportConfig::Telegram(TelegramConfig::default()),
        }
    }

    fn slot(id: &str) -> AgentSlot {
        AgentSlot {
            slot_id: id.into(),
            name: "Olibia".into(),
            persona: None,
            inherit_baseline: true,
            allowed_paths: vec!["~/MAKAKOO/data/harveychat/".into()],
            forbidden_paths: vec![],
            tools: vec!["brain_search".into(), "write_file".into()],
            process_mode: "supervised_pair".into(),
            transports: vec![telegram_block("telegram-main")],
            llm: None,
        }
    }

    #[test]
    fn slot_not_found_uses_locked_exit_code_and_message() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_identity(dir.path(), "nonexistent").unwrap_err();
        assert_eq!(err.exit_code(), EX_USAGE);
        assert_eq!(err.exit_code(), 64);
        let msg = format!("{err}");
        assert!(
            msg.contains("Run 'makakoo agent create nonexistent' to create it."),
            "missing locked CTA in message: {msg}"
        );
        assert!(msg.contains("config/agents/nonexistent.toml"));
    }

    #[test]
    fn load_identity_reads_existing_slot() {
        let dir = tempfile::tempdir().unwrap();
        AgentRegistry::create(dir.path(), &slot("harveychat")).unwrap();
        let id = load_identity(dir.path(), "harveychat").unwrap();
        assert_eq!(id.slot_id, "harveychat");
        assert_eq!(id.slot.name, "Olibia");
    }

    #[test]
    fn slot_from_env_errors_when_unset() {
        let _guard = crate::test_lock::lock_env();
        std::env::remove_var(ENV_VAR);
        assert!(matches!(
            slot_from_env(),
            Err(IdentityError::EnvVarMissing)
        ));
    }

    #[test]
    fn slot_from_env_returns_value() {
        let _guard = crate::test_lock::lock_env();
        std::env::set_var(ENV_VAR, "harveychat");
        let s = slot_from_env().unwrap();
        assert_eq!(s, "harveychat");
        std::env::remove_var(ENV_VAR);
    }

    #[test]
    fn resolve_prefers_override_over_env() {
        let _guard = crate::test_lock::lock_env();
        let dir = tempfile::tempdir().unwrap();
        AgentRegistry::create(dir.path(), &slot("from-cli")).unwrap();
        AgentRegistry::create(dir.path(), &slot("from-env")).unwrap();
        std::env::set_var(ENV_VAR, "from-env");
        let id = resolve(dir.path(), Some("from-cli")).unwrap();
        assert_eq!(id.slot_id, "from-cli");
        std::env::remove_var(ENV_VAR);
    }

    #[test]
    fn render_identity_block_includes_required_fields() {
        let id = AgentIdentity {
            slot_id: "harveychat".into(),
            slot: slot("harveychat"),
        };
        let block = render_identity_block(&id, Some("telegram"));
        assert!(block.contains("You are Olibia."));
        assert!(block.contains("Your slot id is harveychat."));
        assert!(block.contains("This message arrived via telegram."));
        assert!(block.contains("Your allowed tools are brain_search, write_file."));
        assert!(block.contains("Your allowed paths are ~/MAKAKOO/data/harveychat/."));
    }

    #[test]
    fn render_identity_block_omits_transport_when_none() {
        let id = AgentIdentity {
            slot_id: "secretary".into(),
            slot: slot("secretary"),
        };
        let block = render_identity_block(&id, None);
        assert!(!block.contains("arrived via"));
    }

    #[test]
    fn render_identity_block_falls_back_to_slot_id_when_name_empty() {
        let mut s = slot("secretary");
        s.name = String::new();
        let id = AgentIdentity {
            slot_id: s.slot_id.clone(),
            slot: s,
        };
        let block = render_identity_block(&id, None);
        assert!(block.starts_with("You are secretary."));
    }

    #[test]
    fn render_identity_block_handles_empty_tools_and_paths() {
        let mut s = slot("test");
        s.tools = vec![];
        s.allowed_paths = vec![];
        let id = AgentIdentity {
            slot_id: s.slot_id.clone(),
            slot: s,
        };
        let block = render_identity_block(&id, None);
        assert!(block.contains("Your allowed tools are (baseline)."));
        assert!(block.contains("Your allowed paths are (none — least-privilege default)."));
    }
}
