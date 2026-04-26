//! HarveyChat (Olibia) migration: legacy
//! `~/MAKAKOO/data/chat/config.json` → new
//! `~/MAKAKOO/config/agents/harveychat.toml`.
//!
//! Locked by SPRINT.md "Olibia migration (explicit)" section:
//!   - slot id is `harveychat` (NEVER `olibia` — Olibia is the
//!     display `name` only)
//!   - bot token preserved verbatim
//!   - allowed_users carry the legacy chat_id list
//!   - persona = null inherits HARVEY_SYSTEM_PROMPT (don't strip)
//!   - conversations.db archived (not merged) at
//!     `data/agents/harveychat/conversations.db.bak`
//!   - new per-agent DB starts empty at
//!     `data/agents/harveychat/conversations.db`
//!   - migration is IDEMPOTENT: re-running on an already-migrated
//!     slot is a no-op (returns `Ok(MigrationOutcome::AlreadyMigrated)`)

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::agents::registry::AgentRegistry;
use crate::agents::slot::{slot_path, AgentSlot};
use crate::transport::config::{TelegramConfig, TransportConfig, TransportEntry};
use crate::{MakakooError, Result};

/// Outcome of a migration run.
#[derive(Debug, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// First-time migration completed.
    Migrated {
        toml_path: PathBuf,
        archived_db: Option<PathBuf>,
    },
    /// Slot TOML already exists — no-op.
    AlreadyMigrated,
    /// Source `data/chat/config.json` does not exist — nothing to do.
    NothingToMigrate,
}

/// Legacy `data/chat/config.json` schema (only the fields we use).
#[derive(Debug, Deserialize)]
struct LegacyConfig {
    bot_token: String,
    #[serde(default)]
    allowlist: Vec<i64>,
}

