//! Trust ledger — `~/.makakoo/trust/adapters.json`.
//!
//! Each trust entry pins the adapter's canonical manifest hash, version,
//! timestamp, and a revocation flag. On `adapter update`, the ledger is
//! consulted to detect capability/security diffs so the user can re-prompt
//! (Flow 2 in the sprint doc; direct Cursor-CVE mitigation).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::manifest::Manifest;
use super::sign::default_trust_root;

const LEDGER_FILENAME: &str = "adapters.json";

#[derive(Debug, Error)]
pub enum TrustError {
    #[error("failed to read trust ledger {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write trust ledger {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("trust ledger {path} is not valid JSON: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustEntry {
    /// sha256 of the canonical manifest JSON. Changes on any manifest
    /// diff — even whitespace-equivalent reorderings — so the ledger
    /// captures the exact bytes the user trusted.
    pub manifest_hash: String,
    pub version: String,
    pub trusted_at: DateTime<Utc>,
    /// When set, this adapter was flagged as untrusted (e.g. a sandbox
    /// escape attempt). Kept in the ledger so operators can audit history;
    /// the registry refuses to run revoked adapters.
    #[serde(default)]
    pub revoked: bool,
    /// Publisher id (`security.signed_by` at trust time) — left empty
    /// for unsigned local installs.
    #[serde(default)]
    pub publisher: Option<String>,
    /// Capability snapshot at trust time — used to diff against updated
    /// manifests and decide whether to re-prompt.
    pub capabilities_snapshot: CapSnapshot,
    /// Security snapshot at trust time.
    pub security_snapshot: SecSnapshot,
    /// User-supplied freeform notes.
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CapSnapshot {
    pub features: Vec<String>,
    pub models: Vec<String>,
    pub supports_roles: Vec<String>,
    pub max_context: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SecSnapshot {
    pub allowed_hosts: Vec<String>,
    pub sandbox_profile: String,
    pub requires_network: bool,
    pub requires_filesystem: Vec<String>,
    pub requires_env: Vec<String>,
    pub signed_by: Option<String>,
}

impl CapSnapshot {
    pub fn from_manifest(m: &Manifest) -> Self {
        let mut s = Self {
            features: m.capabilities.features.clone(),
            models: m.capabilities.models.clone(),
            supports_roles: m
                .capabilities
                .supports_roles
                .iter()
                .map(|r| r.as_str().to_string())
                .collect(),
            max_context: m.capabilities.max_context,
        };
        s.features.sort();
        s.models.sort();
        s.supports_roles.sort();
        s
    }
}

impl SecSnapshot {
    pub fn from_manifest(m: &Manifest) -> Self {
        let mut s = Self {
            allowed_hosts: m.security.allowed_hosts.clone(),
            sandbox_profile: format!("{:?}", m.security.sandbox_profile).to_ascii_lowercase(),
            requires_network: m.security.requires_network,
            requires_filesystem: m.security.requires_filesystem.clone(),
            requires_env: m.security.requires_env.clone(),
            signed_by: m.security.signed_by.clone(),
        };
        s.allowed_hosts.sort();
        s.requires_filesystem.sort();
        s.requires_env.sort();
        s
    }
}

/// Full trust ledger — a map of adapter name → trust entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustLedger {
    #[serde(flatten)]
    pub by_name: BTreeMap<String, TrustEntry>,
    #[serde(skip)]
    path: PathBuf,
}

impl TrustLedger {
    pub fn default_path() -> PathBuf {
        default_trust_root().join(LEDGER_FILENAME)
    }

    pub fn load_from(path: impl Into<PathBuf>) -> Result<Self, TrustError> {
        let path = path.into();
        if !path.exists() {
            return Ok(Self {
                by_name: BTreeMap::new(),
                path,
            });
        }
        let body = fs::read_to_string(&path).map_err(|e| TrustError::Read {
            path: path.clone(),
            source: e,
        })?;
        let mut ledger: TrustLedger =
            serde_json::from_str(&body).map_err(|e| TrustError::Parse {
                path: path.clone(),
                source: e,
            })?;
        ledger.path = path;
        Ok(ledger)
    }

    pub fn load_default() -> Result<Self, TrustError> {
        Self::load_from(Self::default_path())
    }

