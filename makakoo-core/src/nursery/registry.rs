//! MascotRegistry — persistent collection of named mascots.
//!
//! Rust port of the structural core of `core/buddy/nursery.py` +
//! `core/agent/mascot.py` (Olibia persona). The Python source is a whimsical
//! procedurally-generated Tamagotchi; the Rust port narrows scope to the
//! subset the platform actually needs at runtime: a small set of named,
//! maintainer-owned mascots with a job, a voice, and a patrol cadence.
//!
//! Storage: JSON at `{makakoo_home}/data/nursery.json`, load-once at startup,
//! save-with-lock on every mutation (fs2 exclusive lock during write so two
//! CLIs can't race each other). Seeded with the canonical mascots on first
//! load.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};
use crate::platform::lock_file;

/// A named mascot maintained by a contributor.
///
/// # Schema drift note
///
/// The legacy `data/nursery.json` written by the pre-rewrite Python
/// Tamagotchi era (`core/buddy/nursery.py`) used a wildly different shape:
/// `{mascot_id, species_name, personality, rarity, is_shiny, face, art,
/// stats, mood, eye_style, hat, born_at, ...}`. The Rust rewrite narrowed
/// the schema to what the platform actually uses, but must still be able
/// to load legacy JSON so `makakoo nursery list` works on installs that
/// predate the rewrite. Every field added by Rust is `#[serde(default)]`
/// so legacy files parse into sensible fallbacks. Only `name` and `species`
/// are required, and both exist in the legacy output.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Mascot {
    /// Unique name within the registry (e.g. "Olibia", "Pixel").
    pub name: String,
    /// Species key from the T7 gimmick LEGO catalog ("owl", "fox", ...).
    pub species: String,
    /// Contributor handle that owns this mascot (e.g. "@schkudlara").
    /// Absent in legacy Python JSON — defaults to empty string via serde.
    #[serde(default)]
    pub maintainer: String,
    /// One-line description of what the patrol does.
    /// Absent in legacy Python JSON — defaults to empty string via serde.
    #[serde(default)]
    pub job: String,
    /// Flavor lines used when the mascot speaks.
    /// Absent in legacy Python JSON — defaults to all-empty lines via serde.
    #[serde(default)]
    pub voice: MascotVoice,
    /// How often the patrol runs, in hours. Default: 2.
    #[serde(default = "default_patrol_interval_hours")]
    pub patrol_interval_hours: u32,
    /// UTC timestamp when the mascot was first registered.
    /// Legacy Python stored `born_at` as Unix float and doesn't write
    /// `created_at` — defaults to now via serde.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// Lifecycle state. Legacy Python had no lifecycle — defaults to Active.
    #[serde(default = "default_status")]
    pub status: MascotStatus,
}

fn default_patrol_interval_hours() -> u32 {
    2
}

fn default_status() -> MascotStatus {
    MascotStatus::Active
}

/// Flavor lines for a mascot — how it speaks when it finds something.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct MascotVoice {
    #[serde(default)]
    pub greeting: String,
    #[serde(default)]
    pub alert: String,
    #[serde(default)]
    pub success: String,
    #[serde(default)]
    pub sleeping: String,
}

/// Lifecycle state for a mascot.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum MascotStatus {
    /// Just registered, not yet running patrols.
    Hatching,
    /// Live, patrols run on schedule.
    Active,
    /// Retired, no longer patrols but kept in the registry for history.
    Retired,
}

