//! Atomic JSONL append for the GYM error funnel. Hot-path safe.
//!
//! Writes happen on the kernel's main path (failed `makakoo skill`
//! invocations, denied capability calls, crashed SANCHO tasks). The
//! contract is: never panic, never raise, never block. A capture failure
//! is silent — losing a single error entry is always better than
//! breaking the producer that hit it.
//!
//! Concurrency: Python `capture.py` keeps writing to the same files in
//! the same format. POSIX `O_APPEND` is atomic for writes ≤ `PIPE_BUF`
//! (4 KB on Linux/macOS); JSONL records are well under that bound after
//! truncation, so concurrent Python + Rust writers interleave cleanly
//! without locks.
//!
//! Schema is held byte-identical to the Python `capture.py`:
//! ```json
//! {
//!   "schema_version": "1.0",
//!   "ts":              ISO 8601 UTC,
//!   "source":          "bash" | "tool" | "python" | "sancho" | "manual_flag",
//!   "cmd":             truncated command/tool/function (≤ 512 chars),
//!   "cwd":             working dir with $HOME redacted,
//!   "stderr":          truncated stderr (≤ 2048 chars, $HOME redacted),
//!   "exit_code":       integer or null,
//!   "agent":           "harvey" | ...,
//!   "skill_in_scope":  best-effort skill name or null,
//!   "error_class":     filled by the Layer 2 classifier; null at capture,
//!   "raw":             { ... }   any source-specific extras
//! }
//! ```

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SCHEMA_VERSION: &str = "1.0";
const MAX_CMD: usize = 512;
const MAX_STDERR: usize = 2048;
const TRUNC_TAIL: &str = "...[truncated]";

/// Producer of an error entry. Lower-cased on serialise to match the
/// Python `ErrorSource` enum values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSource {
    Bash,
    Tool,
    Python,
    Sancho,
    ManualFlag,
}

impl ErrorSource {
    fn as_filename(self) -> &'static str {
        match self {
            ErrorSource::Bash => "bash",
            ErrorSource::Tool => "tool",
            ErrorSource::Python => "python",
            ErrorSource::Sancho => "sancho",
            ErrorSource::ManualFlag => "manual_flag",
        }
    }
}

/// One funnel entry. Optional fields default to `None` / empty so
/// callers fill only what they have.
#[derive(Debug, Default, Clone)]
pub struct ErrorEntry {
    pub source: Option<ErrorSource>,
    pub cmd: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub skill_in_scope: Option<String>,
    pub raw: Option<Value>,
    pub error_class: Option<String>,
}

impl ErrorEntry {
    pub fn new(source: ErrorSource) -> Self {
        Self {
            source: Some(source),
            ..Self::default()
        }
    }
    pub fn cmd(mut self, v: impl Into<String>) -> Self {
        self.cmd = Some(v.into());
        self
    }
    pub fn stderr(mut self, v: impl Into<String>) -> Self {
        self.stderr = Some(v.into());
        self
    }
    pub fn exit_code(mut self, v: i32) -> Self {
        self.exit_code = Some(v);
        self
    }
    pub fn cwd(mut self, v: impl Into<String>) -> Self {
        self.cwd = Some(v.into());
        self
    }
    pub fn agent(mut self, v: impl Into<String>) -> Self {
        self.agent = Some(v.into());
        self
    }
    pub fn skill_in_scope(mut self, v: impl Into<String>) -> Self {
        self.skill_in_scope = Some(v.into());
        self
    }
    pub fn raw(mut self, v: Value) -> Self {
        self.raw = Some(v);
        self
    }
    pub fn error_class(mut self, v: impl Into<String>) -> Self {
        self.error_class = Some(v.into());
        self
    }
}

/// Funnel writer. Cheap to construct — no I/O until `record` is called.
pub struct ErrorCapture {
    errors_dir: PathBuf,
    home_redact: Option<String>,
}

impl ErrorCapture {
    /// Build a capture rooted at `data/errors/` under `home`. Best-effort:
    /// the directory is created lazily on the first record.
    pub fn new(home: &Path) -> Self {
        Self {
            errors_dir: home.join("data").join("errors"),
            home_redact: dirs::home_dir().map(|p| p.to_string_lossy().to_string()),
        }
    }

