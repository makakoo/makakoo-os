//! Phase 12 — JSONL audit log.
//!
//! Locked Q14:
//!
//! * `~/MAKAKOO/data/audit/agents.jsonl` — JSONL, one event per line
//! * Rotated at 100 MB, total cap 1 GB
//! * On cap reached: write to `agents.alerts.log` (separate, capped
//!   at 10 MB), continue blocking newest writes briefly while
//!   evicting oldest rotated file
//! * File mode 0600 enforced on creation + after rotation
//! * Schema: `{ts, slot_id, transport_id, kind, actor, target,
//!   outcome, detail}`
//! * Redaction: secret values, OAuth tokens, message bodies NEVER
//!   logged. Actor / target may carry user identifiers (email,
//!   phone, Slack U…) — these are documented as sensitive but
//!   not redacted (forensic requirement).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Locked rotation thresholds.
pub const ROTATE_AT_BYTES: u64 = 100 * 1024 * 1024;
pub const TOTAL_CAP_BYTES: u64 = 1024 * 1024 * 1024;
pub const ALERTS_CAP_BYTES: u64 = 10 * 1024 * 1024;

/// Locked enum of audit event kinds. New variants require a
/// coordinated SPRINT.md update.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    /// Tool whitelist violation.
    ScopeTool,
    /// Path scope violation.
    ScopePath,
    /// Secret resolution attempt (success | not_found | denied).
    SecretResolve,
    /// UserGrant lifecycle.
    GrantIssue,
    GrantRevoke,
    /// Slot lifecycle.
    SlotCreate,
    SlotStart,
    SlotStop,
    SlotDestroy,
    /// Transport credential verify result.
    TransportVerify,
    /// Per-sender rate-limit hit.
    RateLimit,
    /// Fault-injection scenario triggered.
    FaultTest,
    /// Gateway child crash.
    GatewayCrash,
    /// HMAC verification failure on a webhook.
    WebhookInvalidSignature,
    /// Origin allowlist failure on a WS upgrade.
    WebhookBadOrigin,
    /// Cookie verification failure on a WS upgrade.
    WebhookBadCookie,
    /// Generic webhook 4xx.
    WebhookBadRequest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    NotFound,
    Denied,
    Failure,
    Pending,
}

/// One audit event line. JSONL representation matches this struct
/// 1:1 (no internal-use-only fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub ts: DateTime<Utc>,
    pub slot_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_id: Option<String>,
    pub kind: AuditKind,
    /// Subject of the event — user-identifying value (email, phone,
    /// Slack U…). Documented as sensitive but never redacted; audit
    /// forensics require it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Object of the event — tool name, path, secret ref name, etc.
    /// Path components are full paths; secret-ref names are NOT
    /// values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub outcome: AuditOutcome,
    /// Free-form structured detail. The writer enforces redaction
    /// of dangerous keys (`secret_value`, `password`, `token`, `body`,
    /// `text`) at serialization time.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub detail: serde_json::Value,
}

impl AuditEvent {
    pub fn new(
        slot_id: impl Into<String>,
        kind: AuditKind,
        outcome: AuditOutcome,
    ) -> Self {
        Self {
            ts: Utc::now(),
            slot_id: slot_id.into(),
            transport_id: None,
            kind,
            actor: None,
            target: None,
            outcome,
            detail: serde_json::Value::Null,
        }
    }

    pub fn with_transport(mut self, transport_id: impl Into<String>) -> Self {
        self.transport_id = Some(transport_id.into());
        self
    }
    pub fn with_actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = redact(detail);
        self
    }

    pub fn to_jsonl(&self) -> String {
        let mut s = serde_json::to_string(self)
            .expect("AuditEvent serializes — all fields are JSON-safe");
        s.push('\n');
        s
    }
}

/// Locked redaction set. Any nested key matching one of these (case-
/// insensitive substring) is replaced with `"<redacted>"`.
const REDACT_KEYS: &[&str] = &[
    "secret_value",
    "password",
    "token",
    "bot_token",
    "api_key",
    "signing_secret",
    "client_secret",
    "body",
    "text",
];

/// Walk a `serde_json::Value` and redact any dangerous keys. Public
/// so call sites can pre-redact before constructing an event detail.
pub fn redact(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                let k_lower = k.to_lowercase();
                let is_secret = REDACT_KEYS.iter().any(|s| k_lower.contains(s));
                if is_secret {
                    out.insert(k, serde_json::Value::String("<redacted>".into()));
                } else {
                    out.insert(k, redact(val));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(redact).collect())
        }
        other => other,
    }
}

// ── Disk layout ──────────────────────────────────────────────────

pub fn audit_dir(makakoo_home: &Path) -> PathBuf {
    makakoo_home.join("data/audit")
}