    pub fn save(&self) -> Result<(), TrustError> {
        if let Some(parent) = self.path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| TrustError::Write {
                    path: self.path.clone(),
                    source: e,
                })?;
            }
        }
        let body = serde_json::to_string_pretty(self).expect("serializes");
        fs::write(&self.path, body).map_err(|e| TrustError::Write {
            path: self.path.clone(),
            source: e,
        })
    }

    pub fn get(&self, name: &str) -> Option<&TrustEntry> {
        self.by_name.get(name)
    }

    pub fn set(&mut self, name: impl Into<String>, entry: TrustEntry) {
        self.by_name.insert(name.into(), entry);
    }

    pub fn remove(&mut self, name: &str) -> Option<TrustEntry> {
        self.by_name.remove(name)
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }
}

/// Snapshot a manifest for the first-trust moment.
pub fn trust_entry_from_manifest(
    manifest: &Manifest,
    notes: impl Into<String>,
) -> TrustEntry {
    TrustEntry {
        manifest_hash: manifest.canonical_hash(),
        version: manifest.adapter.version.to_string(),
        trusted_at: Utc::now(),
        revoked: false,
        publisher: manifest.security.signed_by.clone(),
        capabilities_snapshot: CapSnapshot::from_manifest(manifest),
        security_snapshot: SecSnapshot::from_manifest(manifest),
        notes: notes.into(),
    }
}

/// Diff report produced when re-trusting after an update.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManifestDiff {
    pub hash_changed: bool,
    pub version_changed: Option<(String, String)>,
    pub features_added: Vec<String>,
    pub features_removed: Vec<String>,
    pub allowed_hosts_added: Vec<String>,
    pub allowed_hosts_removed: Vec<String>,
    pub sandbox_changed: Option<(String, String)>,
    pub requires_network_changed: Option<(bool, bool)>,
    pub requires_filesystem_added: Vec<String>,
    pub requires_filesystem_removed: Vec<String>,
    pub signed_by_changed: Option<(Option<String>, Option<String>)>,
    pub supports_roles_added: Vec<String>,
    pub supports_roles_removed: Vec<String>,
}

impl ManifestDiff {
    pub fn is_empty(&self) -> bool {
        *self == ManifestDiff::default()
    }

    /// True if the diff contains any change that requires re-trust.
    pub fn requires_re_trust(&self) -> bool {
        !self.is_empty()
    }
}

/// Diff a trust entry against a new manifest. Empty diff means the ledger
/// entry still matches the new manifest exactly.
pub fn diff_manifest(entry: &TrustEntry, new: &Manifest) -> ManifestDiff {
    let new_cap = CapSnapshot::from_manifest(new);
    let new_sec = SecSnapshot::from_manifest(new);
    let new_hash = new.canonical_hash();
    let new_version = new.adapter.version.to_string();
    ManifestDiff {
        hash_changed: entry.manifest_hash != new_hash,
        version_changed: if entry.version != new_version {
            Some((entry.version.clone(), new_version))
        } else {
            None
        },
        features_added: added(&entry.capabilities_snapshot.features, &new_cap.features),
        features_removed: added(&new_cap.features, &entry.capabilities_snapshot.features),
        supports_roles_added: added(
            &entry.capabilities_snapshot.supports_roles,
            &new_cap.supports_roles,
        ),
        supports_roles_removed: added(
            &new_cap.supports_roles,
            &entry.capabilities_snapshot.supports_roles,
        ),
        allowed_hosts_added: added(
            &entry.security_snapshot.allowed_hosts,
            &new_sec.allowed_hosts,
        ),
        allowed_hosts_removed: added(
            &new_sec.allowed_hosts,
            &entry.security_snapshot.allowed_hosts,
        ),
        sandbox_changed: if entry.security_snapshot.sandbox_profile != new_sec.sandbox_profile {
            Some((
                entry.security_snapshot.sandbox_profile.clone(),
                new_sec.sandbox_profile.clone(),
            ))
        } else {
            None
        },
        requires_network_changed: if entry.security_snapshot.requires_network
            != new_sec.requires_network
        {
            Some((
                entry.security_snapshot.requires_network,
                new_sec.requires_network,
            ))
        } else {
            None
        },
        requires_filesystem_added: added(
            &entry.security_snapshot.requires_filesystem,
            &new_sec.requires_filesystem,
        ),
        requires_filesystem_removed: added(
            &new_sec.requires_filesystem,
            &entry.security_snapshot.requires_filesystem,
        ),
        signed_by_changed: if entry.security_snapshot.signed_by != new_sec.signed_by {
            Some((entry.security_snapshot.signed_by.clone(), new_sec.signed_by))
        } else {
            None
        },
    }
}