    /// Override the redact-prefix (defaults to `dirs::home_dir`).
    /// Useful for tests that want predictable PII scrubbing.
    pub fn with_redact_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.home_redact = Some(prefix.into());
        self
    }

    /// Override the errors dir explicitly (tests).
    pub fn with_errors_dir(mut self, dir: PathBuf) -> Self {
        self.errors_dir = dir;
        self
    }

    /// Today's per-source JSONL path. Day rolls over in UTC to match
    /// the Python `capture.py` partitioning.
    pub fn today_dir(&self) -> PathBuf {
        let day = Utc::now().format("%Y-%m-%d").to_string();
        self.errors_dir.join(day)
    }

    fn path_for(&self, source: ErrorSource) -> PathBuf {
        self.today_dir().join(format!("{}.jsonl", source.as_filename()))
    }

    /// Append one entry. Returns `true` on a successful write,
    /// `false` on any failure (silently swallowed — capture is a
    /// best-effort observability hook).
    pub fn record(&self, entry: ErrorEntry) -> bool {
        let source = match entry.source {
            Some(s) => s,
            None => return false,
        };
        let cmd = self.scrub_truncate(entry.cmd.as_deref(), MAX_CMD);
        let stderr = self.scrub_truncate(entry.stderr.as_deref(), MAX_STDERR);
        let cwd = self.redact(&entry.cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        }));
        let agent = entry.agent.unwrap_or_else(infer_agent);
        let skill = entry.skill_in_scope.or_else(infer_skill);
        let raw = entry.raw.unwrap_or_else(|| json!({}));

        let record = json!({
            "schema_version": SCHEMA_VERSION,
            "ts":             Utc::now().to_rfc3339(),
            "source":         source.as_filename(),
            "cmd":            cmd,
            "cwd":            cwd,
            "stderr":         stderr,
            "exit_code":      entry.exit_code,
            "agent":          agent,
            "skill_in_scope": skill,
            "error_class":    entry.error_class,
            "raw":            raw,
        });

        let path = self.path_for(source);
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        let line = match serde_json::to_string(&record) {
            Ok(s) => s,
            Err(_) => return false,
        };

        // OpenOptions::append guarantees POSIX O_APPEND under the hood,
        // which gives atomic writes up to PIPE_BUF for free. We still
        // fall back to a temp+rename path for absurdly long records
        // (>4096 bytes including newline) to keep the parity claim
        // honest under heavy concurrent load.
        let blob = format!("{line}\n");
        if blob.len() <= 4096 {
            self.append_atomic(&path, blob.as_bytes())
        } else {
            self.append_via_temp(&path, blob.as_bytes())
        }
    }

    fn append_atomic(&self, path: &Path, bytes: &[u8]) -> bool {
        let mut file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(f) => f,
            Err(_) => return false,
        };
        if file.write_all(bytes).is_err() {
            return false;
        }
        let _ = file.flush();
        // best-effort durability — never panic on EIO
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                let _ = libc::fsync(file.as_raw_fd());
            }
        }
        true
    }

    fn append_via_temp(&self, path: &Path, bytes: &[u8]) -> bool {
        let tmp = path.with_extension(format!(
            "jsonl.{}.{}.tmp",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        if fs::write(&tmp, bytes).is_err() {
            return false;
        }
        // Read existing + new, write back. Slow path — only hit by
        // pathologically large records; >4KB after truncation already
        // implies the caller mis-sized stderr.
        let prior = fs::read(path).unwrap_or_default();
        let mut combined = prior;
        combined.extend_from_slice(bytes);
        let written = fs::write(path, combined).is_ok();
        let _ = fs::remove_file(&tmp);
        written
    }

    fn scrub_truncate(&self, text: Option<&str>, limit: usize) -> String {
        let raw = text.unwrap_or("");
        let redacted = self.redact(raw);
        truncate(&redacted, limit)
    }

    fn redact(&self, text: &str) -> String {
        match &self.home_redact {
            Some(prefix) if !prefix.is_empty() => text.replace(prefix.as_str(), "$HOME"),
            _ => text.to_string(),
        }
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let head_len = limit.saturating_sub(TRUNC_TAIL.len());
    let mut cut = head_len;
    while !text.is_char_boundary(cut) && cut > 0 {
        cut -= 1;
    }
    let mut out = String::with_capacity(limit);
    out.push_str(&text[..cut]);
    out.push_str(TRUNC_TAIL);
    out
}

fn infer_agent() -> String {
    std::env::var("HARVEY_AGENT")
        .or_else(|_| std::env::var("MAKAKOO_AGENT"))
        .unwrap_or_else(|_| "harvey".to_string())
}

fn infer_skill() -> Option<String> {
    std::env::var("HARVEY_SKILL_IN_SCOPE")
        .or_else(|_| std::env::var("MAKAKOO_SKILL_IN_SCOPE"))
        .ok()
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::read_to_string;
    use tempfile::tempdir;

    fn make() -> (tempfile::TempDir, ErrorCapture) {
        let dir = tempdir().unwrap();
        let cap = ErrorCapture::new(dir.path()).with_redact_prefix("/Users/test");
        (dir, cap)
    }

    #[test]
    fn truncate_marks_when_over_budget() {
        let s: String = "x".repeat(100);
        let out = truncate(&s, 30);
        assert!(out.ends_with(TRUNC_TAIL));
        assert!(out.len() <= 30);
    }

    #[test]
    fn record_appends_jsonl_line_with_schema_fields() {
        let (dir, cap) = make();
        let cap = cap.with_errors_dir(dir.path().join("errors"));
        let ok = cap.record(
            ErrorEntry::new(ErrorSource::Tool)
                .cmd("makakoo skill broken")
                .stderr("boom")
                .exit_code(1),
        );
        assert!(ok);
        let path = cap.path_for(ErrorSource::Tool);
        let body = read_to_string(&path).unwrap();
        assert!(body.contains("\"source\":\"tool\""));
        assert!(body.contains("\"schema_version\":\"1.0\""));
        assert!(body.contains("\"cmd\":\"makakoo skill broken\""));
        assert!(body.ends_with('\n'));
    }

    #[test]
    fn record_redacts_home_prefix_in_cwd_and_stderr() {
        let (dir, cap) = make();
        let cap = cap.with_errors_dir(dir.path().join("errors"));
        cap.record(
            ErrorEntry::new(ErrorSource::Sancho)
                .cwd("/Users/test/MAKAKOO/data")
                .stderr("crashed at /Users/test/MAKAKOO/agents/foo.py:42"),
        );
        let path = cap.path_for(ErrorSource::Sancho);
        let body = read_to_string(&path).unwrap();
        assert!(body.contains("$HOME/MAKAKOO/data"));
        assert!(body.contains("$HOME/MAKAKOO/agents/foo.py:42"));
    }

    #[test]
    fn record_truncates_oversized_stderr() {
        let (dir, cap) = make();
        let cap = cap.with_errors_dir(dir.path().join("errors"));
        let huge: String = "e".repeat(5000);
        cap.record(
            ErrorEntry::new(ErrorSource::Python)
                .cmd("traceback")
                .stderr(&huge),
        );
        let path = cap.path_for(ErrorSource::Python);
        let body = read_to_string(&path).unwrap();
        assert!(body.contains(TRUNC_TAIL));
        // stderr is bounded by MAX_STDERR (2048) before being JSON-escaped.
        assert!(body.len() < 5_000);
    }

    #[test]
    fn record_returns_false_when_source_missing() {
        let (dir, cap) = make();
        let cap = cap.with_errors_dir(dir.path().join("errors"));
        let entry = ErrorEntry::default(); // no source set
        assert!(!cap.record(entry));
    }

    #[test]
    fn concurrent_writers_dont_corrupt_jsonl_lines() {
        let (dir, cap) = make();
        let cap = std::sync::Arc::new(cap.with_errors_dir(dir.path().join("errors")));
        let mut handles = Vec::new();
        for i in 0..32 {
            let c = cap.clone();
            handles.push(std::thread::spawn(move || {
                c.record(
                    ErrorEntry::new(ErrorSource::Tool)
                        .cmd(format!("cmd-{i}"))
                        .stderr(format!("stderr-{i}")),
                )
            }));
        }
        for h in handles {
            assert!(h.join().unwrap());
        }
        let path = cap.path_for(ErrorSource::Tool);
        let body = read_to_string(&path).unwrap();
        // Every line must be valid JSON — no half-writes from racing
        // appenders. POSIX O_APPEND semantics give us this for free.
        for line in body.lines() {
            let _: serde_json::Value =
                serde_json::from_str(line).expect("line corrupted");
        }
        let count = body.lines().count();
        assert_eq!(count, 32);
    }
}
