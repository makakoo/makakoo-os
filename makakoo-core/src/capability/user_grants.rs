//! UserGrants — Rust writer + reader for `user_grants.json`.
//!
//! Pairs with the Python mirror at
//! `plugins-core/lib-harvey-core/src/core/capability/user_grants.py`.
//! Both implementations MUST stay schema-compatible — see
//! `spec/USER_GRANTS.md §3` for the locked field set and §5 for the
//! sidecar-lock protocol.
//!
//! Key design decisions (frozen in SPRINT.md §3):
//!
//! * **LD#9** — sidecar lock at `user_grants.json.lock`, NEVER on the
//!   data fd; released AFTER `fs::rename` completes. Acquired via
//!   `fs2::FileExt::lock_exclusive()`.
//! * **LD#4** — file is machine-local, gitignored, never synced.
//! * **Lope F4** — no `use_count` / `last_used_at` on the schema, no
//!   `record_use()` method. Audit log answers "was this grant used".
//! * **Lope F6** — `origin_turn_id` is stored but not enforcement-
//!   bound until v0.3.1.
//! * **Lope F7** — rate-limit counter lives in a separate file
//!   (`state/perms_rate_limit.json`) so a corrupt counter can't
//!   poison the grants.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

/// Current schema version (see `spec/USER_GRANTS.md §3`).
pub const SCHEMA_VERSION: u32 = 1;

/// Errors surfaced by the loader.
#[derive(Debug, Error)]
pub enum UserGrantsError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("json parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

// ═══════════════════════════════════════════════════════════════
//  UserGrant — one persisted entry
// ═══════════════════════════════════════════════════════════════

/// One runtime user grant. Field set mirrors Python `Grant` 1:1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserGrant {
    pub id: String,
    pub scope: String,
    pub created_at: DateTime<Utc>,
    /// `None` when the grant is permanent.
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_granted_by")]
    pub granted_by: String,
    #[serde(default = "default_plugin")]
    pub plugin: String,
    /// Host-provided turn identifier. Enforcement-binding deferred to
    /// v0.3.1 (lope F6).
    #[serde(default)]
    pub origin_turn_id: String,
}

fn default_granted_by() -> String {
    "sebastian".into()
}
fn default_plugin() -> String {
    "cli".into()
}

impl UserGrant {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.map_or(false, |e| e <= now)
    }

    /// Match `abs_path` against this grant's scope glob. Only
    /// `fs/write:<glob>` scopes are checked; other verbs return false
    /// because v0.3 only gates write.
    pub fn matches_path(&self, abs_path: &str) -> bool {
        let Some(glob) = self.scope.strip_prefix("fs/write:") else {
            return false;
        };
        glob_match(glob, abs_path)
    }
}

// ═══════════════════════════════════════════════════════════════
//  UserGrants — on-disk collection
// ═══════════════════════════════════════════════════════════════

/// Typed handle to `$MAKAKOO_HOME/config/user_grants.json`.
///
/// Reads are lock-free (§5.4). Writes go through `save()`, which
/// acquires the sidecar lock, writes the new blob to `.json.tmp`,
/// atomic-renames onto the data file, and releases the lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserGrants {
    pub version: u32,
    pub grants: Vec<UserGrant>,
    #[serde(skip, default)]
    path: PathBuf,
}

impl UserGrants {
    // ── constructors ────────────────────────────────────────
    pub fn empty_at(path: PathBuf) -> Self {
        Self {
            version: SCHEMA_VERSION,
            grants: Vec::new(),
            path,
        }
    }

    /// Load from the canonical path under `home` —
    /// `$home/config/user_grants.json`. Tolerates missing + corrupt
    /// files; both return an empty store and log.
    pub fn load(home: &Path) -> Self {
        let path = default_path(home);
        Self::load_at(&path)
    }

