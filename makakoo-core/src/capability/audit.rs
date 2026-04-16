//! Append-only audit log for capability calls.
//!
//! Spec: `spec/CAPABILITIES.md §3`. Every capability check — whether
//! `allowed`, `denied`, or `error` — writes one JSON line to
//! `$MAKAKOO_HOME/logs/audit.jsonl`. Rotation kicks in at 100 MB:
//! the current file is renamed to `audit.jsonl.<rfc3339>` and a fresh
//! `audit.jsonl` is started.
//!
//! **Concurrency:** writes go through a single `Mutex<BufWriter<File>>`
//! per `AuditLog`. JSONL is line-delimited so one entry per `write_all`
//! call is atomic as long as it fits inside the kernel's PIPE_BUF (on
//! macOS + Linux that's 512 bytes minimum; our entries are ~300 bytes
//! typical, bigger entries still serialize cleanly because we always
//! flush the full buffer before returning).
//!
//! **Phase E/1 scope:** synchronous write side only. The Unix socket
//! handler in Phase E/2 will be the first caller. The `makakoo audit`
//! read-side CLI lands later — until then, the file is directly
//! `jq`-able.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

const ROTATION_THRESHOLD_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum RotationError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("serialize error: {source}")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },
}

/// One audit entry. Fields match `spec/CAPABILITIES.md §3` schema 1:1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub ts: DateTime<Utc>,
    pub plugin: String,
    pub plugin_version: String,
    pub verb: String,
    /// The concrete request the plugin made (URL, path, key, etc.).
    pub scope_requested: String,
    /// Which grant scope in the manifest matched. `None` when denied.
    pub scope_granted: Option<String>,
    pub result: AuditResult,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub bytes_in: Option<u64>,
    #[serde(default)]
    pub bytes_out: Option<u64>,
    #[serde(default)]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditResult {
    Allowed,
    Denied,
    Error,
}

/// Handle to an open audit log. Wrap in an `Arc` if you want multiple
/// callers to share it.
pub struct AuditLog {
    path: PathBuf,
    writer: Mutex<BufWriter<File>>,
    rotation_threshold: u64,
}

impl AuditLog {
    /// Open (or create) the audit log under `$MAKAKOO_HOME/logs/audit.jsonl`.
    pub fn open_default(makakoo_home: &Path) -> Result<Self, RotationError> {
        let path = makakoo_home.join("logs").join("audit.jsonl");
        Self::open_at(&path)
    }

