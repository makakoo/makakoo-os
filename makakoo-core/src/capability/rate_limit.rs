//! Global grant rate-limit counter (see `spec/USER_GRANTS.md §7`).
//!
//! Mirrors the Python helper at
//! `plugins-core/lib-harvey-core/src/core/capability/rate_limit.py`.
//! Both implementations read/write the same
//! `$MAKAKOO_HOME/state/perms_rate_limit.json` under the same sidecar-
//! lock protocol as the grant store. Schema is deliberately minimal
//! so a corrupt counter (lope F7) can't poison the grant store.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

pub const MAX_ACTIVE_GRANTS: usize = 20;
pub const MAX_CREATES_PER_HOUR: usize = 50;
pub const WINDOW_SECONDS: i64 = 60 * 60;

#[derive(Debug, Error)]
pub enum RateLimitError {
    #[error("{0}")]
    Exceeded(String),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("serialize: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WindowState {
    window_start: DateTime<Utc>,
    creates_in_window: u32,
}

pub fn default_path(home: &Path) -> PathBuf {
    home.join("state").join("perms_rate_limit.json")
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

fn load(path: &Path, now: DateTime<Utc>) -> WindowState {
    if !path.exists() {
        return WindowState {
            window_start: now,
            creates_in_window: 0,
        };
    }
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<WindowState>(&bytes) {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "corrupt perms_rate_limit.json at {}: {}; resetting",
                    path.display(),
                    e
                );
                WindowState {
                    window_start: now,
                    creates_in_window: 0,
                }
            }
        },
        Err(e) => {
            warn!(
                "could not read perms_rate_limit.json at {}: {}; resetting",
                path.display(),
                e
            );
            WindowState {
                window_start: now,
                creates_in_window: 0,
            }
        }
    }
}

fn save(path: &Path, state: &WindowState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = tmp_path_for(path);
    let serialized = serde_json::to_vec_pretty(state)?;
    {
        let mut f = File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(&serialized)?;
        f.sync_all().ok();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&tmp) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = fs::set_permissions(&tmp, perms);
        }
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Raise `RateLimitError::Exceeded` if creating a new grant would
/// breach either limit. Otherwise increment the in-window counter.
///
/// `active_grant_count` is supplied by the caller (from
/// `UserGrants::active_grants`) so this helper doesn't need to
/// re-open the grant store.
pub fn check_and_increment(
    active_grant_count: usize,
    home: &Path,
    now: DateTime<Utc>,
) -> Result<(), RateLimitError> {
    if active_grant_count >= MAX_ACTIVE_GRANTS {
        return Err(RateLimitError::Exceeded(format!(
            "rate limit: {} active grants (max {}); revoke some or wait",
            active_grant_count, MAX_ACTIVE_GRANTS
        )));
    }

    let path = default_path(home);
    let lock_path = lock_path_for(&path);
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).map_err(|source| RateLimitError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let lock_fd = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .map_err(|source| RateLimitError::Io {
            path: lock_path.clone(),
            source,
        })?;
    lock_fd
        .lock_exclusive()
        .map_err(|source| RateLimitError::Io {
            path: lock_path.clone(),
            source,
        })?;

    let result: Result<(), RateLimitError> = (|| {
        let mut state = load(&path, now);
        if now - state.window_start >= Duration::seconds(WINDOW_SECONDS) {
            state = WindowState {
                window_start: now,
                creates_in_window: 0,
            };
        }
        if state.creates_in_window as usize >= MAX_CREATES_PER_HOUR {
            return Err(RateLimitError::Exceeded(format!(
                "rate limit: {} grants created in the last hour (max {}); wait a bit",
                state.creates_in_window, MAX_CREATES_PER_HOUR
            )));
        }
        state.creates_in_window += 1;
        save(&path, &state).map_err(|e| RateLimitError::Exceeded(e.to_string()))
    })();

    let _ = FileExt::unlock(&lock_fd);
    result
}

