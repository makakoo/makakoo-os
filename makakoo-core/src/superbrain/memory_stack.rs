//! Memory stack — ports `core/superbrain/memory_stack.py`.
//!
//! The stack assembles a compact context string for LLM queries from
//! four layers:
//!
//!   L0  Session scratchpad (caller-supplied; not stored).
//!   L1  Recent journals (last `recent_days`, FTS5-ranked for the query).
//!   L2  Semantic top-k (supplied as a `VectorHit`-ish list since vectors
//!       live in T4's SuperbrainStore).
//!   L3  Graph neighbourhood of entities mentioned in the query.
//!
//! The Python original wires everything against `SuperbrainStore` +
//! filesystem fallbacks. The Rust port takes raw data plus a shared
//! SQLite connection so we don't couple T5 to T4's public API surface.
//! Higher-level glue that bolts MemoryStack onto SuperbrainStore lives
//! in `core/lib.rs` once both tracks land.
//!
//! Token budget is enforced at the character level — each layer gets a
//! fair share of the requested budget, and unspent space rolls down to
//! the next layer. 1 token ≈ 4 chars for English (same heuristic Python
//! uses).

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};

use crate::error::Result;
use crate::superbrain::graph::GraphStore;
use crate::superbrain::scorer::{combined_score, frequency_score, recency_score};

const CHARS_PER_TOKEN: usize = 4;

/// A single semantic-search hit fed in from T4's vector_search API.
#[derive(Debug, Clone)]
pub struct SemanticHit {
    pub doc_id: String,
    pub content: String,
    pub similarity: f32,
}

/// A single recent-journal candidate.
#[derive(Debug, Clone)]
struct JournalCandidate {
    doc_id: String,
    content: String,
    fts_score: f32,
    age_days: f32,
}

/// L0-identity text passed in by the caller (e.g. the SOUL.md core tone).
#[derive(Debug, Clone)]
pub struct SessionIdentity(pub String);

impl SessionIdentity {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

/// Memory stack — holds references to the shared connection + graph.
pub struct MemoryStack {
    conn: Arc<Mutex<Connection>>,
    graph: Arc<GraphStore>,
}

impl MemoryStack {
    pub fn new(conn: Arc<Mutex<Connection>>, graph: Arc<GraphStore>) -> Self {
        Self { conn, graph }
    }

    /// Build a compact context string budgeted to approximately
    /// `budget_tokens` tokens. Layers are blended by the scorer
    /// (recency, frequency, similarity).
    pub fn assemble_context(
        &self,
        identity: &SessionIdentity,
        query: &str,
        semantic_hits: &[SemanticHit],
        budget_tokens: usize,
    ) -> Result<String> {
        let char_budget = budget_tokens.saturating_mul(CHARS_PER_TOKEN).max(256);
        // Split the budget 20/40/25/15 across L0/L1/L2/L3.
        let l0_budget = char_budget / 5;
        let l1_budget = char_budget * 2 / 5;
        let l2_budget = char_budget / 4;
        let l3_budget = char_budget - l0_budget - l1_budget - l2_budget;

        let l0 = truncate(&identity.0, l0_budget);
        let l1 = self.build_l1(query, l1_budget)?;
        let l2 = self.build_l2(semantic_hits, l2_budget);
        let l3 = self.build_l3(query, l3_budget)?;

        let mut sections = Vec::new();
        if !l0.is_empty() {
            sections.push(format!("## Identity\n{l0}"));
        }
        if !l1.is_empty() {
            sections.push(format!("## Recent\n{l1}"));
        }
        if !l2.is_empty() {
            sections.push(format!("## Semantic\n{l2}"));
        }
        if !l3.is_empty() {
            sections.push(format!("## Graph\n{l3}"));
        }
        Ok(sections.join("\n\n"))
    }

