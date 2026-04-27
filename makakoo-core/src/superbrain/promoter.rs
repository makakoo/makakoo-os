//! Memory promoter — ports `core/memory/memory_promoter.py`.
//!
//! Scores `recall_stats` entries with Harvey's 6-component algorithm and
//! returns (or persists) the top promotion candidates. Weights are a
//! verbatim port from the Python oracle:
//!
//! ```text
//!   frequency     0.22
//!   relevance     0.28
//!   diversity     0.18
//!   recency       0.17
//!   consolidation 0.10
//!   conceptual    0.05
//! ```
//!
//! Plus a capped phase-boost (max 0.08) for consolidation encounters.
//! The `fn score_for_promotion` called out by the task spec is
//! implemented as `MemoryPromoter::score_for_promotion(doc_path)` —
//! anyone asking for a "doc_id" scoring API gets the same path-keyed
//! row from recall_stats that the Python promoter ranks.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection};

use crate::error::Result;

// ── weights (verbatim port from memory_promoter.py) ─────────────────
pub const W_FREQUENCY: f32 = 0.22;
pub const W_RELEVANCE: f32 = 0.28;
pub const W_DIVERSITY: f32 = 0.18;
pub const W_RECENCY: f32 = 0.17;
pub const W_CONSOLIDATION: f32 = 0.10;
pub const W_CONCEPTUAL: f32 = 0.05;
pub const PHASE_BOOST_MAX: f32 = 0.08;
pub const RECENCY_HALF_LIFE: f32 = 21.0;

// ── promotion gates ────────────────────────────────────────────────
pub const MIN_RECALL_COUNT: i64 = 3;
pub const MIN_UNIQUE_QUERIES: i64 = 2;
pub const MIN_SCORE: f32 = 0.70;
pub const MAX_AGE_DAYS: i64 = 45;
pub const MAX_PROMOTIONS_PER_RUN: usize = 8;

/// A candidate surfaced for promotion.
#[derive(Debug, Clone)]
pub struct Promotion {
    pub content_hash: String,
    pub doc_id: i64,
    pub doc_path: String,
    pub score: f32,
    pub components: ScoreComponents,
}

/// Per-component breakdown for auditability.
#[derive(Debug, Clone, Default)]
pub struct ScoreComponents {
    pub frequency: f32,
    pub relevance: f32,
    pub diversity: f32,
    pub recency: f32,
    pub consolidation: f32,
    pub conceptual: f32,
    pub phase_boost: f32,
    pub composite: f32,
}

/// A single row loaded from `recall_stats`. Intentionally public so T5
/// callers can inject fixtures in tests and SANCHO can reuse the type.
#[derive(Debug, Clone)]
pub struct RecallStatsRow {
    pub content_hash: String,
    pub doc_id: i64,
    pub doc_path: String,
    pub recall_count: i64,
    pub unique_queries: i64,
    pub unique_days: i64,
    pub total_score: f64,
    pub max_score: f64,
    pub first_recalled_at: Option<String>,
    pub last_recalled_at: Option<String>,
    pub consolidation_hits: i64,
    pub promoted_at: Option<String>,
    pub concept_tag_count: u32,
}

pub struct MemoryPromoter {
    conn: Arc<Mutex<Connection>>,
}

impl MemoryPromoter {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Score one entry (used by tests and `rank_candidates`).
    pub fn score(&self, row: &RecallStatsRow) -> (f32, ScoreComponents) {
        score_row(row)
    }

    /// Load a single `recall_stats` row by `doc_path` and score it.
    /// Called out as `score_for_promotion(doc_id)` in the task spec —
    /// we key on `doc_path` because that's the stable identifier in
    /// recall_stats (the integer `doc_id` can collide across deletes).
    pub fn score_for_promotion(&self, doc_path: &str) -> Result<Option<f32>> {
        let conn = self.conn.lock().expect("promoter conn poisoned");
        let row = load_row_by_path(&conn, doc_path)?;
        Ok(row.map(|r| score_row(&r).0))
    }

