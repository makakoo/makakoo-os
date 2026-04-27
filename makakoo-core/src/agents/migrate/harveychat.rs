//! HarveyChat (Olibia) migration: legacy
//! `~/MAKAKOO/data/chat/config.json` → new
//! `~/MAKAKOO/config/agents/harveychat.toml`.
//!
//! Locked by SPRINT.md "Olibia migration (explicit)" section.  All
//! side effects live here so library callers (Phase 3 supervisor,
//! tests, future setup wizard) and the CLI wrapper see identical
//! behavior:
//!
//!   - slot id is `harveychat` (NEVER `olibia` — Olibia is the
//!     display `name` only)
//!   - bot token preserved as `inline_secret_dev` fallback;
//!     `secret_ref` + `secret_env` populated for the keychain
//!     migration path
//!   - allowed_users carry the legacy chat_id list (string-encoded)
//!   - persona = null inherits HARVEY_SYSTEM_PROMPT (don't strip)
//!   - legacy `conversations.db` archived (not merged) at
//!     `data/agents/harveychat/conversations.db.bak` — original
//!     preserved for rollback
//!   - legacy `data/chat/config.json` archived to
//!     `data/agents/harveychat/config.json.bak` — same rollback
//!     rationale
//!   - fresh per-agent `conversations.db` seeded at
//!     `data/agents/harveychat/conversations.db` (SQLite would
//!     create on first open; explicit creation surfaces permission
//!     errors at migration time)
//!   - migration is IDEMPOTENT: a TOML-already-present run returns
//!     `MigrationOutcome::AlreadyMigrated { backfilled_artifacts }`,
//!     where the vec lists any missing artifacts (config archive,
//!     DB archive, fresh DB) re-created during the re-run. Empty vec
//!     means the previous migration was already complete.

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
        archived_config: Option<PathBuf>,
        new_db: Option<PathBuf>,
    },
    /// Slot TOML already exists. The migration MAY have backfilled
    /// missing artifacts (fresh DB, archived config) — see
    /// `backfilled_artifacts`.
    AlreadyMigrated {
        backfilled_artifacts: Vec<PathBuf>,
    },
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
    let legacy_path = makakoo_home.join("data").join("chat").join("config.json");
    if toml_path.exists() {
        // Slot already migrated — but a previous run might have
        // skipped some artifacts (older versions of this code only
        // archived the DB). Backfill anything missing so re-runs
        // always converge on a fully migrated state.
        let mut backfilled = Vec::new();
        let agent_dir = makakoo_home.join("data").join("agents").join("harveychat");
        std::fs::create_dir_all(&agent_dir)?;

        if legacy_path.exists() {
            let archived_config = agent_dir.join("config.json.bak");
            if !archived_config.exists() {
                std::fs::copy(&legacy_path, &archived_config)?;
                backfilled.push(archived_config);
            }
        }
        let legacy_db = makakoo_home
            .join("data")
            .join("chat")
            .join("conversations.db");
        if legacy_db.exists() {
            let archived_db = agent_dir.join("conversations.db.bak");
            if !archived_db.exists() {
                std::fs::copy(&legacy_db, &archived_db)?;
                backfilled.push(archived_db);
            }
        }
        let new_db = agent_dir.join("conversations.db");
        if !new_db.exists() {
            std::fs::File::create(&new_db)?;
            backfilled.push(new_db);
        }
        return Ok(MigrationOutcome::AlreadyMigrated {
            backfilled_artifacts: backfilled,
        });
    }
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
        llm: None,
    };
    AgentRegistry::create(makakoo_home, &slot)?;

    // Always create the per-agent data directory so DB seeding +
    // config archive land in the same place.
    let agent_dir = makakoo_home
        .join("data")
        .join("agents")
        .join("harveychat");
    std::fs::create_dir_all(&agent_dir)?;

    // Archive the conversations DB. Original is preserved AS WELL
    // as the archive copy — Phase 2 spec says "archived (not
    // migrated)", original kept for rollback safety.
    let legacy_db = makakoo_home
        .join("data")
        .join("chat")
        .join("conversations.db");
    let archived_db = if legacy_db.exists() {
        let dst = agent_dir.join("conversations.db.bak");
        std::fs::copy(&legacy_db, &dst)?;
        Some(dst)
    } else {
        None
    };

    // Archive the legacy config.json so a future operator can find
    // both pieces of legacy state in one place. Original preserved
    // (copy semantics) for the same rollback rationale.
    let archived_config = {
        let dst = agent_dir.join("config.json.bak");
        std::fs::copy(&legacy_path, &dst)?;
        Some(dst)
    };

    // Seed a fresh per-agent conversations.db at the canonical
    // path. SQLite would create it on first open, but explicit
    // creation surfaces permission errors at migration time.
    let new_db = agent_dir.join("conversations.db");
    let new_db = if !new_db.exists() {
        std::fs::File::create(&new_db)?;
        Some(new_db)
    } else {
        Some(new_db)
    };

    Ok(MigrationOutcome::Migrated {
        toml_path,
        archived_db,
        archived_config,
        new_db,
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
        assert!(matches!(second, MigrationOutcome::AlreadyMigrated { .. }));
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
            MigrationOutcome::Migrated { archived_db, new_db, .. } => {
                assert!(archived_db.is_none());
                // Fresh per-agent DB still seeded even when no
                // legacy DB existed.
                let new_db = new_db.expect("fresh new_db path");
                assert!(new_db.exists());
            }
            other => panic!("expected Migrated, got {:?}", other),
        }
    }

    #[test]
    fn migrate_archives_legacy_config_json() {
        let dir = tempfile::tempdir().unwrap();
        write_legacy_config(dir.path(), "9999:legacy-token", &[1]);
        let original_config = dir.path().join("data").join("chat").join("config.json");
        let outcome = migrate(dir.path()).unwrap();
        match outcome {
            MigrationOutcome::Migrated { archived_config, .. } => {
                let archived = archived_config.expect("archived config path");
                assert!(archived.exists());
                assert!(original_config.exists(), "original config preserved");
                assert_eq!(
                    archived.file_name().and_then(|s| s.to_str()),
                    Some("config.json.bak")
                );
            }
            other => panic!("expected Migrated, got {:?}", other),
        }
    }

    #[test]
    fn migrate_seeds_fresh_per_agent_db() {
        let dir = tempfile::tempdir().unwrap();
        write_legacy_config(dir.path(), "9999:legacy-token", &[1]);
        let outcome = migrate(dir.path()).unwrap();
        match outcome {
            MigrationOutcome::Migrated { new_db, .. } => {
                let new_db = new_db.expect("fresh new_db path");
                assert!(new_db.exists(), "fresh per-agent conversations.db seeded");
                assert_eq!(
                    new_db.file_name().and_then(|s| s.to_str()),
                    Some("conversations.db")
                );
                assert_eq!(
                    new_db.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()),
                    Some("harveychat")
                );
            }
            other => panic!("expected Migrated, got {:?}", other),
        }
    }

    #[test]
    fn migrate_already_migrated_backfills_missing_artifacts() {
        // Simulate a partial first migration: TOML exists, but
        // backup files do NOT (older code path).
        let dir = tempfile::tempdir().unwrap();
        write_legacy_config(dir.path(), "9999:legacy-token", &[1]);
        // First migration creates everything.
        let _ = migrate(dir.path()).unwrap();
        // Delete the artifacts to simulate the partial-state
        // scenario.
        let agent_dir = dir.path().join("data").join("agents").join("harveychat");
        let _ = std::fs::remove_file(agent_dir.join("config.json.bak"));
        let _ = std::fs::remove_file(agent_dir.join("conversations.db"));
        // Re-run — should backfill the deleted artifacts.
        let outcome = migrate(dir.path()).unwrap();
        match outcome {
            MigrationOutcome::AlreadyMigrated { backfilled_artifacts } => {
                assert!(!backfilled_artifacts.is_empty(), "backfill must happen");
                assert!(agent_dir.join("config.json.bak").exists());
                assert!(agent_dir.join("conversations.db").exists());
            }
            other => panic!("expected AlreadyMigrated with backfill, got {:?}", other),
        }
    }
}