    /// Load from an explicit path. Mostly for tests.
    pub fn load_at(path: &Path) -> Self {
        if !path.exists() {
            info!("loaded 0 user grants (no file at {})", path.display());
            return Self::empty_at(path.to_path_buf());
        }
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "could not read user_grants.json at {}: {}; empty",
                    path.display(),
                    e
                );
                return Self::empty_at(path.to_path_buf());
            }
        };
        let mut parsed: UserGrants = match serde_json::from_slice(&bytes) {
            Ok(u) => u,
            Err(e) => {
                warn!(
                    "corrupt user_grants.json at {}: {}; falling back to empty",
                    path.display(),
                    e
                );
                return Self::empty_at(path.to_path_buf());
            }
        };
        if parsed.version != SCHEMA_VERSION {
            warn!(
                "user_grants.json version={} (loader expects {}); best-effort parse",
                parsed.version, SCHEMA_VERSION
            );
        }
        parsed.path = path.to_path_buf();
        info!(
            "loaded {} user grants from {}",
            parsed.grants.len(),
            path.display()
        );
        parsed
    }

    // ── predicates / accessors ──────────────────────────────
    pub fn active_grants(&self, now: DateTime<Utc>) -> Vec<&UserGrant> {
        self.grants.iter().filter(|g| !g.is_expired(now)).collect()
    }

    /// Return the first active grant whose scope-glob matches.
    ///
    /// `_plugin` is accepted for forward compatibility with v0.4
    /// per-plugin scoping but NOT enforced in v0.3.
    pub fn match_write_path(
        &self,
        abs_path: &str,
        _plugin: Option<&str>,
        now: DateTime<Utc>,
    ) -> Option<&UserGrant> {
        self.active_grants(now)
            .into_iter()
            .find(|g| g.matches_path(abs_path))
    }

    pub fn get(&self, grant_id: &str) -> Option<&UserGrant> {
        self.grants.iter().find(|g| g.id == grant_id)
    }

    // ── mutations (callers check rate-limit first) ─────────
    /// Append one grant. Caller MUST have already checked the rate
    /// limit via `capability::rate_limit::check_and_increment`.
    pub fn add(&mut self, grant: UserGrant) {
        self.grants.push(grant);
    }

    /// Remove a grant by id. Returns true if one was removed.
    pub fn remove(&mut self, grant_id: &str) -> bool {
        let before = self.grants.len();
        self.grants.retain(|g| g.id != grant_id);
        before != self.grants.len()
    }

    /// Drop expired grants. Returns the removed list (for audit).
    pub fn purge_expired(&mut self, now: DateTime<Utc>) -> Vec<UserGrant> {
        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(self.grants.len());
        for g in self.grants.drain(..) {
            if g.is_expired(now) {
                removed.push(g);
            } else {
                kept.push(g);
            }
        }
        self.grants = kept;
        removed
    }

    // ── on-disk write (sidecar lock + atomic rename) ───────
    /// Persist to disk. Opens the sidecar lock, writes to
    /// `<path>.tmp`, atomic-renames, releases the lock. Follows the
    /// §5 contract — the data file itself is NEVER locked.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = &self.path;
        anyhow::ensure!(
            !path.as_os_str().is_empty(),
            "UserGrants has no bound path; construct via load()/empty_at()"
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let lock_path = lock_path_for(path);
        let lock_fd = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("opening lock at {}", lock_path.display()))?;
        lock_fd
            .lock_exclusive()
            .with_context(|| format!("flock exclusive on {}", lock_path.display()))?;

        let write_result: anyhow::Result<()> = (|| {
            let tmp_path = tmp_path_for(path);
            let serialized = serde_json::to_vec_pretty(self)
                .context("serializing UserGrants")?;
            {
                let mut tmp = File::create(&tmp_path)
                    .with_context(|| format!("creating {}", tmp_path.display()))?;
                tmp.write_all(&serialized)
                    .with_context(|| format!("writing {}", tmp_path.display()))?;
                tmp.sync_all().ok();
            }
            set_private_mode(&tmp_path);
            fs::rename(&tmp_path, path).with_context(|| {
                format!("rename {} → {}", tmp_path.display(), path.display())
            })?;
            Ok(())
        })();

        // Always release the lock, regardless of write result. Drop of
        // lock_fd would release anyway, but we're explicit for clarity.
        let _ = FileExt::unlock(&lock_fd);

        write_result
    }

    /// Convenience: load, mutate via closure, save. Returns the value
    /// the closure returned.
    pub fn with_mutation<R, F>(&mut self, f: F) -> anyhow::Result<R>
    where
        F: FnOnce(&mut Self) -> R,
    {
        let r = f(self);
        self.save()?;
        Ok(r)
    }
}

// ═══════════════════════════════════════════════════════════════
//  path + time helpers
// ═══════════════════════════════════════════════════════════════

