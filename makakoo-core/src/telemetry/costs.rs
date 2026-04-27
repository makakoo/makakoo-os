//! Cost tracker — per-agent / per-model token + USD ledger.
//!
//! Python source: `core/telemetry/cost_tracker.py`. The Python impl
//! writes to a JSONL file keyed on session id; the Rust port is
//! authoritative on sqlite (`costs` table in `db.rs`). The schema
//! normalises Python's ad-hoc `input` / `output` keys to
//! `prompt_tokens` / `completion_tokens` / `total_tokens` to match
//! the OpenAI-compatible shape used everywhere in switchAILocal.
//!
//! This module is deliberately small: record a row, summarise a
//! window, list recent rows. Pricing math lives with the caller (LLM
//! client, agent) so the telemetry layer stays agnostic of provider
//! price sheets — the USD field is whatever the caller computed.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CostRecord {
    /// Row id. Set to `0` when inserting — `CostTracker::record`
    /// returns the assigned id.
    #[serde(default)]
    pub id: i64,
    pub occurred_at: DateTime<Utc>,
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub usd: f64,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl CostRecord {
    /// Construct a record with `occurred_at = now` and auto-computed
    /// `total_tokens`. Convenient for agents that only know their own
    /// prompt/completion split.
    pub fn now(
        agent: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        prompt_tokens: u32,
        completion_tokens: u32,
        usd: f64,
    ) -> Self {
        Self {
            id: 0,
            occurred_at: Utc::now(),
            agent: agent.into(),
            provider: provider.into(),
            model: model.into(),
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
            usd,
            metadata: serde_json::Value::Object(Default::default()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CostSummary {
    pub total_usd: f64,
    pub total_tokens: u64,
    pub record_count: u64,
    pub by_agent: Vec<(String, f64)>,
    pub by_model: Vec<(String, f64)>,
    pub window: String,
}

pub struct CostTracker {
    conn: Arc<Mutex<Connection>>,
}

impl CostTracker {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Insert a record. Returns the assigned row id.
    pub fn record(&self, mut r: CostRecord) -> Result<i64> {
        if r.total_tokens == 0 {
            r.total_tokens = r.prompt_tokens.saturating_add(r.completion_tokens);
        }
        let metadata_json = serde_json::to_string(&r.metadata)?;
        let occurred_at = r.occurred_at.to_rfc3339();

        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO costs
                (occurred_at, agent, provider, model,
                 prompt_tokens, completion_tokens, total_tokens,
                 usd, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                occurred_at,
                r.agent,
                r.provider,
                r.model,
                r.prompt_tokens as i64,
                r.completion_tokens as i64,
                r.total_tokens as i64,
                r.usd,
                metadata_json,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Roll up costs for a window. Accepted windows:
    /// `"today"`, `"7d"`, `"30d"`, `"all"`.
    pub fn summary(&self, window: &str) -> Result<CostSummary> {
        let cutoff = window_cutoff(window)?;
        let conn = self.lock_conn()?;

        let (total_usd, total_tokens, count): (f64, i64, i64) = if let Some(since) = &cutoff {
            conn.query_row(
                "SELECT COALESCE(SUM(usd), 0.0),
                        COALESCE(SUM(total_tokens), 0),
                        COUNT(*)
                   FROM costs
                  WHERE occurred_at >= ?1",
                params![since],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?
        } else {
            conn.query_row(
                "SELECT COALESCE(SUM(usd), 0.0),
                        COALESCE(SUM(total_tokens), 0),
                        COUNT(*)
                   FROM costs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?
        };

        let by_agent = group_sum(&conn, "agent", cutoff.as_deref())?;
        let by_model = group_sum(&conn, "model", cutoff.as_deref())?;

        Ok(CostSummary {
            total_usd,
            total_tokens: total_tokens.max(0) as u64,
            record_count: count.max(0) as u64,
            by_agent,
            by_model,
            window: window.to_string(),
        })
    }

    /// Most recent `limit` records, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<CostRecord>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, occurred_at, agent, provider, model,
                    prompt_tokens, completion_tokens, total_tokens,
                    usd, metadata_json
               FROM costs
              ORDER BY occurred_at DESC, id DESC
              LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_record)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| MakakooError::internal("cost tracker mutex poisoned"))
    }
}

// ─── helpers ────────────────────────────────────────────────────────

fn window_cutoff(window: &str) -> Result<Option<String>> {
    let now = Utc::now();
    let cutoff = match window {
        "all" => None,
        "today" => {
            let start = now
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .map(|d| DateTime::<Utc>::from_naive_utc_and_offset(d, Utc))
                .unwrap_or(now);
            Some(start.to_rfc3339())
        }
        "7d" => Some((now - Duration::days(7)).to_rfc3339()),
        "30d" => Some((now - Duration::days(30)).to_rfc3339()),
        other => {
            return Err(MakakooError::internal(format!(
                "cost summary: unknown window '{other}' (want today|7d|30d|all)"
            )));
        }
    };
    Ok(cutoff)
}

fn group_sum(
    conn: &Connection,
    column: &'static str,
    cutoff: Option<&str>,
) -> Result<Vec<(String, f64)>> {
    let sql = if cutoff.is_some() {
        format!(
            "SELECT {column}, COALESCE(SUM(usd), 0.0) AS s
               FROM costs
              WHERE occurred_at >= ?1
              GROUP BY {column}
              ORDER BY s DESC"
        )
    } else {
        format!(
            "SELECT {column}, COALESCE(SUM(usd), 0.0) AS s
               FROM costs
              GROUP BY {column}
              ORDER BY s DESC"
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(since) = cutoff {
        stmt.query_map(params![since], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(String, f64)>>>()?
    } else {
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(String, f64)>>>()?
    };
    Ok(rows)
}

fn row_to_record(row: &Row<'_>) -> rusqlite::Result<CostRecord> {
    let occurred: String = row.get(1)?;
    let metadata_json: String = row.get(9)?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata_json).unwrap_or_else(|_| {
        serde_json::Value::Object(Default::default())
    });
    Ok(CostRecord {
        id: row.get(0)?,
        occurred_at: DateTime::parse_from_rfc3339(&occurred)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        agent: row.get(2)?,
        provider: row.get(3)?,
        model: row.get(4)?,
        prompt_tokens: row.get::<_, i64>(5)?.max(0) as u32,
        completion_tokens: row.get::<_, i64>(6)?.max(0) as u32,
        total_tokens: row.get::<_, i64>(7)?.max(0) as u32,
        usd: row.get(8)?,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};

    fn open_tracker() -> (tempfile::TempDir, CostTracker) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = open_db(&path).unwrap();
        run_migrations(&conn).unwrap();
        let shared = Arc::new(Mutex::new(conn));
        (dir, CostTracker::new(shared))
    }

    #[test]
    fn record_assigns_id_and_fills_total_tokens() {
        let (_d, t) = open_tracker();
        let r = CostRecord::now("harvey", "switchailocal", "ail-compound", 100, 50, 0.001);
        let id = t.record(r).unwrap();
        assert!(id > 0);
        let recent = t.recent(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].total_tokens, 150);
        assert_eq!(recent[0].agent, "harvey");
    }

    #[test]
    fn summary_all_sums_every_row() {
        let (_d, t) = open_tracker();
        t.record(CostRecord::now("harvey", "anthropic", "opus-4-6", 100, 50, 0.01))
            .unwrap();
        t.record(CostRecord::now("olibia", "switchailocal", "ail-compound", 200, 80, 0.002))
            .unwrap();
        t.record(CostRecord::now("arbitrage-agent", "openai", "o3", 500, 120, 0.03))
            .unwrap();
        let s = t.summary("all").unwrap();
        assert_eq!(s.record_count, 3);
        assert_eq!(s.total_tokens, 1050);
        assert!((s.total_usd - 0.042).abs() < 1e-6, "total_usd={}", s.total_usd);
        // by_agent in descending USD order.
        assert_eq!(s.by_agent.len(), 3);
        assert_eq!(s.by_agent[0].0, "arbitrage-agent");
    }

    #[test]
    fn summary_7d_filters_old_rows() {
        let (_d, t) = open_tracker();
        // Recent row.
        t.record(CostRecord::now("harvey", "prov", "model", 10, 10, 1.0))
            .unwrap();
        // Ancient row — insert with manual timestamp.
        let mut ancient = CostRecord::now("harvey", "prov", "model", 10, 10, 99.0);
        ancient.occurred_at = Utc::now() - Duration::days(60);
        t.record(ancient).unwrap();

        let s = t.summary("7d").unwrap();
        assert_eq!(s.record_count, 1);
        assert!((s.total_usd - 1.0).abs() < 1e-6);
    }

    #[test]
    fn summary_today_window_includes_now_row() {
        let (_d, t) = open_tracker();
        t.record(CostRecord::now("harvey", "p", "m", 1, 1, 0.5))
            .unwrap();
        let s = t.summary("today").unwrap();
        assert_eq!(s.record_count, 1);
        assert!((s.total_usd - 0.5).abs() < 1e-6);
    }

    #[test]
    fn summary_by_agent_groups_correctly() {
        let (_d, t) = open_tracker();
        t.record(CostRecord::now("harvey", "p", "m", 0, 0, 1.0))
            .unwrap();
        t.record(CostRecord::now("harvey", "p", "m", 0, 0, 2.0))
            .unwrap();
        t.record(CostRecord::now("olibia", "p", "m", 0, 0, 0.5))
            .unwrap();
        let s = t.summary("all").unwrap();
        let harvey = s.by_agent.iter().find(|(k, _)| k == "harvey").unwrap().1;
        let olibia = s.by_agent.iter().find(|(k, _)| k == "olibia").unwrap().1;
        assert!((harvey - 3.0).abs() < 1e-6);
        assert!((olibia - 0.5).abs() < 1e-6);
    }

    #[test]
    fn summary_rejects_unknown_window() {
        let (_d, t) = open_tracker();
        assert!(t.summary("tomorrow").is_err());
    }

    #[test]
    fn recent_returns_newest_first_and_respects_limit() {
        let (_d, t) = open_tracker();
        for i in 0..5 {
            t.record(CostRecord::now(
                format!("agent-{i}"),
                "p",
                "m",
                10,
                10,
                0.01,
            ))
            .unwrap();
        }
        let r = t.recent(3).unwrap();
        assert_eq!(r.len(), 3);
        // Newest first → agent-4, agent-3, agent-2.
        assert_eq!(r[0].agent, "agent-4");
        assert_eq!(r[2].agent, "agent-2");
    }
}
