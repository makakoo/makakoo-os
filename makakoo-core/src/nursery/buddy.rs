//! BuddyTracker — the user's currently-active mascot.
//!
//! Rust port of `core/buddy/buddy.py`. Narrowed to the runtime API the
//! platform actually needs: "who is my active buddy", "nudge its energy and
//! mood", "give me a CLI frame to print at the top of a command".
//!
//! The full art/personality-generation path (Mulberry32 bones, accessories,
//! procedural ASCII) is intentionally *not* reimplemented here — the T7
//! gimmick compositor already owns species → ASCII art, and this tracker
//! just asks the compositor to render the species for the active mood.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};
use crate::gimmicks::{compose_for_mascot, GimmickRegistry};
use crate::platform::lock_file;

use super::registry::{Mascot, MascotRegistry};

/// Persistent buddy state.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BuddyState {
    /// Name of the currently-active mascot, if any.
    pub active: Option<String>,
    /// Last time the user interacted with the buddy.
    pub last_interaction: DateTime<Utc>,
    /// Current mood label (`"happy"`, `"alert"`, `"sleepy"`, `"content"`,
    /// `"lonely"`).
    pub mood: String,
    /// Energy on a 0–100 scale. Decays on `tick`, recharges on `interact`.
    pub energy: u8,
}

impl Default for BuddyState {
    fn default() -> Self {
        Self {
            active: None,
            last_interaction: Utc::now(),
            mood: "content".to_string(),
            energy: 80,
        }
    }
}

/// Thread-safe wrapper over [`BuddyState`] that persists to disk.
pub struct BuddyTracker {
    state: Arc<RwLock<BuddyState>>,
    registry: Arc<MascotRegistry>,
    path: PathBuf,
}