/// Persistent store for mascots.
///
/// Thread-safe via `RwLock`. Every mutation persists to disk under a file
/// lock so concurrent `harvey` processes on the same machine cannot corrupt
/// `nursery.json`.
pub struct MascotRegistry {
    mascots: RwLock<HashMap<String, Mascot>>,
    registry_path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct NurseryFile {
    mascots: Vec<Mascot>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl MascotRegistry {
    /// Load the registry from `path`. If the file does not exist, seed with
    /// [`Self::canonical`] and persist.
    pub fn load(path: &Path) -> Result<Self> {
        let registry = Self {
            mascots: RwLock::new(HashMap::new()),
            registry_path: path.to_path_buf(),
        };

        if path.exists() {
            let raw = fs::read_to_string(path)?;
            if !raw.trim().is_empty() {
                let parsed: NurseryFile = serde_json::from_str(&raw)?;
                let mut map = registry.mascots.write().unwrap();
                for m in parsed.mascots {
                    map.insert(m.name.clone(), m);
                }
            }
        } else {
            let mut map = registry.mascots.write().unwrap();
            for m in Self::canonical() {
                map.insert(m.name.clone(), m);
            }
            drop(map);
            registry.save()?;
        }

        Ok(registry)
    }

    /// Snapshot of every mascot in the registry.
    pub fn all(&self) -> Vec<Mascot> {
        let map = self.mascots.read().unwrap();
        let mut out: Vec<Mascot> = map.values().cloned().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Look up a mascot by name.
    pub fn get(&self, name: &str) -> Option<Mascot> {
        let map = self.mascots.read().unwrap();
        map.get(name).cloned()
    }

    /// Register a new mascot. Returns `Err(Internal)` if a mascot with the
    /// same name is already present.
    pub fn register(&self, mascot: Mascot) -> Result<()> {
        {
            let mut map = self.mascots.write().unwrap();
            if map.contains_key(&mascot.name) {
                return Err(MakakooError::internal(format!(
                    "mascot {} already registered",
                    mascot.name
                )));
            }
            map.insert(mascot.name.clone(), mascot);
        }
        self.save()
    }

    /// Update the lifecycle status of a mascot. Returns `Err(NotFound)` if
    /// the mascot does not exist.
    pub fn set_status(&self, name: &str, status: MascotStatus) -> Result<()> {
        {
            let mut map = self.mascots.write().unwrap();
            let entry = map
                .get_mut(name)
                .ok_or_else(|| MakakooError::NotFound(format!("mascot: {name}")))?;
            entry.status = status;
        }
        self.save()
    }

    /// Every mascot whose status is [`MascotStatus::Active`].
    pub fn list_active(&self) -> Vec<Mascot> {
        self.all()
            .into_iter()
            .filter(|m| m.status == MascotStatus::Active)
            .collect()
    }

    /// Persist the registry to disk. Atomic-ish: writes a sibling `.tmp`
    /// file while holding an exclusive lock on a `.lock` file, then renames
    /// it on top of the real path.
    pub fn save(&self) -> Result<()> {
        let map = self.mascots.read().unwrap();
        let mut mascots: Vec<Mascot> = map.values().cloned().collect();
        mascots.sort_by(|a, b| a.name.cmp(&b.name));
        let file = NurseryFile {
            mascots,
            updated_at: Some(Utc::now().to_rfc3339()),
        };
        let json = serde_json::to_string_pretty(&file)?;

        if let Some(parent) = self.registry_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Take an exclusive file lock so two concurrent writers don't clobber
        // each other. We hold the lock for the full write+rename cycle.
        let lock_path = self.registry_path.with_extension("lock");
        let _guard = lock_file(&lock_path)?;

        let tmp_path = self.registry_path.with_extension("json.tmp");
        {
            let mut tmp = fs::File::create(&tmp_path)?;
            tmp.write_all(json.as_bytes())?;
            tmp.sync_all()?;
        }
        fs::rename(&tmp_path, &self.registry_path)?;
        Ok(())
    }

    /// The canonical seed mascot roster. These match the Python legacy
    /// migration map (`_LEGACY_SPECIES_MAP` / `_LEGACY_NAME_FIX` in
    /// `core/buddy/nursery.py`) plus Olibia from `core/agent/mascot.py`.
    ///
    /// Species keys must exist in the T7 gimmick registry (`owl`, `fox`,
    /// `raven`, `eagle`).
    pub fn canonical() -> Vec<Mascot> {
        let now = Utc::now();
        vec![
            Mascot {
                name: "Olibia".into(),
                species: "owl".into(),
                maintainer: "@schkudlara".into(),
                job: "guardian owl — system prompt personality + milestones".into(),
                voice: MascotVoice {
                    greeting: "Here. Ready.".into(),
                    alert: "Heads up: something needs attention.".into(),
                    success: "Proud of the work. All preserved.".into(),
                    sleeping: "On the branch. Watching.".into(),
                },
                patrol_interval_hours: 2,
                created_at: now,
                status: MascotStatus::Active,
            },
            Mascot {
                name: "Pixel".into(),
                species: "raven".into(),
                maintainer: "@schkudlara".into(),
                job: "log patroller — catches 401/403/ERROR in recent logs".into(),
                voice: MascotVoice {
                    greeting: "Pixel, reporting.".into(),
                    alert: "Something's off in the logs.".into(),
                    success: "Logs are clean. Nothing to see.".into(),
                    sleeping: "Pixel dozes on the log stream.".into(),
                },
                patrol_interval_hours: 2,
                created_at: now,
                status: MascotStatus::Active,
            },
            Mascot {
                name: "Cinder".into(),
                species: "raven".into(),
                maintainer: "@schkudlara".into(),
                job: "build health — flags recent cargo/test failures".into(),
                voice: MascotVoice {
                    greeting: "Cinder, stoking the build.".into(),
                    alert: "A build just smoked out.".into(),
                    success: "Green all the way down.".into(),
                    sleeping: "Embers low. Cinder resting.".into(),
                },
                patrol_interval_hours: 4,
                created_at: now,
                status: MascotStatus::Active,
            },
            Mascot {
                name: "Ziggy".into(),
                species: "owl".into(),
                maintainer: "@schkudlara".into(),
                job: "journal integrity — verifies today's Brain entries parse".into(),
                voice: MascotVoice {
                    greeting: "Ziggy, branching.".into(),
                    alert: "A journal entry broke format.".into(),
                    success: "Today's journal is tidy.".into(),
                    sleeping: "Ziggy blinks slowly.".into(),
                },
                patrol_interval_hours: 2,
                created_at: now,
                status: MascotStatus::Active,
            },
            Mascot {
                name: "Glimmer".into(),
                species: "eagle".into(),
                maintainer: "@schkudlara".into(),
                job: "superbrain health — spot-checks FTS5 + vector coverage".into(),
                voice: MascotVoice {
                    greeting: "Glimmer, scanning high.".into(),
                    alert: "A memory drifted out of the vector space.".into(),
                    success: "Embeddings look balanced.".into(),
                    sleeping: "Glimmer circles lazily.".into(),
                },
                patrol_interval_hours: 6,
                created_at: now,
                status: MascotStatus::Active,
            },
        ]
    }
}

// ══════════════════════════════════════════════════════════════
//  Tests
// ══════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_registry() -> (TempDir, MascotRegistry) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nursery.json");
        let reg = MascotRegistry::load(&path).unwrap();
        (dir, reg)
    }

    #[test]
    fn canonical_has_five_mascots() {
        let canon = MascotRegistry::canonical();
        assert_eq!(canon.len(), 5);
        let names: Vec<&str> = canon.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"Olibia"));
        assert!(names.contains(&"Pixel"));
        assert!(names.contains(&"Cinder"));
        assert!(names.contains(&"Ziggy"));
        assert!(names.contains(&"Glimmer"));
    }

    #[test]
    fn load_seeds_canonical_on_missing_file() {
        let (_dir, reg) = tmp_registry();
        let all = reg.all();
        assert_eq!(all.len(), 5);
        assert!(reg.get("Olibia").is_some());
    }

    #[test]
    fn load_reads_existing_file_without_reseeding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nursery.json");
        {
            let reg = MascotRegistry::load(&path).unwrap();
            reg.set_status("Olibia", MascotStatus::Retired).unwrap();
        }
        // Reopen — Olibia should still be Retired, not reseeded as Active.
        let reg2 = MascotRegistry::load(&path).unwrap();
        let olibia = reg2.get("Olibia").expect("olibia present");
        assert_eq!(olibia.status, MascotStatus::Retired);
    }

    #[test]
    fn register_rejects_duplicates() {
        let (_d, reg) = tmp_registry();
        let dup = Mascot {
            name: "Olibia".into(),
            species: "owl".into(),
            maintainer: "@someone".into(),
            job: "dup".into(),
            voice: MascotVoice {
                greeting: "".into(),
                alert: "".into(),
                success: "".into(),
                sleeping: "".into(),
            },
            patrol_interval_hours: 1,
            created_at: Utc::now(),
            status: MascotStatus::Active,
        };
        assert!(reg.register(dup).is_err());
    }

    #[test]
    fn register_adds_new_mascot() {
        let (_d, reg) = tmp_registry();
        let fresh = Mascot {
            name: "Sparky".into(),
            species: "fox".into(),
            maintainer: "@contrib".into(),
            job: "spark catcher".into(),
            voice: MascotVoice {
                greeting: "Zap.".into(),
                alert: "Short circuit.".into(),
                success: "Conductive.".into(),
                sleeping: "Grounded.".into(),
            },
            patrol_interval_hours: 3,
            created_at: Utc::now(),
            status: MascotStatus::Hatching,
        };
        reg.register(fresh.clone()).unwrap();
        assert_eq!(reg.all().len(), 6);
        assert_eq!(reg.get("Sparky"), Some(fresh));
    }

    #[test]
    fn set_status_updates_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nursery.json");
        let reg = MascotRegistry::load(&path).unwrap();
        reg.set_status("Pixel", MascotStatus::Retired).unwrap();
        let reg2 = MascotRegistry::load(&path).unwrap();
        assert_eq!(reg2.get("Pixel").unwrap().status, MascotStatus::Retired);
    }

    #[test]
    fn set_status_missing_mascot_errors() {
        let (_d, reg) = tmp_registry();
        let err = reg.set_status("Ghost", MascotStatus::Active);
        assert!(matches!(err, Err(MakakooError::NotFound(_))));
    }

    #[test]
    fn list_active_excludes_retired() {
        let (_d, reg) = tmp_registry();
        reg.set_status("Cinder", MascotStatus::Retired).unwrap();
        let active_names: Vec<String> =
            reg.list_active().into_iter().map(|m| m.name).collect();
        assert!(!active_names.contains(&"Cinder".to_string()));
        assert_eq!(reg.list_active().len(), 4);
    }

    #[test]
    fn mascot_json_roundtrip_is_identity() {
        let m = Mascot {
            name: "Echo".into(),
            species: "bat".into(),
            maintainer: "@user".into(),
            job: "audio integrity".into(),
            voice: MascotVoice {
                greeting: "g".into(),
                alert: "a".into(),
                success: "s".into(),
                sleeping: "z".into(),
            },
            patrol_interval_hours: 5,
            created_at: Utc::now(),
            status: MascotStatus::Hatching,
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Mascot = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    /// Legacy Python Tamagotchi schema must load cleanly via serde
    /// defaults. Legacy `data/nursery.json` files look like this; an
    /// earlier Rust loader errored with `missing field "maintainer"`.
    #[test]
    fn load_parses_legacy_python_nursery_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nursery.json");
        let legacy = r#"{
  "mascots": [
    {
      "mascot_id": "a2c57d29",
      "species_name": "Misttooth",
      "name": "Pixel",
      "personality": "hilariously dramatic",
      "rarity": "Common",
      "is_shiny": false,
      "species": "raven",
      "born_at": 1775718310.771816,
      "stats": {"DEBUGGING": 53}
    },
    {
      "mascot_id": "be695c6b",
      "species_name": "Lunanub",
      "name": "Glimmer",
      "species": "eagle",
      "born_at": 1775718999.0
    }
  ],
  "updated_at": "2026-04-10T12:00:00Z"
}"#;
        std::fs::write(&path, legacy).unwrap();
        let reg = MascotRegistry::load(&path).unwrap();
        let pixel = reg.get("Pixel").expect("Pixel loads");
        assert_eq!(pixel.species, "raven");
        assert_eq!(pixel.maintainer, ""); // serde default
        assert_eq!(pixel.job, ""); // serde default
        assert_eq!(pixel.patrol_interval_hours, 2); // serde default
        assert_eq!(pixel.status, MascotStatus::Active); // serde default
        assert_eq!(pixel.voice, MascotVoice::default()); // serde default
        assert!(reg.get("Glimmer").is_some());
        // Both legacy mascots must appear alongside zero canonicals (file
        // existed, so seeding is skipped).
        assert_eq!(reg.all().len(), 2);
    }

    #[test]
    fn all_is_sorted_by_name() {
        let (_d, reg) = tmp_registry();
        let names: Vec<String> = reg.all().into_iter().map(|m| m.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