/// Release one slot from the per-hour create bucket. Called on
/// revoke — NEVER on purge (spec/USER_GRANTS.md v1.1 §7; purge as a
/// decrement path would let slow-drip grants defeat the cap).
///
/// All race / rollover conditions are no-ops — this function never
/// returns `Err` under normal operation:
///
/// * counter file missing → nothing to decrement
/// * counter already at 0 → floor, no change
/// * window expired (`now - window_start >= WINDOW_SECONDS`) → next
///   `check_and_increment` will reset; decrementing now is moot
pub fn decrement(
    home: &Path,
    now: DateTime<Utc>,
) -> Result<(), RateLimitError> {
    let path = default_path(home);
    if !path.exists() {
        return Ok(());
    }

    let lock_path = lock_path_for(&path);
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).map_err(|source| RateLimitError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let lock_fd = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .map_err(|source| RateLimitError::Io {
            path: lock_path.clone(),
            source,
        })?;
    lock_fd
        .lock_exclusive()
        .map_err(|source| RateLimitError::Io {
            path: lock_path.clone(),
            source,
        })?;

    let result: Result<(), RateLimitError> = (|| {
        let mut state = load(&path, now);
        if now - state.window_start >= Duration::seconds(WINDOW_SECONDS) {
            return Ok(());
        }
        if state.creates_in_window == 0 {
            return Ok(());
        }
        state.creates_in_window -= 1;
        save(&path, &state).map_err(|e| RateLimitError::Exceeded(e.to_string()))
    })();

    let _ = FileExt::unlock(&lock_fd);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn home() -> TempDir {
        let t = TempDir::new().unwrap();
        fs::create_dir_all(t.path().join("state")).unwrap();
        t
    }

    #[test]
    fn fresh_window_accepts_up_to_max() {
        let h = home();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        for _ in 0..MAX_CREATES_PER_HOUR {
            check_and_increment(0, h.path(), now).unwrap();
        }
    }

    #[test]
    fn overflow_within_window_fails() {
        let h = home();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        for _ in 0..MAX_CREATES_PER_HOUR {
            check_and_increment(0, h.path(), now).unwrap();
        }
        let e = check_and_increment(0, h.path(), now).unwrap_err();
        assert!(matches!(e, RateLimitError::Exceeded(_)));
    }

    #[test]
    fn window_rolls_after_an_hour() {
        let h = home();
        let t0 = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        for _ in 0..MAX_CREATES_PER_HOUR {
            check_and_increment(0, h.path(), t0).unwrap();
        }
        let t1 = t0 + Duration::minutes(61);
        // Now should succeed after roll.
        check_and_increment(0, h.path(), t1).unwrap();
    }

    #[test]
    fn active_cap_fires_independent_of_window() {
        let h = home();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        let e = check_and_increment(MAX_ACTIVE_GRANTS, h.path(), now)
            .unwrap_err();
        assert!(matches!(e, RateLimitError::Exceeded(_)));
    }

    #[test]
    fn corrupt_counter_resets_gracefully() {
        let h = home();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        fs::write(default_path(h.path()), b"not json at all").unwrap();
        check_and_increment(0, h.path(), now).unwrap();
    }

    #[test]
    fn decrement_on_empty_counter_is_noop() {
        let h = home();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        // Counter file missing — decrement is a silent no-op.
        decrement(h.path(), now).unwrap();
        assert!(!default_path(h.path()).exists());
        // Now prime the file at 0 via a roll-over path, then decrement
        // again — still zero, still no error.
        check_and_increment(0, h.path(), now).unwrap();
        decrement(h.path(), now).unwrap();
        decrement(h.path(), now).unwrap();
        let state: WindowState = serde_json::from_slice(
            &fs::read(default_path(h.path())).unwrap(),
        )
        .unwrap();
        assert_eq!(state.creates_in_window, 0);
    }

    #[test]
    fn decrement_after_window_expired_is_noop() {
        let h = home();
        let t0 = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        // Prime the file with counter=1 at t0.
        check_and_increment(0, h.path(), t0).unwrap();
        // Decrement an hour later — window has rolled off; we leave
        // the stale state intact because the next increment will reset
        // it anyway.
        let t1 = t0 + Duration::minutes(61);
        decrement(h.path(), t1).unwrap();
        let state: WindowState = serde_json::from_slice(
            &fs::read(default_path(h.path())).unwrap(),
        )
        .unwrap();
        assert_eq!(
            state.creates_in_window, 1,
            "decrement past window should not touch stale counter"
        );
    }

    #[test]
    fn decrement_inside_window_reduces_count() {
        let h = home();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        for _ in 0..5 {
            check_and_increment(0, h.path(), now).unwrap();
        }
        decrement(h.path(), now).unwrap();
        let state: WindowState = serde_json::from_slice(
            &fs::read(default_path(h.path())).unwrap(),
        )
        .unwrap();
        assert_eq!(state.creates_in_window, 4);
    }

    #[test]
    fn increment_then_decrement_roundtrip() {
        let h = home();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        // 50 grant/revoke cycles should leave the counter exactly at
        // zero and still allow a 51st increment (mirrors the Phase A
        // dogfood scenario).
        for _ in 0..50 {
            check_and_increment(0, h.path(), now).unwrap();
            decrement(h.path(), now).unwrap();
        }
        let state: WindowState = serde_json::from_slice(
            &fs::read(default_path(h.path())).unwrap(),
        )
        .unwrap();
        assert_eq!(state.creates_in_window, 0);
        // 51st increment still succeeds — not rate-limited.
        check_and_increment(0, h.path(), now).unwrap();
    }

    #[test]
    fn shared_fixture_vectors_match_python() {
        // Drift-gate test — loads the same JSON fixture Python loads
        // and replays each sequence. If either side drifts (different
        // semantics, field rename), both suites fail in lockstep.
        let fixture_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../plugins-core/lib-harvey-core/tests/fixtures/rate_limit_decrement_vectors.json");
        let bytes = fs::read(&fixture_path).unwrap_or_else(|e| {
            panic!("cannot read {}: {}", fixture_path.display(), e)
        });
        let fixture: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        let t0 = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        for seq in fixture["sequences"].as_array().unwrap() {
            let h = home();
            let name = seq["name"].as_str().unwrap().to_string();

            if let Some(ops) = seq["ops"].as_array() {
                for op in ops {
                    let offset = op["offset_s"].as_i64().unwrap_or(0);
                    let now = t0 + Duration::seconds(offset);
                    match op["op"].as_str().unwrap() {
                        "increment" => {
                            check_and_increment(0, h.path(), now).unwrap()
                        }
                        "decrement" => decrement(h.path(), now).unwrap(),
                        other => panic!("unknown op in {}: {}", name, other),
                    }
                    if let Some(expected) = op["expected_count"].as_u64() {
                        let actual = if default_path(h.path()).exists() {
                            let state: WindowState = serde_json::from_slice(
                                &fs::read(default_path(h.path())).unwrap(),
                            )
                            .unwrap();
                            state.creates_in_window as u64
                        } else {
                            0
                        };
                        assert_eq!(
                            actual, expected,
                            "{}: expected count {}, got {}",
                            name, expected, actual
                        );
                    }
                }
            } else if seq.get("ops_template").is_some() {
                // `fifty_cycle_no_lockout` — grant+revoke 50 times,
                // then assert counter is at 0 and a fresh increment
                // still succeeds.
                for _ in 0..50 {
                    check_and_increment(0, h.path(), t0).unwrap();
                    decrement(h.path(), t0).unwrap();
                }
                let state: WindowState = serde_json::from_slice(
                    &fs::read(default_path(h.path())).unwrap(),
                )
                .unwrap();
                assert_eq!(state.creates_in_window, 0, "{}", name);
                if seq["then_increment_succeeds"].as_bool().unwrap_or(false) {
                    check_and_increment(0, h.path(), t0).unwrap();
                }
            }
        }
    }
}
