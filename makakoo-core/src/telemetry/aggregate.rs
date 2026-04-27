//! v0.2 E.2 — Telemetry aggregation.
//!
//! Reads `audit.jsonl` and the cost tracker's per-record store, rolls
//! them up over standard windows (daily / weekly / monthly), returns
//! structured summaries the MCP `costs_summary` handler and the CLI
//! `makakoo metrics` command both consume.
//!
//! Design choice: aggregation is read-only and stateless — every call
//! re-scans the source files. For Sebastian's data volume (audit.jsonl
//! ≤ 100 MB by rotation policy, cost records ≤ a few thousand per day)
//! this is well under 100 ms. If volume ever justifies it, swap in
//! the materialized-rollup table without touching the call sites.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::capability::audit::{AuditLog, AuditResult, RotationError};

/// Time window for a rollup. The aggregator uses these to compute
/// `(since, until)` so callers don't have to hand-roll the date math.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Daily,
    Weekly,
    Monthly,
    /// Last N hours — for ad-hoc "what happened in the last 6h?" queries.
    Hours(u32),
}

impl Period {
    pub fn label(&self) -> String {
        match self {
            Period::Daily => "24h".into(),
            Period::Weekly => "7d".into(),
            Period::Monthly => "30d".into(),
            Period::Hours(h) => format!("{h}h"),
        }
    }

    /// Compute `(since, until)` anchored at `now`. `until` is always
    /// inclusive of `now`.
    pub fn window(&self, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
        let since = match self {
            Period::Daily => now - Duration::hours(24),
            Period::Weekly => now - Duration::days(7),
            Period::Monthly => now - Duration::days(30),
            Period::Hours(h) => now - Duration::hours(*h as i64),
        };
        (since, now)
    }
}

/// One row in a rollup. `key` is whatever bucket dimension the caller
/// asked for — plugin name, verb, or grant scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollupRow {
    pub key: String,
    pub allowed: u64,
    pub denied: u64,
    pub error: u64,
    pub total: u64,
}

impl RollupRow {
    fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            allowed: 0,
            denied: 0,
            error: 0,
            total: 0,
        }
    }

    fn bump(&mut self, result: AuditResult) {
        match result {
            AuditResult::Allowed => self.allowed += 1,
            AuditResult::Denied => self.denied += 1,
            AuditResult::Error => self.error += 1,
        }
        self.total += 1;
    }
}

/// Aggregate output for one period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRollup {
    pub period: String,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub by_plugin: Vec<RollupRow>,
    pub by_verb: Vec<RollupRow>,
    pub total_calls: u64,
    pub total_denied: u64,
    pub total_errors: u64,
}

/// Roll up the audit log over `period`. Reads live + rotated archives
/// via `AuditLog::query`. The result is sorted descending by `total`
/// inside each `by_*` field so the noisiest plugin/verb sits at the top.
pub fn audit_rollup(
    log: &AuditLog,
    period: Period,
    now: DateTime<Utc>,
) -> Result<AuditRollup, RotationError> {
    let (since, until) = period.window(now);
    let entries = log.query(since, until, None)?;

    let mut plugins: BTreeMap<String, RollupRow> = BTreeMap::new();
    let mut verbs: BTreeMap<String, RollupRow> = BTreeMap::new();

    let mut total_calls = 0u64;
    let mut total_denied = 0u64;
    let mut total_errors = 0u64;
    for entry in entries {
        let pkey = if entry.plugin.is_empty() {
            "<unknown>".to_string()
        } else {
            entry.plugin.clone()
        };
        plugins
            .entry(pkey)
            .or_insert_with(|| RollupRow::new(&entry.plugin))
            .bump(entry.result);
        verbs
            .entry(entry.verb.clone())
            .or_insert_with(|| RollupRow::new(&entry.verb))
            .bump(entry.result);
        total_calls += 1;
        match entry.result {
            AuditResult::Denied => total_denied += 1,
            AuditResult::Error => total_errors += 1,
            AuditResult::Allowed => {}
        }
    }

    let mut by_plugin: Vec<RollupRow> = plugins.into_values().collect();
    let mut by_verb: Vec<RollupRow> = verbs.into_values().collect();
    by_plugin.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.key.cmp(&b.key)));
    by_verb.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.key.cmp(&b.key)));

    Ok(AuditRollup {
        period: period.label(),
        since,
        until,
        by_plugin,
        by_verb,
        total_calls,
        total_denied,
        total_errors,
    })
}