impl BuddyTracker {
    /// Load the tracker from `path`. If the file does not exist, starts with
    /// default state (no active buddy).
    pub fn load(registry: Arc<MascotRegistry>, path: &Path) -> Result<Self> {
        let state = if path.exists() {
            let raw = fs::read_to_string(path)?;
            if raw.trim().is_empty() {
                BuddyState::default()
            } else {
                serde_json::from_str(&raw).unwrap_or_default()
            }
        } else {
            BuddyState::default()
        };
        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            registry,
            path: path.to_path_buf(),
        })
    }

    /// Resolve the current active buddy via the registry.
    pub fn active(&self) -> Option<Mascot> {
        let name = self.state.read().unwrap().active.clone()?;
        self.registry.get(&name)
    }

    /// Snapshot of the persisted state.
    pub fn state(&self) -> BuddyState {
        self.state.read().unwrap().clone()
    }

    /// Set the currently-active buddy. Returns `Err(NotFound)` if the name
    /// is not in the registry.
    pub fn set_active(&self, name: &str) -> Result<()> {
        if self.registry.get(name).is_none() {
            return Err(MakakooError::NotFound(format!("mascot: {name}")));
        }
        {
            let mut s = self.state.write().unwrap();
            s.active = Some(name.to_string());
            s.last_interaction = Utc::now();
            s.mood = "happy".to_string();
            s.energy = s.energy.saturating_add(10).min(100);
        }
        self.save()
    }

    /// Record an interaction — bumps energy, resets mood toward "happy",
    /// updates `last_interaction`.
    pub fn interact(&self) {
        let mut s = self.state.write().unwrap();
        s.last_interaction = Utc::now();
        s.energy = s.energy.saturating_add(5).min(100);
        s.mood = if s.energy >= 70 {
            "happy".to_string()
        } else if s.energy >= 40 {
            "content".to_string()
        } else {
            "alert".to_string()
        };
    }

    /// Decay step — shift mood based on staleness and drain a point of
    /// energy. Meant to be called periodically (e.g. from SANCHO's ticker).
    pub fn tick(&self) {
        let mut s = self.state.write().unwrap();
        if s.energy > 0 {
            s.energy -= 1;
        }
        let hours = (Utc::now() - s.last_interaction).num_hours();
        s.mood = if hours < 2 && s.energy >= 60 {
            "happy".to_string()
        } else if hours < 8 && s.energy >= 30 {
            "content".to_string()
        } else if hours < 24 {
            "alert".to_string()
        } else if hours < 72 {
            "sleepy".to_string()
        } else {
            "lonely".to_string()
        };
    }

    /// Render a CLI frame showing the active buddy's species art + a status
    /// line. The art comes from the T7 gimmick compositor so every mood is
    /// a valid 3-line uniform-width sprite.
    pub fn display_frame(&self) -> String {
        let Some(mascot) = self.active() else {
            return "[no active buddy — `harvey nursery adopt <name>` to pick one]\n"
                .to_string();
        };
        let state = self.state();
        // Pick a recipe whose vibe matches the mood. Every key here exists
        // in the T7 recipe catalog.
        let recipe = match state.mood.as_str() {
            "happy" | "content" => "celebrate",
            "alert" => "error",
            "sleepy" => "dream",
            "lonely" => "memory",
            _ => "boot",
        };
        // Sanity-check the recipe is known; fall back if the T7 registry
        // ever drops one of the recipes we map to.
        let reg = GimmickRegistry::global();
        let safe_recipe = if reg.recipe(recipe).is_some() {
            recipe
        } else {
            "search"
        };
        let art = compose_for_mascot(&mascot.name, &mascot.species, safe_recipe);
        let mut out = String::new();
        out.push('\n');
        for line in &art {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&format!(
            "  {} the {} — mood:{} energy:{}/100\n",
            mascot.name, mascot.species, state.mood, state.energy
        ));
        out.push_str(&format!("  {}\n", mascot.voice.greeting));
        out
    }

    /// Persist the tracker to disk. Uses the same lock-then-rename pattern
    /// as the registry.
    pub fn save(&self) -> Result<()> {
        let snapshot = self.state.read().unwrap().clone();
        let json = serde_json::to_string_pretty(&snapshot)?;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = self.path.with_extension("lock");
        let _guard = lock_file(&lock_path)?;

        let tmp_path = self.path.with_extension("json.tmp");
        {
            let mut tmp = fs::File::create(&tmp_path)?;
            tmp.write_all(json.as_bytes())?;
            tmp.sync_all()?;
        }
        fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════
//  Tests
// ══════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_tracker() -> (TempDir, BuddyTracker) {
        let dir = tempfile::tempdir().unwrap();
        let reg_path = dir.path().join("nursery.json");
        let buddy_path = dir.path().join("buddy.json");
        let registry = Arc::new(MascotRegistry::load(&reg_path).unwrap());
        let tracker = BuddyTracker::load(registry, &buddy_path).unwrap();
        (dir, tracker)
    }

    #[test]
    fn default_state_has_no_active_buddy() {
        let (_d, tracker) = tmp_tracker();
        assert!(tracker.active().is_none());
    }

    #[test]
    fn set_active_known_mascot_succeeds() {
        let (_d, tracker) = tmp_tracker();
        tracker.set_active("Olibia").unwrap();
        let active = tracker.active().expect("active set");
        assert_eq!(active.name, "Olibia");
        assert_eq!(tracker.state().active.as_deref(), Some("Olibia"));
    }

    #[test]
    fn set_active_unknown_mascot_errors() {
        let (_d, tracker) = tmp_tracker();
        let err = tracker.set_active("Ghost");
        assert!(matches!(err, Err(MakakooError::NotFound(_))));
    }

    #[test]
    fn set_active_persists_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let reg_path = dir.path().join("nursery.json");
        let buddy_path = dir.path().join("buddy.json");
        {
            let registry = Arc::new(MascotRegistry::load(&reg_path).unwrap());
            let tracker = BuddyTracker::load(registry, &buddy_path).unwrap();
            tracker.set_active("Pixel").unwrap();
        }
        let registry = Arc::new(MascotRegistry::load(&reg_path).unwrap());
        let tracker2 = BuddyTracker::load(registry, &buddy_path).unwrap();
        assert_eq!(tracker2.state().active.as_deref(), Some("Pixel"));
    }

    #[test]
    fn interact_raises_energy_and_mood() {
        let (_d, tracker) = tmp_tracker();
        tracker.set_active("Ziggy").unwrap();
        // Force energy low to watch it climb back up.
        {
            let mut s = tracker.state.write().unwrap();
            s.energy = 50;
            s.mood = "alert".to_string();
        }
        tracker.interact();
        let after = tracker.state();
        assert!(after.energy >= 55);
        assert_eq!(after.mood, "content");
    }

    #[test]
    fn tick_decays_energy_and_shifts_mood() {
        let (_d, tracker) = tmp_tracker();
        tracker.set_active("Cinder").unwrap();
        let before = tracker.state().energy;
        tracker.tick();
        let after = tracker.state();
        assert_eq!(after.energy, before - 1);
        assert!(["happy", "content", "alert", "sleepy", "lonely"]
            .contains(&after.mood.as_str()));
    }

    #[test]
    fn tick_energy_floor_is_zero() {
        let (_d, tracker) = tmp_tracker();
        tracker.set_active("Glimmer").unwrap();
        {
            let mut s = tracker.state.write().unwrap();
            s.energy = 0;
        }
        tracker.tick();
        assert_eq!(tracker.state().energy, 0);
    }

    #[test]
    fn display_frame_contains_mascot_and_mood() {
        let (_d, tracker) = tmp_tracker();
        tracker.set_active("Olibia").unwrap();
        let frame = tracker.display_frame();
        assert!(frame.contains("Olibia"));
        assert!(frame.contains("mood:"));
        assert!(frame.contains("energy:"));
        // Three art lines + name line + voice line + leading newline → at
        // least 5 newlines.
        assert!(frame.matches('\n').count() >= 5);
        // Never any ANSI escapes — this output can end up in MCP responses.
        assert!(!frame.contains('\x1b'));
    }

    #[test]
    fn display_frame_no_buddy_returns_hint() {
        let (_d, tracker) = tmp_tracker();
        let frame = tracker.display_frame();
        assert!(frame.contains("no active buddy"));
    }

    #[test]
    fn state_json_roundtrip_is_identity() {
        let s = BuddyState {
            active: Some("Pixel".into()),
            last_interaction: Utc::now(),
            mood: "happy".into(),
            energy: 73,
        };
        let j = serde_json::to_string(&s).unwrap();
        let back: BuddyState = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }
}
