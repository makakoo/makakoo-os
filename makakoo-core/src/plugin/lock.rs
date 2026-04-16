//! `plugins.lock` — source of truth for what's installed.
//!
//! Spec: `spec/DISTRO.md §9`. Every `makakoo plugin install|uninstall` and
//! every `makakoo distro install` updates this file so `makakoo plugin list`
//! and `makakoo distro update` can reason about the live set without
//! re-walking the registry.
//!
//! Path: `$MAKAKOO_HOME/config/plugins.lock`. TOML, human-readable,
//! git-friendly.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LockError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error on {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("serialize error: {source}")]
    Serialize {
        #[source]
        source: toml::ser::Error,
    },
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PluginsLock {
    #[serde(default)]
    pub meta: LockMeta,

    #[serde(default, rename = "plugin")]
    pub plugins: Vec<LockEntry>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LockMeta {
    /// Active distro name (if any). Empty when the user installed
    /// plugins ad-hoc without a distro.
    #[serde(default)]
    pub distro: Option<String>,
    /// Kernel version that wrote this lock (informational).
    #[serde(default)]
    pub kernel_version: Option<String>,
    /// Timestamp the lock was last written. RFC3339.
    #[serde(default)]
    pub generated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LockEntry {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub blake3: Option<String>,
    /// Source descriptor — e.g. `path:plugins-core/foo`,
    /// `git:https://github.com/x/y@v1.0`, `local:/tmp/plug`. Human-read.
    pub source: String,
    pub installed_at: DateTime<Utc>,
}

/// Canonical lock path under `$MAKAKOO_HOME`.
pub fn lock_path(makakoo_home: &Path) -> PathBuf {
    makakoo_home.join("config").join("plugins.lock")
}

impl PluginsLock {
    /// Load the lock file. Returns an empty lock if the file doesn't
    /// exist — fresh install case.
    pub fn load(makakoo_home: &Path) -> Result<Self, LockError> {
        let path = lock_path(makakoo_home);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path).map_err(|source| LockError::Io {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&raw).map_err(|source| LockError::Parse { path, source })
    }

    /// Write the lock file atomically. Creates parent dirs as needed.
    pub fn save(&self, makakoo_home: &Path) -> Result<(), LockError> {
        let path = lock_path(makakoo_home);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| LockError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let rendered = toml::to_string_pretty(self)
            .map_err(|source| LockError::Serialize { source })?;

        // Write to a sibling tmp file and rename so readers never see a
        // half-written file.
        let tmp = path.with_extension("lock.tmp");
        std::fs::write(&tmp, rendered).map_err(|source| LockError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &path).map_err(|source| LockError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(())
    }

    /// Insert or update an entry by name. Returns the previous entry if any.
    pub fn upsert(&mut self, entry: LockEntry) -> Option<LockEntry> {
        if let Some(slot) = self.plugins.iter_mut().find(|e| e.name == entry.name) {
            let prev = slot.clone();
            *slot = entry;
            Some(prev)
        } else {
            self.plugins.push(entry);
            None
        }
    }

    /// Remove a plugin by name. Returns the removed entry, or `None` if
    /// not present.
    pub fn remove(&mut self, name: &str) -> Option<LockEntry> {
        let pos = self.plugins.iter().position(|e| e.name == name)?;
        Some(self.plugins.remove(pos))
    }

    pub fn get(&self, name: &str) -> Option<&LockEntry> {
        self.plugins.iter().find(|e| e.name == name)
    }

    /// Mark the active distro and stamp the timestamp. Called by
    /// `distro install` before `save`.
    pub fn touch_meta(&mut self, distro: Option<String>, kernel_version: Option<String>) {
        self.meta.distro = distro;
        self.meta.kernel_version = kernel_version;
        self.meta.generated_at = Some(Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(name: &str, version: &str) -> LockEntry {
        LockEntry {
            name: name.into(),
            version: version.into(),
            blake3: Some("a".repeat(64)),
            source: "path:plugins-core/test".into(),
            installed_at: Utc::now(),
        }
    }

    #[test]
    fn load_missing_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let l = PluginsLock::load(tmp.path()).unwrap();
        assert!(l.plugins.is_empty());
        assert!(l.meta.distro.is_none());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mut lock = PluginsLock::default();
        lock.touch_meta(Some("core".into()), Some("0.1.0".into()));
        lock.upsert(entry("aa-plugin", "1.0.0"));
        lock.upsert(entry("bb-plugin", "2.3.4"));
        lock.save(tmp.path()).unwrap();

        let loaded = PluginsLock::load(tmp.path()).unwrap();
        assert_eq!(loaded.plugins.len(), 2);
        assert_eq!(loaded.meta.distro.as_deref(), Some("core"));
        assert!(loaded.get("aa-plugin").is_some());
    }

    #[test]
    fn upsert_replaces_existing() {
        let mut lock = PluginsLock::default();
        lock.upsert(entry("aa-plugin", "1.0.0"));
        let prev = lock.upsert(entry("aa-plugin", "1.1.0"));
        assert!(prev.is_some());
        assert_eq!(lock.plugins.len(), 1);
        assert_eq!(lock.get("aa-plugin").unwrap().version, "1.1.0");
    }

    #[test]
    fn remove_returns_none_for_missing() {
        let mut lock = PluginsLock::default();
        assert!(lock.remove("ghost").is_none());
        lock.upsert(entry("aa-plugin", "1.0.0"));
        let removed = lock.remove("aa-plugin");
        assert!(removed.is_some());
        assert!(lock.plugins.is_empty());
    }

    #[test]
    fn save_writes_under_config_dir() {
        let tmp = TempDir::new().unwrap();
        let lock = PluginsLock::default();
        lock.save(tmp.path()).unwrap();
        assert!(tmp.path().join("config/plugins.lock").exists());
    }
}