/// Run the migration once. `makakoo_home` is the root of the
/// Makakoo workspace (`$MAKAKOO_HOME`).
pub fn migrate(makakoo_home: &Path) -> Result<MigrationOutcome> {
    let toml_path = slot_path(makakoo_home, "harveychat");
    if toml_path.exists() {
        return Ok(MigrationOutcome::AlreadyMigrated);
    }
    let legacy_path = makakoo_home.join("data").join("chat").join("config.json");
    if !legacy_path.exists() {
        return Ok(MigrationOutcome::NothingToMigrate);
    }
    let raw = std::fs::read_to_string(&legacy_path)?;
    let legacy: LegacyConfig = serde_json::from_str(&raw).map_err(|e| {
        MakakooError::Config(format!(
            "harveychat migration: parse legacy config {} failed: {}",
            legacy_path.display(),
            e
        ))
    })?;

    let allowed_users: Vec<String> = legacy
        .allowlist
        .iter()
        .map(|id| id.to_string())
        .collect();

    let transport = TransportEntry {
        id: "telegram-main".into(),
        kind: "telegram".into(),
        enabled: true,
        account_id: Some("@OlibiaBot".into()),
        secret_ref: Some("agent/harveychat/telegram-main/bot_token".into()),
        secret_env: Some("HARVEYCHAT_TELEGRAM_MAIN_TOKEN".into()),
        // Legacy token preserved as the inline dev fallback so the
        // migrated slot keeps working without the operator needing
        // to re-enter the secret. The secret_ref/secret_env fields
        // take precedence as soon as they're populated.
        inline_secret_dev: Some(legacy.bot_token.clone()),
        app_token_ref: None,
        app_token_env: None,
        inline_app_token_dev: None,
        allowed_users,
        config: TransportConfig::Telegram(TelegramConfig {
            polling_timeout_seconds: 30,
            allowed_chat_ids: legacy
                .allowlist
                .iter()
                .map(|id| id.to_string())
                .collect(),
            allowed_group_ids: vec![],
            support_thread: false,
        }),
    };

    let slot = AgentSlot {
        slot_id: "harveychat".into(),
        name: "Olibia".into(),
        // null persona inherits HARVEY_SYSTEM_PROMPT — preserves
        // the legacy bot's voice without copying the constant.
        persona: None,
        inherit_baseline: true,
        allowed_paths: vec![],
        forbidden_paths: vec![],
        tools: vec![],
        process_mode: "supervised_pair".into(),
        transports: vec![transport],
    };
    AgentRegistry::create(makakoo_home, &slot)?;

    // Archive the conversations DB and seed a fresh per-agent DB
    // location.  The legacy DB stays at its original path AS WELL
    // as the archive path — Phase 2 spec says "archived (not
    // migrated)", and we keep the original for rollback safety.
    let legacy_db = makakoo_home
        .join("data")
        .join("chat")
        .join("conversations.db");
    let archived_db = if legacy_db.exists() {
        let agent_dir = makakoo_home.join("data").join("agents").join("harveychat");
        std::fs::create_dir_all(&agent_dir)?;
        let dst = agent_dir.join("conversations.db.bak");
        std::fs::copy(&legacy_db, &dst)?;
        Some(dst)
    } else {
        None
    };

    Ok(MigrationOutcome::Migrated {
        toml_path,
        archived_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_legacy_config(home: &Path, token: &str, allowlist: &[i64]) {
        let dir = home.join("data").join("chat");
        fs::create_dir_all(&dir).unwrap();
        let json = serde_json::json!({
            "bot_token": token,
            "allowlist": allowlist,
        });
        fs::write(dir.join("config.json"), json.to_string()).unwrap();
    }

    #[test]
    fn migrate_no_legacy_returns_nothing_to_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = migrate(dir.path()).unwrap();
        assert_eq!(outcome, MigrationOutcome::NothingToMigrate);
    }

    #[test]
    fn migrate_creates_harveychat_toml_with_legacy_token_and_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        write_legacy_config(dir.path(), "9999:legacy-token", &[746496145, 999]);
        let outcome = migrate(dir.path()).unwrap();
        assert!(matches!(outcome, MigrationOutcome::Migrated { .. }));
        let registry = AgentRegistry::load(dir.path()).unwrap();
        assert_eq!(registry.slots.len(), 1);
        let slot = &registry.slots[0];
        assert_eq!(slot.slot_id, "harveychat");
        assert_eq!(slot.name, "Olibia");
        assert!(slot.persona.is_none());
        assert_eq!(slot.transports.len(), 1);
        assert_eq!(slot.transports[0].kind, "telegram");
        assert_eq!(
            slot.transports[0].inline_secret_dev.as_deref(),
            Some("9999:legacy-token")
        );
        assert_eq!(
            slot.transports[0].allowed_users,
            vec!["746496145".to_string(), "999".to_string()]
        );
    }

    #[test]
    fn migrate_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        write_legacy_config(dir.path(), "9999:legacy-token", &[1]);
        let first = migrate(dir.path()).unwrap();
        assert!(matches!(first, MigrationOutcome::Migrated { .. }));
        let second = migrate(dir.path()).unwrap();
        assert_eq!(second, MigrationOutcome::AlreadyMigrated);
    }

    #[test]
    fn migrate_archives_conversations_db_without_deleting_original() {
        let dir = tempfile::tempdir().unwrap();
        write_legacy_config(dir.path(), "9999:legacy-token", &[1]);
        let chat_dir = dir.path().join("data").join("chat");
        let original_db = chat_dir.join("conversations.db");
        fs::write(&original_db, b"sqlite-bytes-stand-in").unwrap();
        let outcome = migrate(dir.path()).unwrap();
        match outcome {
            MigrationOutcome::Migrated { archived_db, .. } => {
                let archived = archived_db.expect("archived db path");
                assert!(archived.exists());
                assert!(original_db.exists(), "original DB preserved for rollback");
                assert_eq!(
                    archived.file_name().and_then(|s| s.to_str()),
                    Some("conversations.db.bak")
                );
            }
            other => panic!("expected Migrated, got {:?}", other),
        }
    }

    #[test]
    fn migrate_handles_missing_db_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        write_legacy_config(dir.path(), "9999:legacy-token", &[1]);
        // No conversations.db present.
        let outcome = migrate(dir.path()).unwrap();
        match outcome {
            MigrationOutcome::Migrated { archived_db, .. } => assert!(archived_db.is_none()),
            other => panic!("expected Migrated, got {:?}", other),
        }
    }
}