pub fn audit_log_path(makakoo_home: &Path) -> PathBuf {
    audit_dir(makakoo_home).join("agents.jsonl")
}

pub fn alerts_log_path(makakoo_home: &Path) -> PathBuf {
    audit_dir(makakoo_home).join("agents.alerts.log")
}

/// Open the audit log for append. Creates the directory + file with
/// mode 0600 on first use.
pub fn open_for_append(makakoo_home: &Path) -> std::io::Result<std::fs::File> {
    let path = audit_log_path(makakoo_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts.open(&path)?;
    enforce_file_mode_0600(&path)?;
    Ok(f)
}

#[cfg(unix)]
pub fn enforce_file_mode_0600(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path)?.permissions();
    perm.set_mode(0o600);
    std::fs::set_permissions(path, perm)
}

#[cfg(not(unix))]
pub fn enforce_file_mode_0600(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Total bytes consumed by audit logs (jsonl + rotated). Used by
/// the cap-check on each write.
pub fn total_audit_bytes(makakoo_home: &Path) -> std::io::Result<u64> {
    let dir = audit_dir(makakoo_home);
    if !dir.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Audit log files: agents.jsonl + rotated agents.<unix_ts>.jsonl
        if name_str.starts_with("agents") && name_str.ends_with(".jsonl") {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

/// Append one event to the audit log. Rotates at ROTATE_AT_BYTES,
/// emits an alert + evicts oldest rotated file if total exceeds
/// TOTAL_CAP_BYTES.
pub fn append_event(makakoo_home: &Path, event: &AuditEvent) -> std::io::Result<()> {
    let path = audit_log_path(makakoo_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Rotate?
    if path.exists() {
        let size = std::fs::metadata(&path)?.len();
        if size >= ROTATE_AT_BYTES {
            rotate(makakoo_home)?;
        }
    }
    // Cap check: if we'd push total over the cap, evict oldest
    // rotated file first. Up to TOTAL_CAP_BYTES / ROTATE_AT_BYTES
    // rotated files.
    let total = total_audit_bytes(makakoo_home).unwrap_or(0);
    if total >= TOTAL_CAP_BYTES {
        emit_alert(makakoo_home, "audit total cap reached; evicting oldest")?;
        evict_oldest_rotated(makakoo_home)?;
    }
    use std::io::Write;
    let mut f = open_for_append(makakoo_home)?;
    f.write_all(event.to_jsonl().as_bytes())?;
    Ok(())
}

fn rotate(makakoo_home: &Path) -> std::io::Result<()> {
    let from = audit_log_path(makakoo_home);
    if !from.exists() {
        return Ok(());
    }
    let unix_ts = Utc::now().timestamp();
    let to = audit_dir(makakoo_home).join(format!("agents.{unix_ts}.jsonl"));
    std::fs::rename(&from, &to)?;
    enforce_file_mode_0600(&to)?;
    Ok(())
}

fn evict_oldest_rotated(makakoo_home: &Path) -> std::io::Result<()> {
    let dir = audit_dir(makakoo_home);
    let mut rotated: VecDeque<(u64, PathBuf)> = VecDeque::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();
        if name_str.starts_with("agents.") && name_str.ends_with(".jsonl") && name_str != "agents.jsonl" {
            // Extract unix_ts from name.
            if let Some(stem) = name_str.strip_prefix("agents.") {
                if let Some(ts) = stem.strip_suffix(".jsonl") {
                    if let Ok(ts_u) = ts.parse::<u64>() {
                        rotated.push_back((ts_u, entry.path()));
                    }
                }
            }
        }
    }
    if rotated.is_empty() {
        return Ok(());
    }
    // Sort ascending — oldest first.
    let mut sorted: Vec<_> = rotated.into_iter().collect();
    sorted.sort_by_key(|(ts, _)| *ts);
    if let Some((_, path)) = sorted.first() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn emit_alert(makakoo_home: &Path, msg: &str) -> std::io::Result<()> {
    use std::io::Write;
    let path = alerts_log_path(makakoo_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Cap alerts log too.
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() >= ALERTS_CAP_BYTES {
            let _ = std::fs::write(&path, "");
        }
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&path)?;
    let now = Utc::now();
    writeln!(f, "{} {}", now.to_rfc3339(), msg)?;
    eprintln!("[makakoo-audit] {msg}");
    Ok(())
}

/// Read the last N events from the audit log. Used by `agent audit`.
pub fn tail_events(
    makakoo_home: &Path,
    last: usize,
    kind_filter: Option<AuditKind>,
) -> std::io::Result<Vec<AuditEvent>> {
    let path = audit_log_path(makakoo_home);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let body = std::fs::read_to_string(&path)?;
    let mut events: Vec<AuditEvent> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if let Some(k) = kind_filter {
        events.retain(|e| e.kind == k);
    }
    let len = events.len();
    if last < len {
        events.drain(0..(len - last));
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn audit_event_jsonl_roundtrips() {
        let e = AuditEvent::new("secretary", AuditKind::ScopeTool, AuditOutcome::Denied)
            .with_actor("U0123ABCD")
            .with_target("run_command")
            .with_transport("slack-main");
        let line = e.to_jsonl();
        assert!(line.ends_with('\n'));
        let parsed: AuditEvent = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(parsed.slot_id, "secretary");
        assert_eq!(parsed.kind, AuditKind::ScopeTool);
        assert_eq!(parsed.actor.as_deref(), Some("U0123ABCD"));
        assert_eq!(parsed.target.as_deref(), Some("run_command"));
        assert_eq!(parsed.transport_id.as_deref(), Some("slack-main"));
    }

    #[test]
    fn redact_replaces_secret_keys() {
        let v = serde_json::json!({
            "secret_value": "abc123",
            "password": "p",
            "token": "t",
            "user_id": "U001",
        });
        let r = redact(v);
        assert_eq!(r["secret_value"], serde_json::Value::String("<redacted>".into()));
        assert_eq!(r["password"], serde_json::Value::String("<redacted>".into()));
        assert_eq!(r["token"], serde_json::Value::String("<redacted>".into()));
        assert_eq!(r["user_id"], serde_json::Value::String("U001".into()));
    }

    #[test]
    fn redact_walks_nested_objects() {
        let v = serde_json::json!({
            "outer": {"bot_token": "xoxb-secret", "name": "ok"}
        });
        let r = redact(v);
        assert_eq!(r["outer"]["bot_token"], serde_json::Value::String("<redacted>".into()));
        assert_eq!(r["outer"]["name"], serde_json::Value::String("ok".into()));
    }

    #[test]
    fn redact_walks_arrays_of_objects() {
        let v = serde_json::json!([{"api_key": "k"}, {"x": 1}]);
        let r = redact(v);
        assert_eq!(r[0]["api_key"], serde_json::Value::String("<redacted>".into()));
        assert_eq!(r[1]["x"], serde_json::Value::Number(1.into()));
    }

    #[test]
    fn append_creates_dir_and_file() {
        let tmp = TempDir::new().unwrap();
        let event = AuditEvent::new("secretary", AuditKind::SlotStart, AuditOutcome::Success);
        append_event(tmp.path(), &event).unwrap();
        let path = audit_log_path(tmp.path());
        assert!(path.exists());
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"slot_id\":\"secretary\""));
        assert!(body.contains("\"kind\":\"slot_start\""));
    }

    #[test]
    fn append_appends_not_overwrites() {
        let tmp = TempDir::new().unwrap();
        for kind in [AuditKind::SlotStart, AuditKind::SlotStop, AuditKind::SlotDestroy] {
            let e = AuditEvent::new("x", kind, AuditOutcome::Success);
            append_event(tmp.path(), &e).unwrap();
        }
        let body = std::fs::read_to_string(audit_log_path(tmp.path())).unwrap();
        assert_eq!(body.lines().count(), 3);
    }

    #[test]
    fn tail_events_returns_last_n() {
        let tmp = TempDir::new().unwrap();
        for _ in 0..10 {
            let e = AuditEvent::new("x", AuditKind::ScopeTool, AuditOutcome::Denied);
            append_event(tmp.path(), &e).unwrap();
        }
        let events = tail_events(tmp.path(), 3, None).unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn tail_events_filters_by_kind() {
        let tmp = TempDir::new().unwrap();
        for k in [
            AuditKind::ScopeTool,
            AuditKind::SlotStart,
            AuditKind::ScopeTool,
        ] {
            append_event(
                tmp.path(),
                &AuditEvent::new("x", k, AuditOutcome::Success),
            )
            .unwrap();
        }
        let events = tail_events(tmp.path(), 100, Some(AuditKind::ScopeTool)).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn with_detail_redacts_at_construction() {
        let e = AuditEvent::new("x", AuditKind::ScopeTool, AuditOutcome::Denied)
            .with_detail(serde_json::json!({"bot_token": "xoxb-secret"}));
        let body = e.to_jsonl();
        assert!(body.contains("<redacted>"));
        assert!(!body.contains("xoxb-secret"));
    }

    #[cfg(unix)]
    #[test]
    fn audit_log_file_mode_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let e = AuditEvent::new("x", AuditKind::SlotStart, AuditOutcome::Success);
        append_event(tmp.path(), &e).unwrap();
        let perm = std::fs::metadata(audit_log_path(tmp.path()))
            .unwrap()
            .permissions();
        // mode() includes the file-type bits in high bits; mask to
        // permission bits.
        assert_eq!(perm.mode() & 0o777, 0o600);
    }
}