    /// Rank all non-promoted candidates by composite score. Emits a
    /// single `info` tracing event per run with kill-count breakdown by
    /// filter, so `makakoo memory stats` and operator logs have
    /// observable pipeline pressure.
    pub fn rank_candidates(&self) -> Result<Vec<Promotion>> {
        let conn = self.conn.lock().expect("promoter conn poisoned");
        let rows = load_all_rows(&conn)?;
        drop(conn);

        let total = rows.len();
        let mut killed_promoted = 0usize;
        let mut killed_recall_count = 0usize;
        let mut killed_unique_queries = 0usize;
        let mut killed_age = 0usize;
        let mut killed_score = 0usize;
        let mut out = Vec::new();
        for r in rows {
            if r.promoted_at.is_some() {
                killed_promoted += 1;
                continue;
            }
            if r.recall_count < MIN_RECALL_COUNT {
                killed_recall_count += 1;
                continue;
            }
            if r.unique_queries < MIN_UNIQUE_QUERIES {
                killed_unique_queries += 1;
                continue;
            }
            if let Some(age) = age_days(&r.first_recalled_at) {
                if age > MAX_AGE_DAYS {
                    killed_age += 1;
                    continue;
                }
            }
            let (score, components) = score_row(&r);
            if score < MIN_SCORE {
                killed_score += 1;
                continue;
            }
            out.push(Promotion {
                content_hash: r.content_hash,
                doc_id: r.doc_id,
                doc_path: r.doc_path,
                score,
                components,
            });
        }
        tracing::info!(
            total_candidates = total,
            already_promoted = killed_promoted,
            killed_recall_count_lt_min = killed_recall_count,
            killed_unique_queries_lt_min = killed_unique_queries,
            killed_age_gt_max = killed_age,
            killed_score_lt_min = killed_score,
            ranked = out.len(),
            "memory_promoter: rank_candidates"
        );
        out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(MAX_PROMOTIONS_PER_RUN);
        Ok(out)
    }

