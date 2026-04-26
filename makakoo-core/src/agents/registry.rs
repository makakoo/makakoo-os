//! Agent slot registry — enumerate `~/MAKAKOO/config/agents/*.toml`.
//!
//! Phase 2 deliverable.  Pure read/write over the canonical
//! `<makakoo_home>/config/agents/` directory.  No process
//! lifecycle here (Phase 3 wires the per-slot Python gateway
//! supervisor).

use std::path::Path;

use crate::agents::slot::{registry_dir, slot_path, AgentSlot};
use crate::{MakakooError, Result};

/// Result of enumerating the slot registry.
pub struct AgentRegistry {
    pub slots: Vec<AgentSlot>,
}

impl AgentRegistry {
    /// Enumerate every `*.toml` in the registry directory.  Slots
    /// that fail to parse are skipped (with a WARN log) so a
    /// single broken slot doesn't crash `makakoo agent list`.
    pub fn load(makakoo_home: &Path) -> Result<Self> {
        let dir = registry_dir(makakoo_home);
        if !dir.exists() {
            return Ok(Self { slots: vec![] });
        }
        let mut slots = vec![];
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            match AgentSlot::load_from_file(&path) {
                Ok(slot) => slots.push(slot),
                Err(e) => {
                    tracing::warn!(
                        target: "makakoo_core::agents::registry",
                        path = %path.display(),
                        error = %e,
                        "skipping malformed agent slot"
                    );
                }
            }
        }
        slots.sort_by(|a, b| a.slot_id.cmp(&b.slot_id));
        Ok(Self { slots })
    }

    /// Look up a single slot by id.  Returns `None` if missing.
    pub fn get(&self, slot_id: &str) -> Option<&AgentSlot> {
        self.slots.iter().find(|s| s.slot_id == slot_id)
    }

    /// Write a new slot to disk.  Refuses to overwrite an existing
    /// file (Phase 2 spec — duplicate slot rejection by filename).
    pub fn create(makakoo_home: &Path, slot: &AgentSlot) -> Result<()> {
        slot.validate()?;
        let dir = registry_dir(makakoo_home);
        std::fs::create_dir_all(&dir)?;
        let path = slot_path(makakoo_home, &slot.slot_id);
        if path.exists() {
            return Err(MakakooError::InvalidInput(format!(
                "agent slot '{}' already exists at {} — refusing to overwrite",
                slot.slot_id,
                path.display()
            )));
        }
        let raw = toml::to_string_pretty(slot)
            .map_err(|e| MakakooError::Internal(format!("agent slot serialise: {}", e)))?;
        std::fs::write(&path, raw)?;
        Ok(())
    }

    /// Atomic-ish overwrite for an EXISTING slot (used by the
    /// migration path to update harveychat in-place).  Refuses if
    /// the file does not yet exist.
    pub fn update(makakoo_home: &Path, slot: &AgentSlot) -> Result<()> {
        slot.validate()?;
        let path = slot_path(makakoo_home, &slot.slot_id);
        if !path.exists() {
            return Err(MakakooError::NotFound(format!(
                "agent slot '{}' not found at {} — use create() instead",
                slot.slot_id,
                path.display()
            )));
        }
        let tmp = path.with_extension("toml.tmp");
        let raw = toml::to_string_pretty(slot)
            .map_err(|e| MakakooError::Internal(format!("agent slot serialise: {}", e)))?;
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::slot::AgentSlot;
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
            name: "Test".into(),
            persona: None,
            inherit_baseline: true,
            allowed_paths: vec![],
            forbidden_paths: vec![],
            tools: vec![],
            process_mode: "supervised_pair".into(),
            transports: vec![telegram_block("telegram-main")],
        }
    }

    #[test]
    fn load_returns_empty_when_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let r = AgentRegistry::load(dir.path()).unwrap();
        assert!(r.slots.is_empty());
    }

    #[test]
    fn create_writes_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        AgentRegistry::create(dir.path(), &slot("harveychat")).unwrap();
        let r = AgentRegistry::load(dir.path()).unwrap();
        assert_eq!(r.slots.len(), 1);
        assert_eq!(r.slots[0].slot_id, "harveychat");
    }

    #[test]
    fn create_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        AgentRegistry::create(dir.path(), &slot("harveychat")).unwrap();
        let err = AgentRegistry::create(dir.path(), &slot("harveychat")).unwrap_err();
        assert!(format!("{err}").contains("already exists"));
    }

    #[test]
    fn update_refuses_create() {
        let dir = tempfile::tempdir().unwrap();
        let err = AgentRegistry::update(dir.path(), &slot("harveychat")).unwrap_err();
        assert!(format!("{err}").contains("not found"));
    }

    #[test]
    fn update_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        AgentRegistry::create(dir.path(), &slot("harveychat")).unwrap();
        let mut s = slot("harveychat");
        s.name = "Olibia".into();
        AgentRegistry::update(dir.path(), &s).unwrap();
        let r = AgentRegistry::load(dir.path()).unwrap();
        assert_eq!(r.slots[0].name, "Olibia");
    }

    #[test]
    fn load_skips_malformed_slot_with_warn() {
        let dir = tempfile::tempdir().unwrap();
        AgentRegistry::create(dir.path(), &slot("harveychat")).unwrap();
        // Drop a broken file alongside.
        let bad = registry_dir(dir.path()).join("broken.toml");
        std::fs::write(&bad, "this is not valid toml := ===").unwrap();
        let r = AgentRegistry::load(dir.path()).unwrap();
        assert_eq!(r.slots.len(), 1);
        assert_eq!(r.slots[0].slot_id, "harveychat");
    }

    #[test]
    fn get_returns_slot_by_id() {
        let dir = tempfile::tempdir().unwrap();
        AgentRegistry::create(dir.path(), &slot("harveychat")).unwrap();
        AgentRegistry::create(dir.path(), &slot("secretary")).unwrap();
        let r = AgentRegistry::load(dir.path()).unwrap();
        assert!(r.get("harveychat").is_some());
        assert!(r.get("missing").is_none());
    }
}