pub fn default_path(home: &Path) -> PathBuf {
    home.join("config").join("user_grants.json")
}

fn lock_path_for(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".lock");
    PathBuf::from(p)
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".tmp");
    PathBuf::from(p)
}

#[cfg(unix)]
fn set_private_mode(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path) {
    // Windows is non-target — see spec/USER_GRANTS.md §5.5
}

/// `g_<yyyymmdd>_<8hex>` — see `spec/USER_GRANTS.md §3.2`.
pub fn new_grant_id(now: DateTime<Utc>) -> String {
    let date = now.format("%Y%m%d");
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!(
        "g_{}_{:02x}{:02x}{:02x}{:02x}",
        date, bytes[0], bytes[1], bytes[2], bytes[3]
    )
}

// ═══════════════════════════════════════════════════════════════
//  glob matcher (mirrors Python semantics + `capability::verb::glob_match`)
// ═══════════════════════════════════════════════════════════════

/// Match `path` against a glob with the following grammar (spec §4):
///
/// * `**` — any run of characters INCLUDING `/` (descending wildcard)
/// * `*`  — any run of characters EXCEPT `/` (single-segment wildcard)
/// * everything else — literal
///
/// Anchored to the full string.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let re_src = glob_to_regex(pattern);
    match regex::Regex::new(&re_src) {
        Ok(r) => r.is_match(path),
        Err(e) => {
            warn!("invalid glob pattern {pattern:?}: {e}; no-match");
            false
        }
    }
}

fn glob_to_regex(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() * 2 + 2);
    out.push('^');
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'*' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    out.push_str(".*");
                    i += 2;
                } else {
                    out.push_str("[^/]*");
                    i += 1;
                }
            }
            b'?' | b'.' | b'+' | b'(' | b')' | b'|' | b'^' | b'$' | b'{' | b'}'
            | b'[' | b']' | b'\\' => {
                out.push('\\');
                out.push(c as char);
                i += 1;
            }
            other => {
                out.push(other as char);
                i += 1;
            }
        }
    }
    out.push('$');
    out
}

