//! v0.3.3 Phase B — idempotency key for `perms_purge_tick`.
//!
//! Prevents double-processing of expired grants when the SANCHO tick
//! fires twice in rapid succession (clock adjustment, daemon restart,
//! systemd timer drift). Each handler run reads the last-purge
//! timestamp, and if the cooldown window hasn't elapsed yet, returns
//! `Allowed::SkipCooldown` without touching the grant store.
//!
//! State file: `$MAKAKOO_HOME/state/perms_purge_last.json` — kept
//! separate from `perms_rate_limit.json` so lock contention on the
//! hot grant-create path (which touches the rate counter on every
//! call) doesn't block the background purge tick.
//!
//! Schema:
//!
//! ```json
//! {
//!   "last_purged_at": "2026-04-21T09:30:00Z"
//! }
//! ```
//!
//! Missing file or corrupt JSON → treated as "never purged" → allowed.
//!
//! Closes pi R2 in `spec/USER_GRANTS_THREAT_MODEL.md`.
//!
//! Python has no periodic purge caller (only tests use
//! `UserGrantsFile.purge_expired`), so this is Rust-only. The CLI
//! `makakoo perms purge` also consults the cooldown for symmetry —
//! manually retriggering the purge within 60s is almost always a
//! no-op anyway because the first pass already removed anything
//! expired.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Minimum gap between consecutive purge ticks. Matches the
/// "rapid succession" threshold called out in pi R2 — a daemon
/// restart or clock adjustment that re-fires within this window is
/// treated as a duplicate and skipped. Intentionally much shorter
/// than the SANCHO 900s tick cadence so legitimate back-to-back
/// scheduled ticks always run.
pub const PURGE_COOLDOWN_SECONDS: i64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PurgeState {
    last_purged_at: DateTime<Utc>,
}

/// Outcome of the cooldown check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeCheck {
    /// Caller may proceed — the state file has been updated with
    /// `now` as the new `last_purged_at`.
    Proceed,
    /// Caller must skip — the previous purge was within the cooldown
    /// window. `seconds_since_last` is `now - last_purged_at`.
    SkipCooldown { seconds_since_last: i64 },
}

pub fn default_path(home: &Path) -> PathBuf {
    home.join("state").join("perms_purge_last.json")
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

fn load(path: &Path) -> Option<PurgeState> {
    if !path.exists() {
        return None;
    }
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<PurgeState>(&bytes) {
            Ok(s) => Some(s),
            Err(e) => {
                warn!(
                    "corrupt perms_purge_last.json at {}: {}; treating as never-purged",
                    path.display(),
                    e
                );
                None
            }
        },
        Err(e) => {
            warn!(
                "could not read perms_purge_last.json at {}: {}; treating as never-purged",
                path.display(),
                e
            );
            None
        }
    }
}

fn save(path: &Path, state: &PurgeState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path_for(path);
    let serialized =
        serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
    {
        let mut f = File::create(&tmp)?;
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
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Under an exclusive sidecar lock:
///
/// 1. Load the last-purged ts (or None if file missing / corrupt).
/// 2. If `now - last_ts < PURGE_COOLDOWN_SECONDS`, return
///    `SkipCooldown` without touching the file.
/// 3. Otherwise save `now` as the new `last_purged_at` and return
///    `Proceed`.
///
/// Fresh calls (no prior state) always return `Proceed`. I/O errors
/// on the lock or state file fail open — the purge proceeds — because
/// a broken cooldown file must not silently disable hygiene.
pub fn check_and_record(home: &Path, now: DateTime<Utc>) -> PurgeCheck {
    let path = default_path(home);
    let lock_path = lock_path_for(&path);
    if let Some(parent) = lock_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            warn!(
                "perms_purge_last: mkdir {} failed: {}; proceeding fail-open",
                parent.display(),
                e
            );
            return PurgeCheck::Proceed;
        }
    }
    let lock_fd = match OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(e) => {
            warn!(
                "perms_purge_last: lock open {} failed: {}; proceeding fail-open",
                lock_path.display(),
                e
            );
            return PurgeCheck::Proceed;
        }
    };
    if let Err(e) = lock_fd.lock_exclusive() {
        warn!(
            "perms_purge_last: flock failed: {}; proceeding fail-open",
            e
        );
        return PurgeCheck::Proceed;
    }

    let result = if let Some(prev) = load(&path) {
        let delta = now - prev.last_purged_at;
        if delta < Duration::seconds(PURGE_COOLDOWN_SECONDS) {
            PurgeCheck::SkipCooldown {
                seconds_since_last: delta.num_seconds(),
            }
        } else {
            let _ = save(&path, &PurgeState { last_purged_at: now });
            PurgeCheck::Proceed
        }
    } else {
        let _ = save(&path, &PurgeState { last_purged_at: now });
        PurgeCheck::Proceed
    };

    let _ = FileExt::unlock(&lock_fd);
    result
}

/// Test-only: wipe the state file so subsequent checks start fresh.
pub fn reset_for_tests(home: &Path) {
    let _ = fs::remove_file(default_path(home));
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
    fn fresh_call_proceeds_and_records() {
        let h = home();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        assert_eq!(check_and_record(h.path(), now), PurgeCheck::Proceed);
        // State file now exists.
        assert!(default_path(h.path()).exists());
    }

    #[test]
    fn second_call_within_cooldown_skips() {
        let h = home();
        let t0 = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        check_and_record(h.path(), t0);
        let t1 = t0 + Duration::seconds(30);
        match check_and_record(h.path(), t1) {
            PurgeCheck::SkipCooldown { seconds_since_last } => {
                assert_eq!(seconds_since_last, 30);
            }
            other => panic!("expected SkipCooldown, got {other:?}"),
        }
    }

    #[test]
    fn call_after_cooldown_proceeds() {
        let h = home();
        let t0 = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        check_and_record(h.path(), t0);
        let t1 = t0 + Duration::seconds(61);
        assert_eq!(check_and_record(h.path(), t1), PurgeCheck::Proceed);
    }

    #[test]
    fn skip_does_not_update_state_file() {
        let h = home();
        let t0 = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        check_and_record(h.path(), t0);
        let t_mid = t0 + Duration::seconds(30);
        check_and_record(h.path(), t_mid); // skipped
        // After the skipped call, state should STILL carry t0, not t_mid.
        // So a call at t0 + 70s still sees the gap as >= cooldown from t0.
        let t_after = t0 + Duration::seconds(61);
        assert_eq!(
            check_and_record(h.path(), t_after),
            PurgeCheck::Proceed
        );
    }

    #[test]
    fn corrupt_state_fails_open() {
        let h = home();
        let path = default_path(h.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not valid json at all").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 9, 0, 0).unwrap();
        assert_eq!(check_and_record(h.path(), now), PurgeCheck::Proceed);
    }
}