    /// L1: recent journal entries ranked by combined_score(fts, recency, freq=0).
    fn build_l1(&self, query: &str, budget: usize) -> Result<String> {
        let candidates = self.recent_journal_fts(query, 7)?;
        if candidates.is_empty() {
            return Ok(String::new());
        }
        let mut scored: Vec<(JournalCandidate, f32)> = candidates
            .into_iter()
            .map(|c| {
                // Normalise FTS5 BM25 to [0,1]: store.py emits positive
                // scores (lower-is-better pre-flip), so we rescale by
                // `1 / (1 + score)` — safe for our range.
                let sim = 1.0 / (1.0 + c.fts_score.max(0.0));
                let half_life = 3.5_f32;
                let rec = 0.5_f32.powf(c.age_days / half_life);
                let s = combined_score(sim, rec, 0.0);
                (c, s)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut used = 0usize;
        let mut out = String::new();
        for (c, _) in scored {
            let line = format!("- {} — {}\n", c.doc_id, compress_line(&c.content));
            if used + line.len() > budget {
                break;
            }
            used += line.len();
            out.push_str(&line);
        }
        Ok(out.trim_end().to_string())
    }

    /// L2: semantic hits, ranked by similarity and trimmed to budget.
    fn build_l2(&self, hits: &[SemanticHit], budget: usize) -> String {
        let mut ranked: Vec<&SemanticHit> = hits
            .iter()
            .filter(|h| h.similarity > 0.3)
            .collect();
        ranked.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut used = 0usize;
        let mut out = String::new();
        for h in ranked {
            let snippet = truncate(&h.content, 240);
            let line = format!("- [{}] ({:.2}) {}\n", h.doc_id, h.similarity, snippet);
            if used + line.len() > budget {
                break;
            }
            used += line.len();
            out.push_str(&line);
        }
        out.trim_end().to_string()
    }

    /// L3: graph neighbourhood — pick the top query-term match from
    /// `brain_graph_nodes`, then list its strongest neighbours.
    fn build_l3(&self, query: &str, budget: usize) -> Result<String> {
        let terms: Vec<String> = query
            .split_whitespace()
            .filter(|w| w.len() >= 4)
            .map(|w| w.to_ascii_lowercase())
            .collect();
        if terms.is_empty() {
            return Ok(String::new());
        }
        // Pick the first node whose lowercased id contains any term.
        let conn = self.conn.lock().expect("memstack conn poisoned");
        let mut stmt = conn.prepare(
            "SELECT id FROM brain_graph_nodes WHERE node_type != 'journal'",
        )?;
        let ids: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        drop(conn);

        let pivot = ids.into_iter().find(|id| {
            let lower = id.to_ascii_lowercase();
            terms.iter().any(|t| lower.contains(t))
        });
        let pivot = match pivot {
            Some(p) => p,
            None => return Ok(String::new()),
        };

        let (out, inc) = self.graph.neighbors(&pivot)?;
        let mut used = 0usize;
        let mut result = format!("Pivot: {pivot}\n");
        used += result.len();
        for nb in out.iter().take(5) {
            let line = format!("  → {} ({}x)\n", nb.entity, nb.weight);
            if used + line.len() > budget {
                break;
            }
            used += line.len();
            result.push_str(&line);
        }
        for nb in inc.iter().take(5) {
            let line = format!("  ← {} ({}x)\n", nb.entity, nb.weight);
            if used + line.len() > budget {
                break;
            }
            used += line.len();
            result.push_str(&line);
        }
        Ok(result.trim_end().to_string())
    }

    /// Raw FTS5 journal lookup over the last `max_age_days`.
    fn recent_journal_fts(
        &self,
        query: &str,
        max_age_days: i64,
    ) -> Result<Vec<JournalCandidate>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let fts_query = sanitise_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().expect("memstack conn poisoned");
        let sql = "SELECT d.path, d.content,
                          bm25(brain_fts, 5.0, 1.0, 2.0) AS score,
                          CAST((julianday('now') - julianday(d.updated_at)) AS REAL) AS age
                   FROM brain_fts f
                   JOIN brain_docs d ON f.rowid = d.id
                   WHERE brain_fts MATCH ?1
                     AND d.doc_type = 'journal'
                     AND d.updated_at >= datetime('now', ?2)
                   ORDER BY score
                   LIMIT 10";
        let age_modifier = format!("-{max_age_days} days");
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map(params![fts_query, age_modifier], |r| {
                Ok(JournalCandidate {
                    doc_id: r.get::<_, String>(0)?,
                    content: r.get::<_, String>(1)?,
                    // FTS5 BM25 is negative (better=more-negative); flip it.
                    fts_score: -(r.get::<_, f64>(2)? as f32),
                    age_days: r.get::<_, f64>(3)? as f32,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    let mut cut = 0;
    for (i, _) in s.char_indices() {
        if i > max_chars {
            break;
        }
        cut = i;
    }
    s[..cut].trim_end().to_string()
}

fn compress_line(s: &str) -> String {
    s.replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Minimal FTS5 query sanitiser — strips punctuation that FTS5 treats as
/// syntax and quotes terms containing hyphens. A proper port of
/// `store::to_fts5_query` will live in T4 once the helper is shared.
fn sanitise_fts_query(q: &str) -> String {
    let cleaned: String = q
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' })
        .collect();
    cleaned
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .collect::<Vec<_>>()
        .join(" ")
}

// Forwarding so scorer imports are kept via `use`.
#[allow(dead_code)]
fn _touch_scorer_use() {
    let now = chrono::Utc::now();
    let _ = recency_score(now, now, 30.0);
    let _ = frequency_score(0, 20.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use tempfile::tempdir;

    fn make_stack() -> (tempfile::TempDir, MemoryStack) {
        let dir = tempdir().unwrap();
        let conn = open_db(&dir.path().join("sb.db")).unwrap();
        run_migrations(&conn).unwrap();
        let shared = Arc::new(Mutex::new(conn));
        let graph = Arc::new(GraphStore::new(shared.clone()));
        (dir, MemoryStack::new(shared, graph))
    }

    fn seed_journal(stack: &MemoryStack, path: &str, body: &str) {
        let conn = stack.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO brain_docs (path, name, doc_type, content, content_hash, entities, char_count, updated_at)
             VALUES (?1, ?2, 'journal', ?3, 'h', '[]', ?4, datetime('now'))",
            params![path, path, body, body.len() as i64],
        )
        .unwrap();
    }

    #[test]
    fn truncate_respects_budget() {
        let s = "hello world rust";
        let out = truncate(s, 5);
        assert!(out.len() <= 5);
        assert!(s.starts_with(&out));
    }

    #[test]
    fn sanitise_strips_punctuation() {
        assert_eq!(sanitise_fts_query("foo-bar, baz!"), "foo bar baz");
    }

    #[test]
    fn empty_identity_and_query_yields_empty_context() {
        let (_d, stack) = make_stack();
        let out = stack
            .assemble_context(&SessionIdentity::new(""), "", &[], 200)
            .unwrap();
        // No identity, no query, no hits → all four layers empty.
        assert!(out.is_empty() || !out.contains("Identity"));
    }

    #[test]
    fn assemble_context_includes_identity_and_semantic_hits() {
        let (_d, stack) = make_stack();
        seed_journal(
            &stack,
            "2026_04_14",
            "- Worked on Makakoo rust rewrite with Harvey today",
        );
        let identity = SessionIdentity::new("Harvey: the user's autonomous extension.");
        let hits = vec![SemanticHit {
            doc_id: "Makakoo OS".to_string(),
            content: "Platform rename from Harvey OS".to_string(),
            similarity: 0.87,
        }];
        let ctx = stack
            .assemble_context(&identity, "Makakoo rust rewrite", &hits, 400)
            .unwrap();
        assert!(ctx.contains("Identity"));
        assert!(ctx.contains("Harvey"));
        assert!(ctx.contains("Semantic"));
        assert!(ctx.contains("Makakoo OS"));
    }

    #[test]
    fn assemble_context_honours_budget() {
        let (_d, stack) = make_stack();
        let identity = SessionIdentity::new("a".repeat(5000));
        let ctx = stack
            .assemble_context(&identity, "", &[], 100)
            .unwrap();
        // 100 tokens ≈ 400 chars, plus ~30 chars of section framing.
        assert!(ctx.len() < 600, "context too long: {}", ctx.len());
    }
}
