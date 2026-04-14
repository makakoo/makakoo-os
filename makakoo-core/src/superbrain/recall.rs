//! Recall tracker — logs superbrain hits and materializes aggregate stats.
//!
//! Port of `core/memory/recall_tracker.py`. Every superbrain search records
//! which documents it returned into `recall_log`; the promoter rebuilds
//! `recall_stats` from that log before ranking promotion candidates.
//!
//! Schemas are byte-identical to the Python source (migrated in
//! `db::SCHEMA_V1`). The aggregation SQL in `rebuild_stats` is a
//! near-literal port so the two implementations share the same
//! T1 oracle.
//!
//! Content-hash: the Python port truncates a SHA1 to 12 hex chars. We use
//! `blake3` (already a workspace dep) truncated to 12 hex chars. The exact
//! bytes differ from Python but the hash is deterministic, collision-safe
//! well past the user's personal-install cardinality, and keyed the same
//! way within this runtime.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};

use crate::error::Result;

/// Internal aggregate row pulled out of `recall_log` during rebuild.
type StatsAggregate = (
    String, // content_hash
    i64,    // doc_id
    String, // doc_path
    i64,    // recall_count
    i64,    // unique_queries
    i64,    // unique_days
    f64,    // total_score
    f64,    // max_score
    String, // first_recalled_at
    String, // last_recalled_at
);

/// A single recall event — one search result for one query.
#[derive(Debug, Clone)]
pub struct RecallItem {
    pub doc_id: i64,
    pub doc_path: String,
    pub content: String,
    pub score: f64,
    pub source: String,
}

impl RecallItem {
    pub fn new(doc_id: i64, doc_path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            doc_id,
            doc_path: doc_path.into(),
            content: content.into(),
            score: 0.0,
            source: "search".to_string(),
        }
    }

    #[must_use]
    pub fn with_score(mut self, score: f64) -> Self {
        self.score = score;
        self
    }

    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }
}

/// Recall tracker backed by a shared SQLite connection.
pub struct RecallTracker {
    conn: Arc<Mutex<Connection>>,
}

impl RecallTracker {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Record a single recall event.
    pub fn track(&self, item: &RecallItem, query: &str) -> Result<()> {
        self.track_batch(std::slice::from_ref(item), query)
    }