    /// Open an audit log at an explicit path. Mostly used in tests.
    pub fn open_at(path: &Path) -> Result<Self, RotationError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| RotationError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| RotationError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: Mutex::new(BufWriter::new(file)),
            rotation_threshold: ROTATION_THRESHOLD_BYTES,
        })
    }

    /// For tests that want to trigger rotation without writing 100 MB.
    #[doc(hidden)]
    pub fn with_rotation_threshold(mut self, bytes: u64) -> Self {
        self.rotation_threshold = bytes;
        self
    }

    /// Append one entry. Rotates the file first if it's over threshold.
    /// Flushes on return so `cat` / `jq` see the entry immediately.
    pub fn append(&self, entry: &AuditEntry) -> Result<(), RotationError> {
        self.rotate_if_needed()?;

        let line = serde_json::to_string(entry)
            .map_err(|source| RotationError::Serialize { source })?;
        let mut guard = self
            .writer
            .lock()
            .expect("audit writer mutex poisoned — previous panic during a write");
        guard
            .write_all(line.as_bytes())
            .map_err(|source| RotationError::Io {
                path: self.path.clone(),
                source,
            })?;
        guard.write_all(b"\n").map_err(|source| RotationError::Io {
            path: self.path.clone(),
            source,
        })?;
        guard.flush().map_err(|source| RotationError::Io {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
    }

    /// Check the file size and rename it if it's at or above threshold.
    /// Opens a fresh file in place.
    pub fn rotate_if_needed(&self) -> Result<(), RotationError> {
        let meta = match fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(RotationError::Io {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        if meta.len() < self.rotation_threshold {
            return Ok(());
        }
        self.force_rotate()
    }

    /// Unconditional rotation. Used when operators hit a size ceiling
    /// or by tests.
    pub fn force_rotate(&self) -> Result<(), RotationError> {
        let ts = Utc::now().format("%Y%m%dT%H%M%S%.3fZ").to_string();
        let archive = self.path.with_extension(format!("jsonl.{ts}"));
        debug!(
            "rotating audit log {} → {}",
            self.path.display(),
            archive.display()
        );

        // Flush the current writer so we don't lose buffered bytes.
        {
            let mut guard = self
                .writer
                .lock()
                .expect("audit writer mutex poisoned");
            let _ = guard.flush();
        }

        fs::rename(&self.path, &archive).map_err(|source| RotationError::Io {
            path: archive.clone(),
            source,
        })?;

        // Open a fresh file and swap.
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| RotationError::Io {
                path: self.path.clone(),
                source,
            })?;
        {
            let mut guard = self
                .writer
                .lock()
                .expect("audit writer mutex poisoned");
            *guard = BufWriter::new(file);
        }

        // Best-effort prune of archives older than 7 days.
        if let Err(e) = prune_old_archives(&self.path, 7) {
            warn!(
                "audit archive prune failed at {}: {e}",
                self.path.display()
            );
        }

        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Remove `audit.jsonl.<ts>` siblings older than `retention_days`. No-op
/// if none match.
fn prune_old_archives(live: &Path, retention_days: i64) -> std::io::Result<()> {
    let Some(dir) = live.parent() else {
        return Ok(());
    };
    let Some(prefix) = live.file_name().and_then(|s| s.to_str()) else {
        return Ok(());
    };
    let cutoff = Utc::now() - chrono::Duration::days(retention_days);
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with(&format!("{prefix}.")) {
            continue;
        }
        if name == prefix {
            continue;
        }
        let meta = entry.metadata()?;
        let mtime: DateTime<Utc> = meta
            .modified()
            .ok()
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| DateTime::<Utc>::from_timestamp(d.as_secs() as i64, 0).unwrap())
            })
            .unwrap_or_else(Utc::now);
        if mtime < cutoff {
            fs::remove_file(&p).ok();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(verb: &str, result: AuditResult) -> AuditEntry {
        AuditEntry {
            ts: Utc::now(),
            plugin: "test-plugin".into(),
            plugin_version: "1.0.0".into(),
            verb: verb.into(),
            scope_requested: "https://example.com/api".into(),
            scope_granted: Some("https://example.com/*".into()),
            result,
            duration_ms: Some(12),
            bytes_in: Some(100),
            bytes_out: Some(0),
            correlation_id: Some("abc123".into()),
        }
    }

    #[test]
    fn append_writes_one_line_per_entry() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let log = AuditLog::open_default(home).unwrap();
        log.append(&entry("brain/read", AuditResult::Allowed))
            .unwrap();
        log.append(&entry("net/http", AuditResult::Denied)).unwrap();

        let raw =
            std::fs::read_to_string(home.join("logs/audit.jsonl")).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: AuditEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.verb, "brain/read");
        assert_eq!(parsed.result, AuditResult::Allowed);
    }

    #[test]
    fn rotation_triggered_at_threshold() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let log = AuditLog::open_default(home)
            .unwrap()
            .with_rotation_threshold(256);

        // Write enough entries to cross 256 bytes. Typical entry is
        // ~300 bytes so one is already over.
        log.append(&entry("brain/read", AuditResult::Allowed))
            .unwrap();
        log.append(&entry("brain/read", AuditResult::Allowed))
            .unwrap();

        let dir = home.join("logs");
        let mut archives: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                n != "audit.jsonl" && n.starts_with("audit.jsonl.")
            })
            .collect();
        archives.sort();
        assert_eq!(archives.len(), 1, "expected one rotated archive");

        // Live file exists + is the fresh one (smaller than archive).
        assert!(home.join("logs/audit.jsonl").exists());
    }

    #[test]
    fn serde_roundtrip_covers_optional_fields() {
        let e = AuditEntry {
            ts: Utc::now(),
            plugin: "x".into(),
            plugin_version: "1.0.0".into(),
            verb: "brain/read".into(),
            scope_requested: "any".into(),
            scope_granted: None,
            result: AuditResult::Denied,
            duration_ms: None,
            bytes_in: None,
            bytes_out: None,
            correlation_id: None,
        };
        let s = serde_json::to_string(&e).unwrap();
        let back: AuditEntry = serde_json::from_str(&s).unwrap();
        assert!(back.scope_granted.is_none());
        assert_eq!(back.result, AuditResult::Denied);
        assert!(s.contains("\"result\":\"denied\""));
    }

    #[test]
    fn concurrent_appends_do_not_interleave() {
        use std::sync::Arc;
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();
        let log = Arc::new(AuditLog::open_default(&home).unwrap());

        let mut handles = vec![];
        for i in 0..10 {
            let log = Arc::clone(&log);
            handles.push(thread::spawn(move || {
                for _ in 0..20 {
                    log.append(&entry(
                        &format!("brain/read"),
                        if i % 2 == 0 {
                            AuditResult::Allowed
                        } else {
                            AuditResult::Denied
                        },
                    ))
                    .unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let raw =
            std::fs::read_to_string(home.join("logs/audit.jsonl")).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 200);
        // Every line is valid JSON — no torn writes.
        for l in lines {
            serde_json::from_str::<AuditEntry>(l).unwrap();
        }
    }
}
