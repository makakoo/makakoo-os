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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};

use crate::error::Result;

/// Canonicalize a Brain doc path to a `$MAKAKOO_HOME`-rooted form.
///
/// Three jobs, in order:
///   1. Rewrite the legacy `/Users/sebastian/HARVEY/` prefix (or any
///      `$HOME/HARVEY/` style path) to the `$MAKAKOO_HOME` equivalent.
///      The symlink `~/HARVEY → ~/MAKAKOO` normally resolves both to the
///      same inode, but we do the rewrite explicitly so the stored path
///      stays valid after the symlink is removed.
///   2. If the path is relative, prepend `$MAKAKOO_HOME` (falling back
///      to `$HARVEY_HOME`, then `$HOME/MAKAKOO`).
///   3. Best-effort `fs::canonicalize` to resolve any remaining symlinks.
///      If canonicalization fails (path doesn't exist yet, permissions,
///      etc.) the non-canonical string is returned — this is a stored-
///      data normalizer, not a filesystem assertion.
pub fn canonicalize_brain_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let makakoo_home = std::env::var("MAKAKOO_HOME")
        .or_else(|_| std::env::var("HARVEY_HOME"))
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/sebastian".to_string());
            format!("{home}/MAKAKOO")
        });

    // Step 1: legacy HARVEY prefix rewrite. Cover both the hard-coded
    // Sebastian path and any `$HOME/HARVEY/` form. The rewrite target is
    // the same `$MAKAKOO_HOME` either way.
    let mut rewritten = if let Some(rest) = path.strip_prefix("/Users/sebastian/HARVEY/") {
        format!("{}/{}", makakoo_home.trim_end_matches('/'), rest)
    } else if path == "/Users/sebastian/HARVEY" {
        makakoo_home.clone()
    } else if let Ok(home) = std::env::var("HOME") {
        let harvey_prefix = format!("{home}/HARVEY/");
        if let Some(rest) = path.strip_prefix(&harvey_prefix) {
            format!("{}/{}", makakoo_home.trim_end_matches('/'), rest)
        } else if path == format!("{home}/HARVEY") {
            makakoo_home.clone()
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };

    // Step 2: relative path resolution.
    if !Path::new(&rewritten).is_absolute() {
        let joined = PathBuf::from(&makakoo_home).join(&rewritten);
        rewritten = joined.to_string_lossy().to_string();
    }

    // Step 3: best-effort canonicalize. Ignore errors (the path may not
    // exist yet, or live on a removed volume). Canonical path must still
    // be UTF-8; fall back to the rewritten string if conversion fails.
    if let Ok(canon) = std::fs::canonicalize(&rewritten) {
        if let Some(s) = canon.to_str() {
            return s.to_string();
        }
    }
    rewritten
}

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

/// Report from [`migrate_legacy_paths`]. All counts are the number of
/// rows rewritten (or that would be rewritten in dry-run mode).
#[derive(Debug, Default, Clone)]
pub struct LegacyPathReport {
    pub recall_log_rewritten: i64,
    pub recall_stats_rewritten: i64,
    pub memory_promotions_rewritten: i64,
    pub recall_stats_deduped: i64,
}