    /// Record a batch of recall events for one query. Single transaction.
    pub fn track_batch(&self, items: &[RecallItem], query: &str) -> Result<()> {
        if items.is_empty() || query.is_empty() {
            return Ok(());
        }
        let query_hash = Self::hash(&Self::normalize(query));
        let mut conn = self.conn.lock().expect("recall conn mutex poisoned");
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO recall_log (
                    doc_id, doc_path, content_hash, query_hash,
                    score, source, recalled_at, recall_day
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), date('now'))",
            )?;
            for item in items {
                let snippet: String = item.content.chars().take(280).collect();
                let content_hash = Self::hash(&Self::normalize(&snippet));
                stmt.execute(params![
                    item.doc_id,
                    item.doc_path,
                    content_hash,
                    query_hash,
                    item.score,
                    item.source,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Rebuild `recall_stats` from `recall_log`.
    ///
    /// Mirrors the Python SQL (GROUP BY content_hash with COUNT /
    /// COUNT(DISTINCT) / SUM / MAX / MIN), then UPSERTs each aggregate row
    /// while preserving `first_recalled_at` on existing entries.
    pub fn rebuild_stats(&self) -> Result<usize> {
        let conn = self.conn.lock().expect("recall conn mutex poisoned");
        let rows: Vec<StatsAggregate> = {
            let mut stmt = conn.prepare(
                "SELECT
                    content_hash,
                    MAX(doc_id) AS doc_id,
                    MAX(doc_path) AS doc_path,
                    COUNT(*) AS recall_count,
                    COUNT(DISTINCT query_hash) AS unique_queries,
                    COUNT(DISTINCT recall_day) AS unique_days,
                    SUM(score) AS total_score,
                    MAX(score) AS max_score,
                    MIN(recalled_at) AS first_recalled_at,
                    MAX(recalled_at) AS last_recalled_at
                 FROM recall_log
                 GROUP BY content_hash",
            )?;
            let iter = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, f64>(6)?,
                    r.get::<_, f64>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                ))
            })?;
            iter.collect::<std::result::Result<Vec<_>, _>>()?
        };

        for (
            content_hash,
            doc_id,
            doc_path,
            recall_count,
            unique_queries,
            unique_days,
            total_score,
            max_score,
            first_recalled_at,
            last_recalled_at,
        ) in rows.iter()
        {
            conn.execute(
                "INSERT INTO recall_stats (
                    content_hash, doc_id, doc_path,
                    recall_count, unique_queries, unique_days,
                    total_score, max_score,
                    first_recalled_at, last_recalled_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(content_hash) DO UPDATE SET
                    doc_id = excluded.doc_id,
                    doc_path = excluded.doc_path,
                    recall_count = excluded.recall_count,
                    unique_queries = excluded.unique_queries,
                    unique_days = excluded.unique_days,
                    total_score = excluded.total_score,
                    max_score = excluded.max_score,
                    first_recalled_at = COALESCE(
                        recall_stats.first_recalled_at, excluded.first_recalled_at
                    ),
                    last_recalled_at = excluded.last_recalled_at",
                params![
                    content_hash,
                    doc_id,
                    doc_path,
                    recall_count,
                    unique_queries,
                    unique_days,
                    total_score,
                    max_score,
                    first_recalled_at,
                    last_recalled_at,
                ],
            )?;
        }

        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM recall_stats", [], |r| r.get(0))?;
        Ok(count as usize)
    }

    /// Top-N docs by recall_count. Returns `(doc_path, recall_count)`.
    pub fn top_recalled(&self, limit: usize) -> Result<Vec<(String, u32)>> {
        let conn = self.conn.lock().expect("recall conn mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT doc_path, recall_count FROM recall_stats
             ORDER BY recall_count DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Increment `consolidation_hits` for a content_hash (used by SANCHO dream).
    pub fn record_consolidation_hit(&self, content_hash: &str) -> Result<()> {
        let conn = self.conn.lock().expect("recall conn mutex poisoned");
        conn.execute(
            "UPDATE recall_stats
             SET consolidation_hits = consolidation_hits + 1
             WHERE content_hash = ?1",
            params![content_hash],
        )?;
        Ok(())
    }

    /// Prune rows older than `max_age_days`. Returns number deleted.
    pub fn prune_old_logs(&self, max_age_days: i64) -> Result<usize> {
        let conn = self.conn.lock().expect("recall conn mutex poisoned");
        let modifier = format!("-{max_age_days} days");
        let n = conn.execute(
            "DELETE FROM recall_log WHERE recall_day < date('now', ?1)",
            params![modifier],
        )?;
        Ok(n)
    }

    // ── helpers ───────────────────────────────────────────────────

    fn normalize(text: &str) -> String {
        text.split_whitespace()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn hash(text: &str) -> String {
        let digest = blake3::hash(text.as_bytes());
        digest.to_hex().as_str().chars().take(12).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use tempfile::tempdir;

    fn make_tracker() -> (tempfile::TempDir, RecallTracker) {
        let dir = tempdir().unwrap();
        let conn = open_db(&dir.path().join("sb.db")).unwrap();
        run_migrations(&conn).unwrap();
        let tracker = RecallTracker::new(Arc::new(Mutex::new(conn)));
        (dir, tracker)
    }

    #[test]
    fn normalize_lowercases_and_collapses_whitespace() {
        assert_eq!(RecallTracker::normalize("  Hello   World "), "hello world");
    }

    #[test]
    fn hash_is_deterministic_and_12_chars() {
        let a = RecallTracker::hash("foo bar");
        let b = RecallTracker::hash("foo bar");
        assert_eq!(a, b);
        assert_eq!(a.len(), 12);
        let c = RecallTracker::hash("foo baz");
        assert_ne!(a, c);
    }

    #[test]
    fn track_batch_inserts_all_rows() {
        let (_d, t) = make_tracker();
        let items: Vec<RecallItem> = (0..50)
            .map(|i| RecallItem::new(i, format!("doc/{i}.md"), format!("snippet for doc {i}")))
            .collect();
        t.track_batch(&items, "test query").unwrap();

        let conn = t.conn.lock().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 50);
    }

    #[test]
    fn rebuild_stats_aggregates_duplicates() {
        let (_d, t) = make_tracker();
        // Same doc hit 3x across 2 different queries.
        let item = RecallItem::new(1, "doc/a.md", "shared content body");
        t.track(&item, "query one").unwrap();
        t.track(&item, "query one").unwrap();
        t.track(&item, "query two").unwrap();

        let stats_count = t.rebuild_stats().unwrap();
        assert_eq!(stats_count, 1);

        let conn = t.conn.lock().unwrap();
        let (rc, uq): (i64, i64) = conn
            .query_row(
                "SELECT recall_count, unique_queries FROM recall_stats",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rc, 3);
        assert_eq!(uq, 2);
    }

    #[test]
    fn top_recalled_orders_by_count() {
        let (_d, t) = make_tracker();
        let hot = RecallItem::new(1, "hot.md", "hot content");
        let warm = RecallItem::new(2, "warm.md", "warm content");
        for _ in 0..5 {
            t.track(&hot, "q").unwrap();
        }
        for _ in 0..2 {
            t.track(&warm, "q").unwrap();
        }
        t.rebuild_stats().unwrap();
        let top = t.top_recalled(10).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "hot.md");
        assert_eq!(top[0].1, 5);
        assert_eq!(top[1].0, "warm.md");
    }

    #[test]
    fn prune_old_logs_removes_nothing_when_fresh() {
        let (_d, t) = make_tracker();
        t.track(&RecallItem::new(1, "x", "y"), "q").unwrap();
        let deleted = t.prune_old_logs(30).unwrap();
        assert_eq!(deleted, 0);
    }
}
