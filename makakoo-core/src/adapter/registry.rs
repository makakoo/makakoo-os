//! Registered-adapter walker.
//!
//! Reads `~/.makakoo/adapters/registered/*.toml` (or an override root) into
//! typed `Manifest` values keyed by adapter name. Malformed manifests are
//! logged and skipped — the registry never refuses to boot because one
//! adapter's file is broken.
//!
//! Phase A ships the read side only. Install/update/remove lifecycle
//! (Phase C) writes into the same directory via atomic swap.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::warn;

use super::manifest::{AdapterRole, Manifest, ManifestError};

const REGISTERED_DIRNAME: &str = "registered";

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("failed to read adapter registry dir {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no adapter registered as `{0}`")]
    NotFound(String),
}

/// One entry in the registry: the parsed manifest plus where on disk it
/// came from.
#[derive(Debug, Clone)]
pub struct RegisteredAdapter {
    pub manifest: Manifest,
    pub manifest_path: PathBuf,
}

impl RegisteredAdapter {
    pub fn name(&self) -> &str {
        &self.manifest.adapter.name
    }
}

/// Read-only view of `~/.makakoo/adapters/registered/`.
#[derive(Debug, Clone)]
pub struct AdapterRegistry {
    root: PathBuf,
    by_name: BTreeMap<String, RegisteredAdapter>,
}

impl AdapterRegistry {
    /// Resolve the default registry dir under `$MAKAKOO_HOME/adapters/registered`.
    /// `$MAKAKOO_HOME` falls back to `$HARVEY_HOME`, then `~/MAKAKOO`, then
    /// the user's home directory.
    pub fn default_root() -> PathBuf {
        default_adapters_root().join(REGISTERED_DIRNAME)
    }

    /// Load every `.toml` under `root` into memory.
    pub fn load(root: impl AsRef<Path>) -> Result<AdapterRegistry, RegistryError> {
        let root = root.as_ref().to_path_buf();
        let mut by_name = BTreeMap::new();

        if !root.exists() {
            return Ok(AdapterRegistry { root, by_name });
        }

        let entries = std::fs::read_dir(&root).map_err(|e| RegistryError::Io {
            path: root.clone(),
            source: e,
        })?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "adapter registry: bad dir entry");
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            match Manifest::load(&path) {
                Ok(manifest) => {
                    if let Some(prev) = by_name.insert(
                        manifest.adapter.name.clone(),
                        RegisteredAdapter {
                            manifest,
                            manifest_path: path.clone(),
                        },
                    ) {
                        warn!(
                            name = %prev.name(),
                            old = %prev.manifest_path.display(),
                            new = %path.display(),
                            "adapter registry: duplicate name — later file wins"
                        );
                    }
                }
                Err(ManifestError::Toml { .. } | ManifestError::Invalid { .. }) => {
                    warn!(path = %path.display(), "adapter registry: malformed manifest skipped");
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "adapter registry: skipped");
                }
            }
        }

        Ok(AdapterRegistry { root, by_name })
    }

    /// Load from `AdapterRegistry::default_root()` — never errors if the
    /// dir is missing (returns empty).
    pub fn load_default() -> Result<AdapterRegistry, RegistryError> {
        Self::load(Self::default_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(|s| s.as_str())
    }

    pub fn list(&self) -> impl Iterator<Item = &RegisteredAdapter> {
        self.by_name.values()
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredAdapter> {
        self.by_name.get(name)
    }

    pub fn require(&self, name: &str) -> Result<&RegisteredAdapter, RegistryError> {
        self.get(name)
            .ok_or_else(|| RegistryError::NotFound(name.to_string()))
    }

    /// All adapters that advertise the given role.
    pub fn resolve_by_role(&self, role: AdapterRole) -> Vec<&RegisteredAdapter> {
        self.by_name
            .values()
            .filter(|a| a.manifest.supports_role(role))
            .collect()
    }
}

fn default_adapters_root() -> PathBuf {
    if let Ok(p) = std::env::var("MAKAKOO_ADAPTERS_HOME") {
        return PathBuf::from(p);
    }
    // Adapter config lives under `~/.makakoo/` (user-level, like `~/.aws/`),
    // not under `$MAKAKOO_HOME` (the platform project root). Falls back to
    // the current dir if no home can be determined, which is harmless: the
    // registry just returns empty.
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".makakoo")
        .join("adapters")
}

// ── primary-adapter primitive ───────────────────────────────────────────
//
// Names which registered adapter is the "primary" — the one the wizard
// (and, eventually, routing consumers) treats as the default. Separate
// from `Enabled`/`Disabled` toggles: several adapters can be enabled,
// exactly zero or one is primary at any time.
//
// File: `~/.makakoo/primary_adapter.toml`. Minimal schema:
//
//     name = "switchailocal"
//
// The file is optional — absence means "no primary has been picked yet"
// and consumers fall back to their pre-existing default behavior.