// ═══════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use tempfile::TempDir;

    fn mk_home() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("config")).unwrap();
        fs::create_dir_all(tmp.path().join("state")).unwrap();
        tmp
    }

    fn base_grant(id: &str, scope: &str, expires: Option<DateTime<Utc>>) -> UserGrant {
        UserGrant {
            id: id.into(),
            scope: scope.into(),
            created_at: Utc.with_ymd_and_hms(2026, 4, 21, 9, 30, 0).unwrap(),
            expires_at: expires,
            label: "t".into(),
            granted_by: "sebastian".into(),
            plugin: "cli".into(),
            origin_turn_id: "".into(),
        }
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let home = mk_home();
        let u = UserGrants::load(home.path());
        assert_eq!(u.grants.len(), 0);
        assert_eq!(u.version, SCHEMA_VERSION);
    }

    #[test]
    fn load_corrupt_json_returns_empty() {
        let home = mk_home();
        let p = default_path(home.path());
        fs::write(&p, b"{not valid json at all").unwrap();
        let u = UserGrants::load(home.path());
        assert_eq!(u.grants.len(), 0);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let home = mk_home();
        let p = default_path(home.path());
        let mut u = UserGrants::empty_at(p.clone());
        u.add(base_grant(
            "g_20260421_a",
            "fs/write:/Users/sebastian/code/**",
            Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()),
        ));
        u.save().unwrap();

        let u2 = UserGrants::load(home.path());
        assert_eq!(u2.grants.len(), 1);
        assert_eq!(u2.grants[0].id, "g_20260421_a");
    }

    #[test]
    fn remove_drops_by_id() {
        let home = mk_home();
        let p = default_path(home.path());
        let mut u = UserGrants::empty_at(p.clone());
        u.add(base_grant("g1", "fs/write:/a/**", None));
        u.add(base_grant("g2", "fs/write:/b/**", None));
        assert!(u.remove("g1"));
        assert!(!u.remove("nope"));
        assert_eq!(u.grants.len(), 1);
        assert_eq!(u.grants[0].id, "g2");
    }

    #[test]
    fn purge_expired_removes_and_returns() {
        let home = mk_home();
        let p = default_path(home.path());
        let mut u = UserGrants::empty_at(p.clone());
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap();
        u.add(base_grant(
            "expired",
            "fs/write:/tmp/**",
            Some(now - Duration::minutes(10)),
        ));
        u.add(base_grant("active", "fs/write:/Users/sebastian/code/**", None));
        let removed = u.purge_expired(now);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, "expired");
        assert_eq!(u.grants.len(), 1);
        assert_eq!(u.grants[0].id, "active");
    }

    #[test]
    fn match_write_path_finds_descending_grant() {
        let home = mk_home();
        let mut u = UserGrants::empty_at(default_path(home.path()));
        u.add(base_grant(
            "g1",
            "fs/write:/Users/sebastian/code/**",
            None,
        ));
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap();
        let m = u.match_write_path("/Users/sebastian/code/src/lib.rs", None, now);
        assert!(m.is_some());
        assert_eq!(m.unwrap().id, "g1");
    }

    #[test]
    fn match_write_path_rejects_outside_scope() {
        let home = mk_home();
        let mut u = UserGrants::empty_at(default_path(home.path()));
        u.add(base_grant("g1", "fs/write:/Users/sebastian/code/**", None));
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap();
        assert!(u
            .match_write_path("/Users/sebastian/other/lib.rs", None, now)
            .is_none());
    }

    #[test]
    fn match_write_path_single_star_stops_at_slash() {
        let home = mk_home();
        let mut u = UserGrants::empty_at(default_path(home.path()));
        u.add(base_grant("g1", "fs/write:/tmp/*", None));
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap();
        assert!(u.match_write_path("/tmp/foo.md", None, now).is_some());
        assert!(u
            .match_write_path("/tmp/sub/foo.md", None, now)
            .is_none());
    }

    #[test]
    fn match_write_path_filters_expired() {
        let home = mk_home();
        let mut u = UserGrants::empty_at(default_path(home.path()));
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap();
        u.add(base_grant(
            "g1",
            "fs/write:/tmp/**",
            Some(now - Duration::minutes(1)),
        ));
        assert!(u.match_write_path("/tmp/foo.md", None, now).is_none());
    }

    #[test]
    fn new_grant_id_shape() {
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 30, 0).unwrap();
        let id = new_grant_id(now);
        assert!(id.starts_with("g_20260421_"));
        assert_eq!(id.len(), 2 + 8 + 1 + 8);
    }

    #[test]
    fn glob_vectors_shared_fixture() {
        // Drift-detection: exercise the same fixture the Python tests
        // consume at MAKAKOO/tests/test_user_grants.py.
        let fixture = include_str!("../../../tests/fixtures/grant_glob_vectors.json");
        let v: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let vectors = v["vectors"].as_array().expect("vectors array");
        let mut failures: Vec<String> = Vec::new();
        for entry in vectors {
            let name = entry["name"].as_str().unwrap_or("<unnamed>");
            let raw_scope = entry["scope_glob"].as_str().unwrap();
            let path = entry["path"].as_str().unwrap();
            let expected = entry["match"].as_bool().unwrap();
            let actual = glob_match(raw_scope, path);
            if actual != expected {
                failures.push(format!(
                    "{name}: scope={raw_scope:?} path={path:?} → {actual} (expected {expected})"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "shared-fixture drift ({} cases):\n  - {}",
            failures.len(),
            failures.join("\n  - ")
        );
    }

    #[test]
    fn dropped_malformed_entries_are_handled_via_serde_default() {
        // A grant with missing optional fields still loads via serde
        // defaults. A grant with missing REQUIRED fields (no created_at)
        // fails serde_json parse and we fall back to empty.
        let home = mk_home();
        let p = default_path(home.path());
        let bad = r#"{"version":1,"grants":[{"id":"x","scope":"fs/write:/tmp/**"}]}"#;
        fs::write(&p, bad).unwrap();
        let u = UserGrants::load(home.path());
        assert_eq!(u.grants.len(), 0, "expected empty on schema mismatch");
    }

    #[test]
    fn save_creates_parent_dir() {
        let tmp = TempDir::new().unwrap();
        // no config/ dir created
        let path = tmp.path().join("config").join("user_grants.json");
        let mut u = UserGrants::empty_at(path.clone());
        u.add(base_grant("g1", "fs/write:/tmp/**", None));
        u.save().unwrap();
        assert!(path.exists());
    }
}