/// Rewrite legacy `/Users/sebastian/HARVEY/` paths to the equivalent
/// `/Users/sebastian/MAKAKOO/` paths across `recall_log`, `recall_stats`,
/// and `memory_promotions`. Deduplicate `recall_stats` on `content_hash`
/// (keep oldest rowid) if the rewrite creates collisions.
///
/// In `dry_run` mode the report contains the counts that WOULD be
/// rewritten but no UPDATE runs. Idempotent — a second run produces a
/// zero-count report.
pub fn migrate_legacy_paths(conn: &Connection, dry_run: bool) -> Result<LegacyPathReport> {
    let count_prefix = |table: &str| -> Result<i64> {
        let sql = format!(
            "SELECT COUNT(*) FROM {table} WHERE doc_path LIKE '/Users/sebastian/HARVEY/%'"
        );
        Ok(conn.query_row(&sql, [], |r| r.get::<_, i64>(0))?)
    };

    let recall_log_n = count_prefix("recall_log")?;
    let recall_stats_n = count_prefix("recall_stats")?;
    let memory_promotions_n = count_prefix("memory_promotions")?;

    let mut report = LegacyPathReport {
        recall_log_rewritten: recall_log_n,
        recall_stats_rewritten: recall_stats_n,
        memory_promotions_rewritten: memory_promotions_n,
        ..Default::default()
    };

    if dry_run {
        // Still compute the would-be dedup count so the user sees the full
        // shape of the operation. Copy recall_stats rows into a temp view
        // after simulated rewrite, count collisions.
        let would_dedup: i64 = conn.query_row(
            "SELECT COALESCE(SUM(n), 0) FROM (
                SELECT COUNT(*) - 1 AS n
                FROM recall_stats
                GROUP BY content_hash,
                    REPLACE(doc_path, '/Users/sebastian/HARVEY/', '/Users/sebastian/MAKAKOO/')
                HAVING COUNT(*) > 1
            )",
            [],
            |r| r.get(0),
        )?;
        report.recall_stats_deduped = would_dedup;
        return Ok(report);
    }

    // ── real run ─────────────────────────────────────────────────
    conn.execute(
        "UPDATE recall_log
            SET doc_path = REPLACE(doc_path,
                '/Users/sebastian/HARVEY/', '/Users/sebastian/MAKAKOO/')
          WHERE doc_path LIKE '/Users/sebastian/HARVEY/%'",
        [],
    )?;
    conn.execute(
        "UPDATE recall_stats
            SET doc_path = REPLACE(doc_path,
                '/Users/sebastian/HARVEY/', '/Users/sebastian/MAKAKOO/')
          WHERE doc_path LIKE '/Users/sebastian/HARVEY/%'",
        [],
    )?;
    conn.execute(
        "UPDATE memory_promotions
            SET doc_path = REPLACE(doc_path,
                '/Users/sebastian/HARVEY/', '/Users/sebastian/MAKAKOO/')
          WHERE doc_path LIKE '/Users/sebastian/HARVEY/%'",
        [],
    )?;

    // Deduplicate recall_stats on content_hash: rewrite may have
    // converted two different-path rows (HARVEY vs MAKAKOO) into two
    // identical content_hash rows. Keep the oldest rowid to preserve
    // first_recalled_at history.
    let dedup_n = conn.execute(
        "DELETE FROM recall_stats
          WHERE rowid NOT IN (
              SELECT MIN(rowid) FROM recall_stats GROUP BY content_hash
          )",
        [],
    )? as i64;
    report.recall_stats_deduped = dedup_n;

    Ok(report)
}

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
                let canonical_path = canonicalize_brain_path(&item.doc_path);
                stmt.execute(params![
                    item.doc_id,
                    canonical_path,
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
        // Absolute paths so canonicalize_brain_path returns them unchanged
        // (no HARVEY prefix, no relative resolution).
        let hot = RecallItem::new(1, "/tmp/recall-test-hot.md", "hot content");
        let warm = RecallItem::new(2, "/tmp/recall-test-warm.md", "warm content");
        for _ in 0..5 {
            t.track(&hot, "q").unwrap();
        }
        for _ in 0..2 {
            t.track(&warm, "q").unwrap();
        }
        t.rebuild_stats().unwrap();
        let top = t.top_recalled(10).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "/tmp/recall-test-hot.md");
        assert_eq!(top[0].1, 5);
        assert_eq!(top[1].0, "/tmp/recall-test-warm.md");
    }

    #[test]
    fn prune_old_logs_removes_nothing_when_fresh() {
        let (_d, t) = make_tracker();
        t.track(&RecallItem::new(1, "x", "y"), "q").unwrap();
        let deleted = t.prune_old_logs(30).unwrap();
        assert_eq!(deleted, 0);
    }

    // ── canonicalize_brain_path helper tests ──────────────────────
    // These tests mutate env vars. Serialize them on a Mutex so parallel
    // test execution doesn't race on $MAKAKOO_HOME.
    use std::sync::Mutex as StdMutex;
    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn with_makakoo_home<F: FnOnce()>(home: &str, f: F) {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_mak = std::env::var("MAKAKOO_HOME").ok();
        let prev_har = std::env::var("HARVEY_HOME").ok();
        std::env::set_var("MAKAKOO_HOME", home);
        std::env::remove_var("HARVEY_HOME");
        f();
        match prev_mak {
            Some(v) => std::env::set_var("MAKAKOO_HOME", v),
            None => std::env::remove_var("MAKAKOO_HOME"),
        }
        if let Some(v) = prev_har {
            std::env::set_var("HARVEY_HOME", v);
        }
    }

    #[test]
    fn canonicalize_rewrites_harvey_to_makakoo() {
        with_makakoo_home("/tmp/nonexistent-makakoo-home", || {
            let out = canonicalize_brain_path("/Users/sebastian/HARVEY/data/Brain/x.md");
            assert_eq!(out, "/tmp/nonexistent-makakoo-home/data/Brain/x.md");
        });
    }

    #[test]
    fn canonicalize_leaves_makakoo_paths_alone() {
        with_makakoo_home("/tmp/nonexistent-makakoo-home", || {
            let out = canonicalize_brain_path(
                "/tmp/nonexistent-makakoo-home/data/Brain/journals/2026_04_19.md",
            );
            // No existing file → canonicalize best-effort returns input unchanged.
            assert_eq!(
                out,
                "/tmp/nonexistent-makakoo-home/data/Brain/journals/2026_04_19.md"
            );
        });
    }

    #[test]
    fn canonicalize_resolves_relative_paths() {
        with_makakoo_home("/tmp/rel-makakoo-home", || {
            let out = canonicalize_brain_path("data/Brain/pages/Tytus.md");
            assert_eq!(out, "/tmp/rel-makakoo-home/data/Brain/pages/Tytus.md");
        });
    }

    #[test]
    fn canonicalize_empty_input_returns_empty() {
        let out = canonicalize_brain_path("");
        assert_eq!(out, "");
    }

    #[test]
    fn canonicalize_bare_harvey_home() {
        with_makakoo_home("/mak", || {
            let out = canonicalize_brain_path("/Users/sebastian/HARVEY");
            assert_eq!(out, "/mak");
        });
    }

    // ── Phase B — migrate_legacy_paths tests ─────────────────────
    fn seed_harvey_rows(conn: &Connection) {
        // recall_log
        conn.execute(
            "INSERT INTO recall_log
                (doc_id, doc_path, content_hash, query_hash, score, source)
             VALUES (1, '/Users/sebastian/HARVEY/data/Brain/pages/Tytus.md',
                     'h1', 'q1', 0.9, 'superbrain')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recall_log
                (doc_id, doc_path, content_hash, query_hash, score, source)
             VALUES (2, '/Users/sebastian/HARVEY/data/Brain/journals/2026_04_15.md',
                     'h2', 'q1', 0.8, 'superbrain')",
            [],
        )
        .unwrap();
        // recall_stats
        conn.execute(
            "INSERT INTO recall_stats
                (content_hash, doc_id, doc_path, recall_count, unique_queries,
                 unique_days, total_score, max_score)
             VALUES ('h1', 1, '/Users/sebastian/HARVEY/data/Brain/pages/Tytus.md',
                     3, 2, 2, 2.5, 0.9)",
            [],
        )
        .unwrap();
        // memory_promotions
        conn.execute(
            "INSERT INTO memory_promotions
                (content_hash, doc_id, doc_path, promoted_at, reason)
             VALUES ('h1', 1, '/Users/sebastian/HARVEY/data/Brain/pages/Tytus.md',
                     datetime('now'), 'score=0.82')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn migrate_legacy_paths_dry_run_reports_counts_without_writing() {
        let (_d, t) = make_tracker();
        {
            let conn = t.conn.lock().unwrap();
            seed_harvey_rows(&conn);
            let rep = migrate_legacy_paths(&conn, true).unwrap();
            assert_eq!(rep.recall_log_rewritten, 2);
            assert_eq!(rep.recall_stats_rewritten, 1);
            assert_eq!(rep.memory_promotions_rewritten, 1);
            // Still on HARVEY path after dry-run.
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM recall_log WHERE doc_path LIKE '/Users/sebastian/HARVEY/%'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 2);
        }
    }

    #[test]
    fn migrate_legacy_paths_rewrites_all_three_tables() {
        let (_d, t) = make_tracker();
        {
            let conn = t.conn.lock().unwrap();
            seed_harvey_rows(&conn);
            let rep = migrate_legacy_paths(&conn, false).unwrap();
            assert_eq!(rep.recall_log_rewritten, 2);
            assert_eq!(rep.recall_stats_rewritten, 1);
            assert_eq!(rep.memory_promotions_rewritten, 1);

            for table in ["recall_log", "recall_stats", "memory_promotions"] {
                let n: i64 = conn
                    .query_row(
                        &format!(
                            "SELECT COUNT(*) FROM {table} WHERE doc_path LIKE '/Users/sebastian/HARVEY/%'"
                        ),
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(n, 0, "{table} still has HARVEY paths");
            }
        }
    }

    #[test]
    fn migrate_legacy_paths_is_idempotent() {
        let (_d, t) = make_tracker();
        {
            let conn = t.conn.lock().unwrap();
            seed_harvey_rows(&conn);
            let _ = migrate_legacy_paths(&conn, false).unwrap();
            let rep2 = migrate_legacy_paths(&conn, false).unwrap();
            assert_eq!(rep2.recall_log_rewritten, 0);
            assert_eq!(rep2.recall_stats_rewritten, 0);
            assert_eq!(rep2.memory_promotions_rewritten, 0);
        }
    }

    #[test]
    fn migrate_legacy_paths_dedupes_content_hash_collisions() {
        let (_d, t) = make_tracker();
        {
            let conn = t.conn.lock().unwrap();
            // Seed a pair that will collide on content_hash after rewrite:
            // same hash, HARVEY and MAKAKOO paths already present.
            conn.execute(
                "INSERT INTO recall_stats (content_hash, doc_id, doc_path, recall_count, unique_queries, unique_days, total_score, max_score)
                 VALUES ('collide', 1, '/Users/sebastian/HARVEY/x.md', 2, 1, 1, 0.8, 0.8)",
                [],
            ).unwrap();
            // Now rewrite — after rewrite the HARVEY row becomes MAKAKOO/x.md.
            // Seed an existing MAKAKOO row with same content_hash.
            conn.execute(
                "INSERT INTO recall_stats (content_hash, doc_id, doc_path, recall_count, unique_queries, unique_days, total_score, max_score)
                 VALUES ('collide', 1, '/tmp/other.md', 2, 1, 1, 0.8, 0.8)",
                [],
            ).unwrap_err();
            // PRIMARY KEY(content_hash) blocks the raw second insert.
            // So the real collision scenario for this schema is: schemas
            // use content_hash PRIMARY KEY — dedup is defense-in-depth only.
            // Verify the DELETE runs with no spurious removals when there
            // are no duplicates possible.
            let n_before: i64 = conn
                .query_row("SELECT COUNT(*) FROM recall_stats", [], |r| r.get(0))
                .unwrap();
            let rep = migrate_legacy_paths(&conn, false).unwrap();
            let n_after: i64 = conn
                .query_row("SELECT COUNT(*) FROM recall_stats", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n_before, n_after, "dedup must not drop rows with no duplicates");
            assert_eq!(rep.recall_stats_deduped, 0);
        }
    }

    #[test]
    fn track_batch_canonicalizes_harvey_path_before_insert() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("MAKAKOO_HOME", "/tmp/t-canon-home");
        std::env::remove_var("HARVEY_HOME");
        let (_d, t) = make_tracker();
        let item = RecallItem::new(1, "/Users/sebastian/HARVEY/data/Brain/a.md", "body");
        t.track(&item, "query").unwrap();
        let conn = t.conn.lock().unwrap();
        let path: String = conn
            .query_row("SELECT doc_path FROM recall_log LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(path, "/tmp/t-canon-home/data/Brain/a.md");
    }
}
