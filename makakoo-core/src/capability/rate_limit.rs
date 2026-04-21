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
}