/// Absolute path to `primary_adapter.toml`. Respects
/// `MAKAKOO_ADAPTERS_HOME` so tests can redirect writes.
pub fn primary_adapter_path() -> PathBuf {
    // Adapters home is `~/.makakoo/adapters/` by default; primary marker
    // sits one level up (`~/.makakoo/`) alongside `trust/` so it's
    // trivially visible in a directory listing.
    let adapters_root = default_adapters_root();
    match adapters_root.parent() {
        Some(p) => p.join("primary_adapter.toml"),
        None => PathBuf::from("primary_adapter.toml"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PrimaryAdapterFile {
    name: String,
}

/// Read the primary-adapter name from disk. Returns `None` when the
/// file is missing, unreadable, malformed, or names an adapter not in
/// the provided registry. Never panics — consumers can trust the result.
pub fn load_primary_adapter(registry: &AdapterRegistry) -> Option<String> {
    let path = primary_adapter_path();
    load_primary_adapter_from(&path, registry)
}

/// Same as [`load_primary_adapter`] but reads from an explicit path.
/// Separates the filesystem-IO seam from the default-path lookup so
/// unit tests can target it directly without env-var trickery.
pub fn load_primary_adapter_from(path: &Path, registry: &AdapterRegistry) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: PrimaryAdapterFile = toml::from_str(&raw).ok()?;
    if registry.get(&parsed.name).is_some() {
        Some(parsed.name)
    } else {
        None
    }
}

#[derive(Debug, Error)]
pub enum PrimaryAdapterError {
    #[error("unknown adapter `{0}` — not registered in {1}")]
    UnknownAdapter(String, PathBuf),
    #[error("failed to write primary_adapter.toml at {path}: {source}")]
    WriteFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize primary_adapter.toml: {0}")]
    SerializeFailed(#[from] toml::ser::Error),
}

/// Atomically persist `name` as the primary adapter.
///
/// - Rejects names that aren't in the registry.
/// - Creates parent dirs on demand.
/// - Writes to a sibling `*.tmp` then renames — safe under interrupt.
pub fn write_primary_adapter(
    name: &str,
    registry: &AdapterRegistry,
) -> Result<PathBuf, PrimaryAdapterError> {
    let path = primary_adapter_path();
    write_primary_adapter_to(&path, name, registry)?;
    Ok(path)
}

/// Same as [`write_primary_adapter`] but writes to an explicit path.
pub fn write_primary_adapter_to(
    path: &Path,
    name: &str,
    registry: &AdapterRegistry,
) -> Result<(), PrimaryAdapterError> {
    if registry.get(name).is_none() {
        return Err(PrimaryAdapterError::UnknownAdapter(
            name.to_string(),
            registry.root().to_path_buf(),
        ));
    }
    let body = toml::to_string(&PrimaryAdapterFile {
        name: name.to_string(),
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PrimaryAdapterError::WriteFailed {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body).map_err(|e| PrimaryAdapterError::WriteFailed {
        path: tmp.clone(),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| PrimaryAdapterError::WriteFailed {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const SAMPLE: &str = r#"
[adapter]
name = "ref-adapter"
version = "0.1.0"
manifest_schema = 1
description = "sample"

[compatibility]
bridge_version = "^2.0"
protocols = ["openai-chat-v1"]

[transport]
kind = "openai-compatible"
base_url = "http://127.0.0.1:3000/v1"

[auth]
scheme = "none"

[output]
format = "lope-verdict-block"

[capabilities]
supports_roles = ["validator", "delegate"]

[install]
source_type = "local"

[security]
requires_network = false
sandbox_profile = "network-io"
"#;

    fn write(dir: &Path, file: &str, body: &str) {
        fs::write(dir.join(file), body).unwrap();
    }

    #[test]
    fn empty_registry_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let reg = AdapterRegistry::load(&missing).unwrap();
        assert!(reg.is_empty());
    }

    #[test]
    fn loads_single_adapter() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ref-adapter.toml", SAMPLE);
        let reg = AdapterRegistry::load(tmp.path()).unwrap();
        assert_eq!(reg.len(), 1);
        let a = reg.require("ref-adapter").unwrap();
        assert_eq!(a.name(), "ref-adapter");
    }

    #[test]
    fn resolves_by_role() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ref-adapter.toml", SAMPLE);

        let other = SAMPLE
            .replace(r#"name = "ref-adapter""#, r#"name = "swarm-only""#)
            .replace(
                r#"supports_roles = ["validator", "delegate"]"#,
                r#"supports_roles = ["swarm_member"]"#,
            );
        write(tmp.path(), "swarm-only.toml", &other);

        let reg = AdapterRegistry::load(tmp.path()).unwrap();
        assert_eq!(reg.len(), 2);

        let validators: Vec<_> = reg
            .resolve_by_role(AdapterRole::Validator)
            .into_iter()
            .map(|a| a.name().to_string())
            .collect();
        assert_eq!(validators, vec!["ref-adapter".to_string()]);

        let swarmers: Vec<_> = reg
            .resolve_by_role(AdapterRole::SwarmMember)
            .into_iter()
            .map(|a| a.name().to_string())
            .collect();
        assert_eq!(swarmers, vec!["swarm-only".to_string()]);
    }

    #[test]
    fn malformed_manifest_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ref-adapter.toml", SAMPLE);
        write(tmp.path(), "broken.toml", "{ this is not toml at all");
        let reg = AdapterRegistry::load(tmp.path()).unwrap();
        assert_eq!(reg.len(), 1);
        assert!(reg.get("ref-adapter").is_some());
    }

    #[test]
    fn non_toml_files_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ref-adapter.toml", SAMPLE);
        write(tmp.path(), "README.md", "not a manifest");
        write(tmp.path(), "something.txt", "also not");
        let reg = AdapterRegistry::load(tmp.path()).unwrap();
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn not_found_errors_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = AdapterRegistry::load(tmp.path()).unwrap();
        let err = reg.require("nope").unwrap_err();
        assert!(matches!(err, RegistryError::NotFound(_)));
    }

    // ── primary_adapter_path / load / write ────────────────────────────

    fn registry_with_adapters(dir: &Path, names: &[&str]) -> AdapterRegistry {
        for name in names {
            let body = SAMPLE.replace(r#"name = "ref-adapter""#, &format!(r#"name = "{name}""#));
            write(dir, &format!("{name}.toml"), &body);
        }
        AdapterRegistry::load(dir).unwrap()
    }

    #[test]
    fn load_primary_returns_none_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal"]);
        let path = tmp.path().join("no-such-file.toml");
        assert_eq!(load_primary_adapter_from(&path, &reg), None);
    }

    #[test]
    fn load_primary_returns_none_when_points_to_unknown_adapter() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal"]);
        let path = tmp.path().join("primary.toml");
        fs::write(&path, r#"name = "nonexistent""#).unwrap();
        assert_eq!(load_primary_adapter_from(&path, &reg), None);
    }

    #[test]
    fn load_primary_returns_some_when_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal", "tytus-cli"]);
        let path = tmp.path().join("primary.toml");
        fs::write(&path, r#"name = "switchailocal""#).unwrap();
        assert_eq!(
            load_primary_adapter_from(&path, &reg),
            Some("switchailocal".to_string())
        );
    }

    #[test]
    fn load_primary_returns_none_on_malformed_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal"]);
        let path = tmp.path().join("primary.toml");
        fs::write(&path, "this is not valid toml [").unwrap();
        assert_eq!(load_primary_adapter_from(&path, &reg), None);
    }

    #[test]
    fn write_primary_rejects_unknown_name() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal"]);
        let path = tmp.path().join("primary.toml");
        let err = write_primary_adapter_to(&path, "nonexistent", &reg).unwrap_err();
        assert!(matches!(err, PrimaryAdapterError::UnknownAdapter(..)));
        assert!(!path.exists(), "no file should be written on error");
    }

    #[test]
    fn write_primary_happy_path_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal", "tytus-cli"]);
        let path = tmp.path().join("primary.toml");
        write_primary_adapter_to(&path, "tytus-cli", &reg).unwrap();
        assert!(path.exists());
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(r#"name = "tytus-cli""#));
    }

    #[test]
    fn write_primary_is_atomic_no_leftover_tmp() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal"]);
        let path = tmp.path().join("primary.toml");
        write_primary_adapter_to(&path, "switchailocal", &reg).unwrap();
        assert!(path.exists());
        let tmp_sibling = path.with_extension("toml.tmp");
        assert!(!tmp_sibling.exists(), "tmp sibling should be renamed away");
    }

    #[test]
    fn write_primary_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal", "tytus-cli"]);
        let path = tmp.path().join("primary.toml");
        write_primary_adapter_to(&path, "switchailocal", &reg).unwrap();
        write_primary_adapter_to(&path, "tytus-cli", &reg).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(r#"name = "tytus-cli""#));
        assert!(!body.contains(r#"name = "switchailocal""#));
    }

    #[test]
    fn write_primary_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal"]);
        let nested = tmp.path().join("nested").join("deeper").join("primary.toml");
        write_primary_adapter_to(&nested, "switchailocal", &reg).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn round_trip_write_then_load() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = registry_with_adapters(tmp.path(), &["switchailocal"]);
        let path = tmp.path().join("primary.toml");
        write_primary_adapter_to(&path, "switchailocal", &reg).unwrap();
        assert_eq!(
            load_primary_adapter_from(&path, &reg),
            Some("switchailocal".to_string())
        );
    }
}