fn added(old: &[String], new: &[String]) -> Vec<String> {
    new.iter()
        .filter(|n| !old.iter().any(|o| o == *n))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Manifest;
    use std::path::Path;

    const MANIFEST: &str = r#"
[adapter]
name = "openclaw"
version = "1.4.2"
manifest_schema = 1
description = "d"

[compatibility]
bridge_version = "^2.0"
protocols = ["openai-chat-v1"]

[transport]
kind = "openai-compatible"
base_url = "http://127.0.0.1:3000/v1"

[auth]
scheme = "bearer"
key_env = "OPENCLAW_API_KEY"

[output]
format = "lope-verdict-block"

[capabilities]
features = ["tool_use"]
supports_roles = ["validator"]

[install]
source_type = "git"
source = "https://example.com/x.git"
ref = "v1.4.2"

[security]
requires_network = true
allowed_hosts = ["127.0.0.1"]
sandbox_profile = "network-io"
signed_by = "traylinx"
"#;

    fn write_ledger(dir: &Path) -> TrustLedger {
        let path = dir.join("adapters.json");
        TrustLedger {
            by_name: BTreeMap::new(),
            path,
        }
    }

    #[test]
    fn empty_ledger_saves_and_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let l = write_ledger(tmp.path());
        l.save().unwrap();
        let reloaded = TrustLedger::load_from(tmp.path().join("adapters.json")).unwrap();
        assert!(reloaded.is_empty());
    }

    #[test]
    fn single_entry_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Manifest::parse_str(MANIFEST).unwrap();
        let mut l = write_ledger(tmp.path());
        l.set("openclaw", trust_entry_from_manifest(&m, "first install"));
        l.save().unwrap();
        let reloaded = TrustLedger::load_from(tmp.path().join("adapters.json")).unwrap();
        let e = reloaded.get("openclaw").unwrap();
        assert_eq!(e.version, "1.4.2");
        assert_eq!(e.manifest_hash, m.canonical_hash());
        assert!(!e.revoked);
    }

    #[test]
    fn diff_empty_when_identical() {
        let m = Manifest::parse_str(MANIFEST).unwrap();
        let entry = trust_entry_from_manifest(&m, "");
        let diff = diff_manifest(&entry, &m);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_detects_feature_addition() {
        let m1 = Manifest::parse_str(MANIFEST).unwrap();
        let entry = trust_entry_from_manifest(&m1, "");
        let m2_src = MANIFEST.replace(
            r#"features = ["tool_use"]"#,
            r#"features = ["tool_use", "fs_write"]"#,
        );
        let m2 = Manifest::parse_str(&m2_src).unwrap();
        let diff = diff_manifest(&entry, &m2);
        assert!(!diff.is_empty());
        assert_eq!(diff.features_added, vec!["fs_write".to_string()]);
        assert!(diff.requires_re_trust());
    }

    #[test]
    fn diff_detects_allowed_host_addition() {
        let m1 = Manifest::parse_str(MANIFEST).unwrap();
        let entry = trust_entry_from_manifest(&m1, "");
        let m2_src = MANIFEST.replace(
            r#"allowed_hosts = ["127.0.0.1"]"#,
            r#"allowed_hosts = ["127.0.0.1", "evil.example"]"#,
        );
        let m2 = Manifest::parse_str(&m2_src).unwrap();
        let diff = diff_manifest(&entry, &m2);
        assert_eq!(
            diff.allowed_hosts_added,
            vec!["evil.example".to_string()]
        );
    }

    #[test]
    fn diff_detects_sandbox_change() {
        let m1 = Manifest::parse_str(MANIFEST).unwrap();
        let entry = trust_entry_from_manifest(&m1, "");
        let m2_src = MANIFEST.replace(r#"sandbox_profile = "network-io""#, r#"sandbox_profile = "none""#);
        let m2 = Manifest::parse_str(&m2_src).unwrap();
        let diff = diff_manifest(&entry, &m2);
        assert_eq!(
            diff.sandbox_changed,
            Some(("networkio".into(), "none".into()))
        );
    }

    #[test]
    fn diff_detects_version_bump() {
        let m1 = Manifest::parse_str(MANIFEST).unwrap();
        let entry = trust_entry_from_manifest(&m1, "");
        let m2_src = MANIFEST.replace(r#"version = "1.4.2""#, r#"version = "1.5.0""#)
            .replace(r#"ref = "v1.4.2""#, r#"ref = "v1.5.0""#);
        let m2 = Manifest::parse_str(&m2_src).unwrap();
        let diff = diff_manifest(&entry, &m2);
        assert_eq!(
            diff.version_changed,
            Some(("1.4.2".into(), "1.5.0".into()))
        );
        assert!(diff.hash_changed);
    }
}