    /// Write the top candidates above `threshold` to `memory_promotions`
    /// and stamp `promoted_at` on recall_stats.
    pub fn promote_candidates(
        &self,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<Promotion>> {
        let ranked = self.rank_candidates()?;
        let picks: Vec<Promotion> = ranked
            .into_iter()
            .filter(|p| p.score >= threshold)
            .take(limit)
            .collect();
        if picks.is_empty() {
            return Ok(picks);
        }
        let conn = self.conn.lock().expect("promoter conn poisoned");
        for p in &picks {
            conn.execute(
                "INSERT INTO memory_promotions
                    (content_hash, doc_id, doc_path, promoted_at, reason)
                 VALUES (?1, ?2, ?3, datetime('now'), ?4)",
                params![
                    p.content_hash,
                    p.doc_id,
                    p.doc_path,
                    format!("score={:.3}", p.score),
                ],
            )?;
            conn.execute(
                "UPDATE recall_stats SET promoted_at = datetime('now')
                 WHERE content_hash = ?1",
                params![p.content_hash],
            )?;
        }
        Ok(picks)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Pure scoring
// ─────────────────────────────────────────────────────────────────────

fn score_row(r: &RecallStatsRow) -> (f32, ScoreComponents) {
    let freq = frequency(r);
    let rel = relevance(r);
    let div = diversity(r);
    let rec = recency(r);
    let con = consolidation(r);
    let cpt = conceptual(r);
    let phase = phase_boost(r, rec);

    let composite = (W_FREQUENCY * freq
        + W_RELEVANCE * rel
        + W_DIVERSITY * div
        + W_RECENCY * rec
        + W_CONSOLIDATION * con
        + W_CONCEPTUAL * cpt
        + phase)
        .min(1.0);

    let components = ScoreComponents {
        frequency: freq,
        relevance: rel,
        diversity: div,
        recency: rec,
        consolidation: con,
        conceptual: cpt,
        phase_boost: phase,
        composite,
    };
    (composite, components)
}

fn frequency(r: &RecallStatsRow) -> f32 {
    let signals = (r.recall_count + r.consolidation_hits) as f32;
    if signals <= 0.0 {
        return 0.0;
    }
    (signals.ln_1p() / 10.0_f32.ln_1p()).min(1.0)
}

fn relevance(r: &RecallStatsRow) -> f32 {
    let count = r.recall_count.max(1) as f32;
    (r.total_score as f32 / count).clamp(0.0, 1.0)
}

fn diversity(r: &RecallStatsRow) -> f32 {
    let uq = r.unique_queries as f32;
    let ud = r.unique_days as f32;
    (uq.max(ud) / 5.0).min(1.0)
}

fn recency(r: &RecallStatsRow) -> f32 {
    let Some(last) = r.last_recalled_at.as_ref() else {
        return 0.1;
    };
    let Some(age) = age_days(&Some(last.clone())) else {
        return 0.1;
    };
    let lam = std::f32::consts::LN_2 / RECENCY_HALF_LIFE;
    (-lam * age.max(0) as f32).exp()
}

fn consolidation(r: &RecallStatsRow) -> f32 {
    let ud = r.unique_days;
    if ud == 0 {
        return 0.0;
    }
    if ud == 1 {
        return 0.2;
    }
    let spacing = ((ud - 1) as f32).ln_1p() / 4.0_f32.ln_1p();
    let span_days = span_days(&r.first_recalled_at, &r.last_recalled_at).unwrap_or(0);
    let span = (span_days as f32 / 7.0).min(1.0);
    0.55 * spacing.min(1.0) + 0.45 * span
}

fn conceptual(r: &RecallStatsRow) -> f32 {
    (r.concept_tag_count as f32 / 6.0).min(1.0)
}

fn phase_boost(r: &RecallStatsRow, rec_score: f32) -> f32 {
    if r.consolidation_hits == 0 {
        return 0.0;
    }
    let strength = ((r.consolidation_hits as f32).ln_1p() / 6.0_f32.ln_1p()).min(1.0);
    (PHASE_BOOST_MAX * strength * rec_score).min(PHASE_BOOST_MAX)
}

fn age_days(ts: &Option<String>) -> Option<i64> {
    let s = ts.as_ref()?;
    let trimmed = s.split('+').next().unwrap_or(s).replace('T', " ");
    let parsed = DateTime::<Utc>::from_naive_utc_and_offset(
        NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%d %H:%M:%S").ok()?,
        Utc,
    );
    Some((Utc::now() - parsed).num_days())
}

fn span_days(first: &Option<String>, last: &Option<String>) -> Option<i64> {
    let (f, l) = match (first, last) {
        (Some(a), Some(b)) => (a, b),
        _ => return None,
    };
    let parse = |s: &str| -> Option<DateTime<Utc>> {
        let trimmed = s.split('+').next().unwrap_or(s).replace('T', " ");
        let ndt = NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%d %H:%M:%S").ok()?;
        Some(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc))
    };
    let fa = parse(f)?;
    let la = parse(l)?;
    Some((la - fa).num_days())
}

// ─────────────────────────────────────────────────────────────────────
// DB helpers
// ─────────────────────────────────────────────────────────────────────

fn load_all_rows(conn: &Connection) -> Result<Vec<RecallStatsRow>> {
    let mut stmt = conn.prepare(
        "SELECT content_hash, doc_id, doc_path,
                recall_count, unique_queries, unique_days,
                total_score, max_score,
                first_recalled_at, last_recalled_at,
                consolidation_hits, promoted_at, concept_tags
         FROM recall_stats
         ORDER BY recall_count DESC",
    )?;
    let rows = stmt
        .query_map([], row_to_stats)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_row_by_path(conn: &Connection, doc_path: &str) -> Result<Option<RecallStatsRow>> {
    let mut stmt = conn.prepare(
        "SELECT content_hash, doc_id, doc_path,
                recall_count, unique_queries, unique_days,
                total_score, max_score,
                first_recalled_at, last_recalled_at,
                consolidation_hits, promoted_at, concept_tags
         FROM recall_stats WHERE doc_path = ?1 LIMIT 1",
    )?;
    let rows = stmt
        .query_map(params![doc_path], row_to_stats)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().next())
}

fn row_to_stats(r: &rusqlite::Row<'_>) -> rusqlite::Result<RecallStatsRow> {
    let concept_tags_raw: String = r.get(12)?;
    let concept_tag_count = parse_concept_tag_count(&concept_tags_raw);
    Ok(RecallStatsRow {
        content_hash: r.get(0)?,
        doc_id: r.get(1)?,
        doc_path: r.get(2)?,
        recall_count: r.get(3)?,
        unique_queries: r.get(4)?,
        unique_days: r.get(5)?,
        total_score: r.get(6)?,
        max_score: r.get(7)?,
        first_recalled_at: r.get(8)?,
        last_recalled_at: r.get(9)?,
        consolidation_hits: r.get(10)?,
        promoted_at: r.get(11)?,
        concept_tag_count,
    })
}

fn parse_concept_tag_count(raw: &str) -> u32 {
    serde_json::from_str::<Vec<String>>(raw)
        .map(|v| v.len() as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use tempfile::tempdir;

    fn make_promoter() -> (tempfile::TempDir, MemoryPromoter, Arc<Mutex<Connection>>) {
        let dir = tempdir().unwrap();
        let conn = open_db(&dir.path().join("sb.db")).unwrap();
        run_migrations(&conn).unwrap();
        let shared = Arc::new(Mutex::new(conn));
        (dir, MemoryPromoter::new(shared.clone()), shared)
    }

    #[allow(clippy::too_many_arguments)]
    fn seed_stat(
        shared: &Arc<Mutex<Connection>>,
        content_hash: &str,
        doc_path: &str,
        recall_count: i64,
        unique_queries: i64,
        unique_days: i64,
        total_score: f64,
        last_recalled_at: &str,
        first_recalled_at: &str,
    ) {
        let conn = shared.lock().unwrap();
        conn.execute(
            "INSERT INTO recall_stats
                (content_hash, doc_id, doc_path, recall_count, unique_queries,
                 unique_days, total_score, max_score,
                 first_recalled_at, last_recalled_at,
                 consolidation_hits, concept_tags)
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, 0.9, ?7, ?8, 0, '[]')",
            params![
                content_hash,
                doc_path,
                recall_count,
                unique_queries,
                unique_days,
                total_score,
                first_recalled_at,
                last_recalled_at,
            ],
        )
        .unwrap();
    }

    #[test]
    fn frequency_is_zero_for_no_signals() {
        let r = RecallStatsRow {
            content_hash: "h".into(),
            doc_id: 0,
            doc_path: "".into(),
            recall_count: 0,
            unique_queries: 0,
            unique_days: 0,
            total_score: 0.0,
            max_score: 0.0,
            first_recalled_at: None,
            last_recalled_at: None,
            consolidation_hits: 0,
            promoted_at: None,
            concept_tag_count: 0,
        };
        assert_eq!(frequency(&r), 0.0);
    }

    #[test]
    fn diversity_saturates_at_five() {
        let mut r = RecallStatsRow {
            content_hash: "h".into(),
            doc_id: 0,
            doc_path: "".into(),
            recall_count: 0,
            unique_queries: 5,
            unique_days: 5,
            total_score: 0.0,
            max_score: 0.0,
            first_recalled_at: None,
            last_recalled_at: None,
            consolidation_hits: 0,
            promoted_at: None,
            concept_tag_count: 0,
        };
        assert_eq!(diversity(&r), 1.0);
        r.unique_queries = 10;
        assert_eq!(diversity(&r), 1.0);
    }

    #[test]
    fn rank_candidates_picks_hot_row() {
        let (_d, p, shared) = make_promoter();
        // Hot row — recent, many hits, diverse queries, high score.
        let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let earlier = (Utc::now() - chrono::Duration::days(3))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        seed_stat(&shared, "hot", "hot.md", 12, 5, 4, 11.0, &now, &earlier);
        // Cold row — only one recall, one query → below gates.
        seed_stat(&shared, "cold", "cold.md", 1, 1, 1, 0.5, &now, &now);

        let picks = p.rank_candidates().unwrap();
        assert!(!picks.is_empty());
        assert_eq!(picks[0].content_hash, "hot");
        assert!(picks[0].score >= MIN_SCORE);
    }

    #[test]
    fn promote_candidates_stamps_recall_stats() {
        let (_d, p, shared) = make_promoter();
        let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let earlier = (Utc::now() - chrono::Duration::days(3))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        seed_stat(&shared, "hot", "hot.md", 12, 5, 4, 11.0, &now, &earlier);

        let picks = p.promote_candidates(MIN_SCORE, 5).unwrap();
        assert_eq!(picks.len(), 1);
        let conn = shared.lock().unwrap();
        let promoted: Option<String> = conn
            .query_row(
                "SELECT promoted_at FROM recall_stats WHERE content_hash='hot'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(promoted.is_some());
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_promotions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn weights_sum_near_one() {
        let total = W_FREQUENCY + W_RELEVANCE + W_DIVERSITY + W_RECENCY
            + W_CONSOLIDATION + W_CONCEPTUAL;
        assert!((total - 1.0).abs() < 1e-4, "weights sum = {total}");
    }
}