/// Convenience: rollup using `$MAKAKOO_HOME/logs/audit.jsonl`.
pub fn rollup_default(
    home: &Path,
    period: Period,
) -> Result<AuditRollup, RotationError> {
    let log = AuditLog::open_default(home)?;
    audit_rollup(&log, period, Utc::now())
}

/// Truncate a UTC timestamp to the start of the same day. Useful for
/// callers that want to render "today's" rollup explicitly.
pub fn truncate_to_day(ts: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(ts.year(), ts.month(), ts.day(), 0, 0, 0)
        .single()
        .unwrap_or(ts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::audit::AuditEntry;
    use tempfile::TempDir;

    fn entry(plugin: &str, verb: &str, result: AuditResult, ts: DateTime<Utc>) -> AuditEntry {
        AuditEntry {
            ts,
            plugin: plugin.into(),
            plugin_version: "1.0.0".into(),
            verb: verb.into(),
            scope_requested: "any".into(),
            scope_granted: Some("*".into()),
            result,
            duration_ms: Some(5),
            bytes_in: None,
            bytes_out: None,
            correlation_id: None,
        }
    }

    #[test]
    fn period_window_anchors_at_now() {
        let now = Utc::now();
        let (s, u) = Period::Daily.window(now);
        assert_eq!(u, now);
        assert!((now - s).num_hours() == 24);
        let (s2, u2) = Period::Hours(6).window(now);
        assert_eq!(u2, now);
        assert!((now - s2).num_hours() == 6);
    }

    #[test]
    fn rollup_groups_by_plugin_and_verb_descending() {
        let tmp = TempDir::new().unwrap();
        let log = AuditLog::open_default(tmp.path()).unwrap();
        let now = Utc::now();

        // 5x agent-pi/exec, 2x agent-pi/brain-write, 1x agent-foo/brain-read
        for _ in 0..5 {
            log.append(&entry(
                "agent-pi", "exec/binary:pi", AuditResult::Allowed, now,
            ))
            .unwrap();
        }
        for _ in 0..2 {
            log.append(&entry(
                "agent-pi", "brain/write", AuditResult::Allowed, now,
            ))
            .unwrap();
        }
        log.append(&entry(
            "agent-foo", "brain/read", AuditResult::Denied, now,
        ))
        .unwrap();

        let r = audit_rollup(&log, Period::Daily, now).unwrap();
        assert_eq!(r.total_calls, 8);
        assert_eq!(r.total_denied, 1);
        // by_plugin: agent-pi (7) > agent-foo (1)
        assert_eq!(r.by_plugin[0].key, "agent-pi");
        assert_eq!(r.by_plugin[0].total, 7);
        assert_eq!(r.by_plugin[1].key, "agent-foo");
        assert_eq!(r.by_plugin[1].denied, 1);
        // by_verb: exec/binary:pi (5) > brain/write (2) > brain/read (1)
        assert_eq!(r.by_verb[0].key, "exec/binary:pi");
        assert_eq!(r.by_verb[0].total, 5);
    }

    #[test]
    fn rollup_filters_to_period_window() {
        let tmp = TempDir::new().unwrap();
        let log = AuditLog::open_default(tmp.path()).unwrap();
        let now = Utc::now();
        log.append(&entry(
            "old-plugin", "x", AuditResult::Allowed, now - Duration::days(8),
        ))
        .unwrap();
        log.append(&entry(
            "new-plugin", "y", AuditResult::Allowed, now,
        ))
        .unwrap();

        let r = audit_rollup(&log, Period::Daily, now).unwrap();
        assert_eq!(r.total_calls, 1);
        assert_eq!(r.by_plugin[0].key, "new-plugin");

        let weekly = audit_rollup(&log, Period::Weekly, now).unwrap();
        assert_eq!(weekly.total_calls, 1, "8d-old entry stays out of 7d window");

        let monthly = audit_rollup(&log, Period::Monthly, now).unwrap();
        assert_eq!(monthly.total_calls, 2);
    }

    #[test]
    fn rollup_handles_empty_log() {
        let tmp = TempDir::new().unwrap();
        let log = AuditLog::open_default(tmp.path()).unwrap();
        let r = audit_rollup(&log, Period::Daily, Utc::now()).unwrap();
        assert_eq!(r.total_calls, 0);
        assert!(r.by_plugin.is_empty());
        assert!(r.by_verb.is_empty());
    }

    #[test]
    fn period_labels_are_stable() {
        assert_eq!(Period::Daily.label(), "24h");
        assert_eq!(Period::Weekly.label(), "7d");
        assert_eq!(Period::Monthly.label(), "30d");
        assert_eq!(Period::Hours(6).label(), "6h");
    }
}
