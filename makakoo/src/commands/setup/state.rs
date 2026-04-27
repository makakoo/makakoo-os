//! Read/write `completed.json` — the wizard's durable per-section state.
//!
//! Location: `$MAKAKOO_HOME/state/makakoo-setup/completed.json`.
//! Follows the makakoo state-dir convention (`state/<plugin-or-component>/`).
//! Atomic writes via tmp+rename so partial writes can never corrupt the
//! file; on parse error we treat the file as absent (never crash).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::harness::SectionStatus;

/// On-disk schema version. Bump when adding incompatible fields; loader
/// treats a mismatched version as a fresh state so users don't crash on
/// upgrade.
pub const STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateFile {
    pub version: u32,
    pub sections: BTreeMap<String, SectionStatus>,
}

impl Default for StateFile {
    fn default() -> Self {
        Self {
            version: STATE_SCHEMA_VERSION,
            sections: BTreeMap::new(),
        }
    }
}

impl StateFile {
    pub fn get(&self, section: &str) -> SectionStatus {
        self.sections
            .get(section)
            .cloned()
            .unwrap_or(SectionStatus::NotStarted)
    }

    pub fn set(&mut self, section: &str, status: SectionStatus) {
        self.sections.insert(section.to_string(), status);
    }

    pub fn remove(&mut self, section: &str) {
        self.sections.remove(section);
    }
}

/// Directory under `$MAKAKOO_HOME` where `completed.json` lives.
pub fn state_dir_for(home: &Path) -> PathBuf {
    home.join("state").join("makakoo-setup")
}

/// Full path to `completed.json`.
pub fn state_path_for(home: &Path) -> PathBuf {
    state_dir_for(home).join("completed.json")
}

/// Load the state file. Returns `Default::default()` (empty, version=1)
/// if the file is missing, unreadable, malformed JSON, or has a
/// mismatched schema version. Never panics; callers can trust the result.
pub fn load(home: &Path) -> StateFile {
    let path = state_path_for(home);
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return StateFile::default(),
    };
    match serde_json::from_str::<StateFile>(&raw) {
        Ok(s) if s.version == STATE_SCHEMA_VERSION => s,
        _ => StateFile::default(),
    }
}

/// Atomically persist the state file. Creates the parent directory on
/// demand. Any error propagates — the dispatcher surfaces it to the user.
pub fn save(home: &Path, state: &StateFile) -> anyhow::Result<()> {
    let dir = state_dir_for(home);
    fs::create_dir_all(&dir)?;
    let path = state_path_for(home);
    let body = serde_json::to_string_pretty(state)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Wipe the state file (used by `makakoo setup --reset`). If the file
/// doesn't exist, this is a no-op.
pub fn reset(home: &Path) -> anyhow::Result<()> {
    let path = state_path_for(home);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn fresh_home() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn load_returns_default_when_missing() {
        let home = fresh_home();
        let state = load(home.path());
        assert_eq!(state.version, STATE_SCHEMA_VERSION);
        assert!(state.sections.is_empty());
    }

    #[test]
    fn save_and_reload_roundtrips() {
        let home = fresh_home();
        let mut state = StateFile::default();
        state.set(
            "persona",
            SectionStatus::Completed { at: Utc::now() },
        );
        state.set(
            "brain",
            SectionStatus::Skipped { at: Utc::now() },
        );
        save(home.path(), &state).unwrap();

        let back = load(home.path());
        assert_eq!(back.sections.len(), 2);
        assert!(matches!(
            back.get("persona"),
            SectionStatus::Completed { .. }
        ));
        assert!(matches!(back.get("brain"), SectionStatus::Skipped { .. }));
    }

    #[test]
    fn save_is_atomic() {
        let home = fresh_home();
        let state = StateFile::default();
        save(home.path(), &state).unwrap();
        // tmp file must not linger after a successful write
        let tmp = state_path_for(home.path()).with_extension("json.tmp");
        assert!(!tmp.exists(), "tmp file should be gone after atomic rename");
    }

    #[test]
    fn corrupt_file_falls_back_to_default() {
        let home = fresh_home();
        let dir = state_dir_for(home.path());
        fs::create_dir_all(&dir).unwrap();
        fs::write(state_path_for(home.path()), "{not json").unwrap();
        let state = load(home.path());
        assert!(state.sections.is_empty());
    }

    #[test]
    fn version_mismatch_falls_back_to_default() {
        let home = fresh_home();
        let dir = state_dir_for(home.path());
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            state_path_for(home.path()),
            r#"{"version": 99, "sections": {"foo": {"status": "NotStarted"}}}"#,
        )
        .unwrap();
        let state = load(home.path());
        assert!(state.sections.is_empty());
    }

    #[test]
    fn reset_removes_file() {
        let home = fresh_home();
        save(home.path(), &StateFile::default()).unwrap();
        assert!(state_path_for(home.path()).exists());
        reset(home.path()).unwrap();
        assert!(!state_path_for(home.path()).exists());
    }

    #[test]
    fn reset_is_noop_when_missing() {
        let home = fresh_home();
        reset(home.path()).unwrap(); // should not error
    }

    #[test]
    fn get_missing_returns_not_started() {
        let state = StateFile::default();
        assert_eq!(state.get("anything"), SectionStatus::NotStarted);
    }
}
