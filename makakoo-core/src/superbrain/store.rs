//! SuperbrainStore — port of `core/superbrain/store.py` to Rust.
//!
//! # Schema mapping
//!
//! The Python source of truth uses a `brain_docs` table whose unique key
//! is `path TEXT UNIQUE NOT NULL` — NOT an integer rowid the caller
//! picks. To keep the public Rust API ergonomic (and to match the
//! task-spec signature `write_document(doc_id: &str, ...)`), we map the
//! caller-supplied `doc_id` onto the `path` column. Every helper here
//! speaks that mapping consistently.
//!
//! Metadata is serialized into the `entities` TEXT column as JSON. When
//! metadata is a JSON array of strings, it goes in verbatim (matching
//! the Python wikilink extraction format `["Harvey", "Makakoo OS"]`).
//! When metadata is any other shape, wikilinks are auto-extracted from
//! the content body so downstream entity-graph rebuilds keep working.
//!
//! # Behaviour locked in from the T1 Python oracle
//!
//! 1. **BM25 sign flip.** FTS5's `bm25()` returns negative scores
//!    (lower = better). Python flips the sign on emission so callers see
//!    higher-is-better scores. We do the same.
//! 2. **Journal recency boost.** After the flip, rows with
//!    `doc_type = 'journal'` receive a multiplicative boost keyed on the
//!    journal's date stem (`YYYY_MM_DD` → `YYYY-MM-DD`). Temporal-query
//!    boosts are stronger than the baseline boost. The multipliers match
//!    `store.py::search()` lines 729–754 verbatim.
//! 3. **Cosine dim guard.** A mismatched length returns `0.0` — the row
//!    is excluded rather than ranked on truncated dimensions.
//! 4. **Vector floor.** Any similarity `<= 0.3` is dropped from
//!    `vector_search` results. (Python uses `> 0.3`; we use strict
//!    greater-than to match.)
//! 5. **LE f32 blobs.** `f32::to_le_bytes()` + `f32::from_le_bytes()` so
//!    `b"\x00\x00\x80\x3f" == 1.0_f32`.
//! 6. **FTS5 tokenizer.** `porter unicode61` (set in `db::SCHEMA_V1`) —
//!    accent-folding ON.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{open_db, run_migrations};
use crate::error::{MakakooError, Result};

// ─────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────

/// A ranked full-text search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    /// The document's stable identifier (maps to the `path` column).
    pub doc_id: String,
    pub content: String,
    pub doc_type: String,
    /// Higher = better. BM25 sign has already been flipped.
    pub score: f32,
    /// Deserialised entities/metadata JSON.
    pub metadata: Value,
}

/// A ranked vector-similarity hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorHit {
    pub doc_id: String,
    pub similarity: f32,
    pub content: String,
}

/// A raw document fetched by id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub doc_id: String,
    pub content: String,
    pub doc_type: String,
    pub created_at: DateTime<Utc>,
    pub metadata: Value,
}

/// Aggregate stats for observability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreStats {
    pub doc_count: usize,
    pub vector_count: usize,
    pub size_bytes: u64,
}

// ─────────────────────────────────────────────────────────────────────
// Vector serialisation — little-endian f32 blobs
// ─────────────────────────────────────────────────────────────────────

/// Pack a slice of `f32` into a little-endian byte vector.
///
/// Matches Python's `struct.pack(f"{n}f", *vec)` byte layout, which is
/// native byte order on every platform we target (x86_64, aarch64) but
/// we enforce little-endian explicitly so cross-architecture dumps
/// round-trip.
pub fn pack_vector(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Unpack a little-endian f32 blob. Non-multiple-of-4 lengths are
/// truncated (matching Python's `len(blob) // 4` behaviour).
pub fn unpack_vector(blob: &[u8]) -> Vec<f32> {
    let n = blob.len() / 4;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let start = i * 4;
        let bytes = [
            blob[start],
            blob[start + 1],
            blob[start + 2],
            blob[start + 3],
        ];
        out.push(f32::from_le_bytes(bytes));
    }
    out
}

/// Cosine similarity between two vectors.
///
/// Dimension-mismatched inputs return `0.0` rather than truncating to
/// the shorter length — see the T1 gotcha list. Zero-norm inputs also
/// return `0.0` (no division-by-zero panic).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot: f32 = 0.0;
    let mut norm_a: f32 = 0.0;
    let mut norm_b: f32 = 0.0;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

// ─────────────────────────────────────────────────────────────────────
// FTS5 query building
// ─────────────────────────────────────────────────────────────────────

/// English stop words filtered out of FTS5 queries. Mirrors
/// `store.py::_STOP_WORDS` exactly.
const STOP_WORDS: &[&str] = &[
    "what", "how", "when", "where", "which", "who", "why", "does", "the", "and", "for", "are",
    "but", "not", "you", "all", "can", "has", "was", "its", "this", "that", "with", "from",
    "about", "into", "also", "been", "have", "will", "did", "our", "your", "their", "there",
    "some", "would", "could", "should", "make", "know", "just", "like", "very", "much", "more",
    "most", "only", "than", "them", "then", "they", "each", "want", "need", "tell", "please",
    "really", "think", "using", "used", "use",
];

fn is_stop(w: &str) -> bool {
    let lower = w.to_ascii_lowercase();
    STOP_WORDS.iter().any(|s| *s == lower)
}

/// Convert a natural-language query into an FTS5 MATCH expression.
/// Direct port of `store.py::_to_fts5_query`.
pub fn to_fts5_query(query: &str) -> String {
    // Strip everything that isn't alphanumeric, whitespace, `-`, `_`, or
    // `"`. Python uses `[^\w\s"-]` which matches `\w = [A-Za-z0-9_]` in
    // the default locale.
    let cleaned: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '-' || c == '_' || c == '"' {
                c
            } else {
                ' '
            }
        })
        .collect();

    let all_words: Vec<&str> = cleaned.split_whitespace().filter(|w| w.len() > 2).collect();
    let mut words: Vec<&str> = all_words.iter().copied().filter(|w| !is_stop(w)).collect();
    if words.is_empty() {
        // Fallback — match Python behaviour of skipping the stop filter.
        words = all_words;
    }
    if words.is_empty() {
        return String::new();
    }

    if words.len() == 1 {
        let w = words[0];
        return format!("\"{w}\" OR {w}*");
    }

    // Multi-word: phrase > NEAR > OR > prefix
    let take = 5.min(words.len());
    let phrase = words[..take].join(" ");
    let near_terms = words[..take]
        .iter()
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let near = format!("NEAR({near_terms}, 10)");
    let or_terms = words
        .iter()
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" OR ");
    let prefix_terms = words
        .iter()
        .map(|w| format!("{w}*"))
        .collect::<Vec<_>>()
        .join(" OR ");

    format!("(\"{phrase}\") OR ({near}) OR ({or_terms}) OR ({prefix_terms})")
}

// ─────────────────────────────────────────────────────────────────────
// Journal recency boost
// ─────────────────────────────────────────────────────────────────────

const TEMPORAL_WORDS: &[&str] = &[
    "today",
    "yesterday",
    "recent",
    "recently",
    "latest",
    "last",
    "week",
    "tonight",
];

fn is_temporal_query(query: &str) -> bool {
    let lowered = query.to_ascii_lowercase();
    lowered.split_whitespace().any(|w| TEMPORAL_WORDS.contains(&w))
}

/// Apply journal recency boost. Mirrors `store.py::search` lines 729–754.
/// `now` is injected so tests can pin the clock.
fn apply_recency_boost(
    score: f32,
    doc_type: &str,
    name: &str,
    query: &str,
    now: DateTime<Utc>,
) -> f32 {
    if doc_type != "journal" {
        return score;
    }
    // Convert `YYYY_MM_DD...` → `YYYY-MM-DD`.
    if name.len() < 10 {
        return score;
    }
    let date_part: String = name[..10].chars().map(|c| if c == '_' { '-' } else { c }).collect();
    let parsed = match NaiveDate::parse_from_str(&date_part, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return score,
    };
    let journal_dt = match Utc.from_local_datetime(&parsed.and_hms_opt(0, 0, 0).unwrap()) {
        chrono::LocalResult::Single(d) => d,
        _ => return score,
    };
    let days_ago = (now - journal_dt).num_days();
    if days_ago < 0 {
        return score; // future-dated journals: leave alone
    }
    let days_ago_f = days_ago as f32;

    if is_temporal_query(query) {
        if days_ago == 0 {
            score * 3.0
        } else if days_ago <= 1 {
            score * 2.5
        } else if days_ago <= 7 {
            score * 2.0
        } else if days_ago <= 30 {
            score * 1.3
        } else {
            score
        }
    } else if days_ago <= 7 {
        score * (1.0 + 0.3 * (1.0 - days_ago_f / 7.0))
    } else if days_ago <= 30 {
        score * (1.0 + 0.1 * (1.0 - days_ago_f / 30.0))
    } else {
        score
    }
}

// ─────────────────────────────────────────────────────────────────────
// SuperbrainStore
// ─────────────────────────────────────────────────────────────────────

/// Canonical SQLite-backed knowledge store: FTS5 full-text search,
/// little-endian f32 vector blobs with brute-force cosine similarity.
///
/// The struct is synchronous. Higher layers that need an async API
/// should wrap calls in `tokio::task::spawn_blocking`.
pub struct SuperbrainStore {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl SuperbrainStore {
    /// Open or create a store at `path`. Runs migrations on every call.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = open_db(path)?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
        })
    }

    /// Return the path the store was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Clone the inner connection handle. Used by subsystems that
    /// construct their own helpers against the same sqlite file
    /// (e.g. SANCHO's MemoryPromoter wrapper).
    pub fn conn_arc(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// Return up to `limit` (doc_id, content) pairs for documents that
    /// have no row in `brain_vectors`. Used by SANCHO's nightly
    /// `superbrain_sync_embed` handler to find orphaned docs and embed
    /// them without re-indexing the whole corpus.
    pub fn docs_missing_vectors(&self, limit: usize) -> Result<Vec<(String, String)>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT d.path, d.content
             FROM brain_docs d
             LEFT JOIN brain_vectors v ON v.doc_id = d.id
             WHERE v.doc_id IS NULL
             ORDER BY d.updated_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let path: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok((path, content))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ───────── write path ─────────

    /// Upsert a document. `doc_id` is stored in the `path` column.
    ///
    /// `metadata` is accepted as an arbitrary JSON value. When it's a
    /// JSON array of strings it is stored verbatim in the `entities`
    /// column (matching Python's wikilink-extraction format). Otherwise
    /// wikilinks are extracted from `content` and used instead, so the
    /// entity-graph rebuild stays consistent.
    pub fn write_document(
        &self,
        doc_id: &str,
        content: &str,
        doc_type: &str,
        metadata: Value,
    ) -> Result<()> {
        let entities = normalize_entities(&metadata, content);
        let entities_json = serde_json::to_string(&entities)?;
        let content_hash = blake3::hash(content.as_bytes()).to_hex().to_string();
        let name = derive_name(doc_id);
        let char_count = content.chars().count() as i64;

        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO brain_docs (path, name, doc_type, content, content_hash, entities, char_count, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
             ON CONFLICT(path) DO UPDATE SET
                 name = excluded.name,
                 doc_type = excluded.doc_type,
                 content = excluded.content,
                 content_hash = excluded.content_hash,
                 entities = excluded.entities,
                 char_count = excluded.char_count,
                 updated_at = excluded.updated_at",
            params![doc_id, name, doc_type, content, content_hash, entities_json, char_count],
        )?;
        Ok(())
    }

    /// Delete a document and its vector (if any).
    pub fn delete_document(&self, doc_id: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        // brain_vectors is keyed on the integer rowid; resolve it first.
        let row_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM brain_docs WHERE path = ?1",
                params![doc_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(id) = row_id {
            conn.execute("DELETE FROM brain_vectors WHERE doc_id = ?1", params![id])?;
        }
        conn.execute("DELETE FROM brain_docs WHERE path = ?1", params![doc_id])?;
        Ok(())
    }

    // ───────── read path ─────────

    /// Fetch a document by id, or `None` if missing.
    pub fn get_document(&self, doc_id: &str) -> Result<Option<Document>> {
        let conn = self.lock_conn()?;
        let row = conn
            .query_row(
                "SELECT path, content, doc_type, updated_at, entities
                 FROM brain_docs WHERE path = ?1",
                params![doc_id],
                row_to_document,
            )
            .optional()?;
        Ok(row)
    }

    /// FTS5 full-text search. Returns up to `limit` hits ordered by
    /// BM25 score (higher = better after the sign flip), with journal
    /// recency boost applied.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let fts_query = to_fts5_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock_conn()?;
        let sql = "SELECT d.path, d.doc_type, d.content, d.entities, d.name,
                          bm25(brain_fts, 5.0, 1.0, 2.0) AS score
                   FROM brain_fts f
                   JOIN brain_docs d ON f.rowid = d.id
                   WHERE brain_fts MATCH ?1
                   ORDER BY score
                   LIMIT ?2";
        let mut stmt = conn.prepare(sql)?;
        let raw_limit = limit.max(1) as i64;
        // Over-fetch slightly so the post-boost re-sort doesn't starve.
        let fetch_limit = (raw_limit * 3).max(raw_limit);
        let rows = stmt.query_map(params![fts_query, fetch_limit], |row| {
            let path: String = row.get(0)?;
            let doc_type: String = row.get(1)?;
            let content: String = row.get(2)?;
            let entities: Option<String> = row.get(3)?;
            let name: String = row.get(4)?;
            let raw_score: f64 = row.get(5)?;
            Ok((path, doc_type, content, entities, name, raw_score))
        })?;

        let now = Utc::now();
        let mut hits: Vec<SearchHit> = Vec::new();
        for row in rows {
            let (path, doc_type, content, entities, name, raw_score) = row?;
            // FTS5 BM25: raw is negative; flip so higher = better.
            let flipped = (-raw_score) as f32;
            let boosted = apply_recency_boost(flipped, &doc_type, &name, query, now);
            let metadata = entities
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or(Value::Array(Vec::new()));
            hits.push(SearchHit {
                doc_id: path,
                content,
                doc_type,
                score: boosted,
                metadata,
            });
        }
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit);
        Ok(hits)
    }

    /// Return the most recently updated documents. Optional `doc_type`
    /// filter restricts the result set.
    pub fn recent(&self, limit: usize, doc_type: Option<&str>) -> Result<Vec<SearchHit>> {
        let conn = self.lock_conn()?;
        let (sql, params_vec): (&str, Vec<String>) = match doc_type {
            Some(dt) => (
                "SELECT path, doc_type, content, entities, 0.0 AS score
                 FROM brain_docs WHERE doc_type = ?1
                 ORDER BY updated_at DESC LIMIT ?2",
                vec![dt.to_string(), limit.to_string()],
            ),
            None => (
                "SELECT path, doc_type, content, entities, 0.0 AS score
                 FROM brain_docs
                 ORDER BY updated_at DESC LIMIT ?1",
                vec![limit.to_string()],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(params_from_iter(params_vec.iter()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_search_hit(row)?);
        }
        Ok(out)
    }

    // ───────── vectors ─────────

    /// Store (or replace) the embedding for `doc_id`. The document must
    /// already exist — otherwise `NotFound` is returned.
    pub fn store_vector(&self, doc_id: &str, vec: &[f32]) -> Result<()> {
        let conn = self.lock_conn()?;
        let row_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM brain_docs WHERE path = ?1",
                params![doc_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let id = row_id.ok_or_else(|| {
            MakakooError::NotFound(format!("brain_docs row for doc_id={doc_id}"))
        })?;
        let blob = pack_vector(vec);
        let dim = vec.len() as i64;
        conn.execute(
            "INSERT INTO brain_vectors (doc_id, embedding, dim, model, created_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(doc_id) DO UPDATE SET
                 embedding = excluded.embedding,
                 dim = excluded.dim,
                 model = excluded.model,
                 created_at = excluded.created_at",
            params![id, blob, dim, "unknown"],
        )?;
        Ok(())
    }

    /// Brute-force cosine similarity search. Any hit below the 0.3
    /// similarity floor is dropped.
    pub fn vector_search(&self, query_vec: &[f32], limit: usize) -> Result<Vec<VectorHit>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT d.path, d.content, v.embedding
             FROM brain_vectors v
             JOIN brain_docs d ON v.doc_id = d.id",
        )?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let content: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((path, content, blob))
        })?;

        let mut scored: Vec<VectorHit> = Vec::new();
        for row in rows {
            let (path, content, blob) = row?;
            let stored = unpack_vector(&blob);
            let sim = cosine_similarity(query_vec, &stored);
            if sim > 0.3 {
                scored.push(VectorHit {
                    doc_id: path,
                    similarity: sim,
                    content,
                });
            }
        }
        scored.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    // ───────── stats ─────────

    /// Number of documents in `brain_docs`.
    pub fn count(&self) -> Result<usize> {
        let conn = self.lock_conn()?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM brain_docs", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Summary stats for observability.
    pub fn stats(&self) -> Result<StoreStats> {
        let conn = self.lock_conn()?;
        let docs: i64 = conn.query_row("SELECT COUNT(*) FROM brain_docs", [], |r| r.get(0))?;
        let vectors: i64 =
            conn.query_row("SELECT COUNT(*) FROM brain_vectors", [], |r| r.get(0))?;
        drop(conn);
        let size_bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        Ok(StoreStats {
            doc_count: docs as usize,
            vector_count: vectors as usize,
            size_bytes,
        })
    }

    // ───────── internal ─────────

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| MakakooError::internal("superbrain store mutex poisoned"))
    }
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

/// Extract the filename stem from a path-like doc_id for the `name`
/// column. Mirrors Python's `file_path.stem`. For non-path doc_ids the
/// trailing segment is used verbatim.
fn derive_name(doc_id: &str) -> String {
    let trimmed = doc_id.trim_end_matches('/');
    let segment = trimmed.rsplit('/').next().unwrap_or(trimmed);
    // Strip a single `.md`/`.txt`/etc. extension.
    match segment.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => segment.to_string(),
    }
}

/// Normalise the caller-supplied metadata into a `Vec<String>` suitable
/// for the `entities` column. If metadata is already `["a", "b"]` we
/// keep it as-is; otherwise we extract `[[wikilinks]]` from `content`.
fn normalize_entities(metadata: &Value, content: &str) -> Vec<String> {
    if let Value::Array(items) = metadata {
        let all_strings = items.iter().all(|v| v.is_string());
        if all_strings {
            return items
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
    }
    extract_wikilinks(content)
}

/// Find `[[Target]]` wikilinks in `content`, deduplicated, in first-seen
/// order. Mirrors `WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")`.
fn extract_wikilinks(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(end) = content[i + 2..].find("]]") {
                let start = i + 2;
                let candidate = &content[start..start + end];
                if !candidate.is_empty() && !candidate.contains(']') && !out.contains(&candidate.to_string())
                {
                    out.push(candidate.to_string());
                }
                i = start + end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn row_to_document(row: &Row<'_>) -> rusqlite::Result<Document> {
    let doc_id: String = row.get(0)?;
    let content: String = row.get(1)?;
    let doc_type: String = row.get(2)?;
    let updated_at: String = row.get(3)?;
    let entities: Option<String> = row.get(4)?;
    let created_at = parse_sqlite_datetime(&updated_at).unwrap_or_else(Utc::now);
    let metadata = entities
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Array(Vec::new()));
    Ok(Document {
        doc_id,
        content,
        doc_type,
        created_at,
        metadata,
    })
}

fn row_to_search_hit(row: &Row<'_>) -> rusqlite::Result<SearchHit> {
    let doc_id: String = row.get(0)?;
    let doc_type: String = row.get(1)?;
    let content: String = row.get(2)?;
    let entities: Option<String> = row.get(3)?;
    let score: f64 = row.get(4)?;
    let metadata = entities
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Array(Vec::new()));
    Ok(SearchHit {
        doc_id,
        content,
        doc_type,
        score: score as f32,
        metadata,
    })
}

fn parse_sqlite_datetime(s: &str) -> Option<DateTime<Utc>> {
    // SQLite `datetime('now')` emits `YYYY-MM-DD HH:MM:SS`.
    let with_t = s.replace(' ', "T");
    let with_z = format!("{with_t}Z");
    DateTime::parse_from_rfc3339(&with_z)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn tmp_store() -> (TempDir, SuperbrainStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("superbrain.db");
        let store = SuperbrainStore::open(&path).unwrap();
        (dir, store)
    }

    #[test]
    fn cosine_dim_mismatch_returns_zero() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![1.0_f32, 0.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_zero_vector_returns_zero() {
        let a = vec![0.0_f32, 0.0, 0.0];
        let b = vec![1.0_f32, 1.0, 1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_orthogonal_and_parallel() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 1e-6);
        let c = vec![2.0_f32, 0.0];
        assert!((cosine_similarity(&a, &c) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn f32_le_round_trip() {
        // b"\x00\x00\x80\x3f" must equal 1.0_f32 in little-endian.
        assert_eq!(unpack_vector(&[0x00, 0x00, 0x80, 0x3f]), vec![1.0_f32]);
        let original = vec![0.0_f32, 1.0, -1.5, 7.125, -42.0];
        let blob = pack_vector(&original);
        assert_eq!(blob.len(), original.len() * 4);
        assert_eq!(unpack_vector(&blob), original);
        // First 4 bytes of the packed 1.0 must be the canonical LE form.
        assert_eq!(&blob[4..8], &[0x00, 0x00, 0x80, 0x3f]);
    }

    #[test]
    fn write_and_get_document() {
        let (_dir, store) = tmp_store();
        store
            .write_document(
                "/brain/pages/Makakoo OS.md",
                "Hello from [[Makakoo OS]] and [[Harvey]]",
                "page",
                json!(["Makakoo OS", "Harvey"]),
            )
            .unwrap();
        let doc = store
            .get_document("/brain/pages/Makakoo OS.md")
            .unwrap()
            .expect("doc exists");
        assert_eq!(doc.doc_type, "page");
        assert!(doc.content.contains("Harvey"));
        let metadata = doc.metadata.as_array().unwrap();
        assert_eq!(metadata.len(), 2);
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn write_document_is_idempotent_upsert() {
        let (_dir, store) = tmp_store();
        store
            .write_document("doc1", "first version", "page", json!([]))
            .unwrap();
        store
            .write_document("doc1", "second version", "page", json!([]))
            .unwrap();
        assert_eq!(store.count().unwrap(), 1);
        let doc = store.get_document("doc1").unwrap().unwrap();
        assert_eq!(doc.content, "second version");
    }

    #[test]
    fn delete_document_removes_row_and_vector() {
        let (_dir, store) = tmp_store();
        store
            .write_document("doc1", "some content body", "page", json!([]))
            .unwrap();
        store.store_vector("doc1", &[0.1, 0.2, 0.3]).unwrap();
        assert_eq!(store.stats().unwrap().vector_count, 1);
        store.delete_document("doc1").unwrap();
        assert_eq!(store.count().unwrap(), 0);
        assert_eq!(store.stats().unwrap().vector_count, 0);
    }

    #[test]
    fn fts5_finds_terms() {
        let (_dir, store) = tmp_store();
        store
            .write_document(
                "pages/polymarket.md",
                "Polymarket arbitrage agent trades on election markets",
                "page",
                json!([]),
            )
            .unwrap();
        store
            .write_document(
                "pages/harvey.md",
                "Harvey is a cognitive extension for the user",
                "page",
                json!([]),
            )
            .unwrap();
        let hits = store.search("polymarket", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "pages/polymarket.md");
        assert!(hits[0].score > 0.0, "BM25 score must be positive after flip");
    }

    #[test]
    fn fts5_accent_folding_porter_unicode61() {
        let (_dir, store) = tmp_store();
        store
            .write_document("pages/cafe.md", "I met her at the café", "page", json!([]))
            .unwrap();
        store
            .write_document(
                "pages/naive.md",
                "The naïve implementation was fast enough",
                "page",
                json!([]),
            )
            .unwrap();
        let cafe_hits = store.search("cafe", 10).unwrap();
        assert!(
            cafe_hits.iter().any(|h| h.doc_id == "pages/cafe.md"),
            "unicode61 must fold `café` → `cafe`"
        );
        let naive_hits = store.search("naive", 10).unwrap();
        assert!(
            naive_hits.iter().any(|h| h.doc_id == "pages/naive.md"),
            "unicode61 must fold `naïve` → `naive`"
        );
    }

    #[test]
    fn vector_search_respects_0_3_floor() {
        let (_dir, store) = tmp_store();
        store
            .write_document("a", "alpha content body one", "page", json!([]))
            .unwrap();
        store
            .write_document("b", "beta content body two", "page", json!([]))
            .unwrap();
        // Near-orthogonal vectors: cosine ≈ 0.14 < 0.3 floor.
        store.store_vector("a", &[1.0, 0.1, 0.0]).unwrap();
        store.store_vector("b", &[1.0, 0.0, 0.0]).unwrap();
        let hits = store.vector_search(&[0.1, 1.0, 0.0], 10).unwrap();
        assert!(hits.is_empty(), "expected floor to drop both hits, got {hits:?}");

        // Add one obviously similar vector.
        store
            .write_document("c", "closely aligned vector", "page", json!([]))
            .unwrap();
        store.store_vector("c", &[0.0, 1.0, 0.0]).unwrap();
        let hits2 = store.vector_search(&[0.1, 1.0, 0.0], 10).unwrap();
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].doc_id, "c");
        assert!(hits2[0].similarity > 0.9);
    }

    #[test]
    fn vector_search_ignores_dim_mismatch_rows() {
        let (_dir, store) = tmp_store();
        store
            .write_document("a", "three-dim vector holder", "page", json!([]))
            .unwrap();
        store
            .write_document("b", "four-dim vector holder", "page", json!([]))
            .unwrap();
        store.store_vector("a", &[1.0, 0.0, 0.0]).unwrap();
        store.store_vector("b", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        // Query dim = 3, so doc b falls out via cosine-returns-0.0.
        let hits = store.vector_search(&[1.0, 0.0, 0.0], 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "a");
    }

    #[test]
    fn journal_recency_boost_lifts_recent_over_old() {
        let now = Utc::now();
        let today = now.format("%Y_%m_%d").to_string();
        // A 45-day-old journal date stem.
        let old_date = (now - chrono::Duration::days(45)).format("%Y_%m_%d").to_string();

        // Same baseline score, journal doc_type, query is temporal-neutral.
        // The recent one gets the standard (1 + 0.3 * (1 - days/7)) boost,
        // the old one falls outside both windows → unchanged.
        let boosted_recent = apply_recency_boost(10.0, "journal", &today, "polymarket", now);
        let boosted_old = apply_recency_boost(10.0, "journal", &old_date, "polymarket", now);
        assert!(
            boosted_recent > boosted_old,
            "recent journal ({boosted_recent}) should rank above old ({boosted_old})"
        );
        // Non-journal stays untouched.
        let page_score = apply_recency_boost(10.0, "page", &today, "polymarket", now);
        assert_eq!(page_score, 10.0);
    }

    #[test]
    fn journal_recency_boost_temporal_triples_today() {
        let now = Utc::now();
        let today = now.format("%Y_%m_%d").to_string();
        let boosted = apply_recency_boost(4.0, "journal", &today, "what happened today", now);
        assert!((boosted - 12.0).abs() < 1e-4, "today + temporal → ×3.0, got {boosted}");
    }

    #[test]
    fn search_orders_recent_journal_above_old_on_ties() {
        let (_dir, store) = tmp_store();
        let now = Utc::now();
        let today = now.format("%Y_%m_%d").to_string();
        let old = (now - chrono::Duration::days(40))
            .format("%Y_%m_%d")
            .to_string();
        // Same content so BM25 scores tie; recency boost breaks the tie.
        store
            .write_document(
                &format!("journals/{today}.md"),
                "polymarket arbitrage polymarket",
                "journal",
                json!([]),
            )
            .unwrap();
        store
            .write_document(
                &format!("journals/{old}.md"),
                "polymarket arbitrage polymarket",
                "journal",
                json!([]),
            )
            .unwrap();
        let hits = store.search("polymarket", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(
            hits[0].doc_id.contains(&today),
            "recent journal must sort first; hits={hits:?}"
        );
    }

    #[test]
    fn recent_returns_newest_first() {
        let (_dir, store) = tmp_store();
        store
            .write_document("a", "first doc", "page", json!([]))
            .unwrap();
        // SQLite `datetime('now')` has 1-second resolution; sleep to
        // guarantee distinct updated_at stamps without racing.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        store
            .write_document("b", "second doc", "page", json!([]))
            .unwrap();
        let hits = store.recent(10, None).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].doc_id, "b");
    }

    #[test]
    fn recent_filters_by_doc_type() {
        let (_dir, store) = tmp_store();
        store
            .write_document("p1", "page content", "page", json!([]))
            .unwrap();
        store
            .write_document("j1", "journal content", "journal", json!([]))
            .unwrap();
        let journal_only = store.recent(10, Some("journal")).unwrap();
        assert_eq!(journal_only.len(), 1);
        assert_eq!(journal_only[0].doc_id, "j1");
    }

    #[test]
    fn store_vector_on_missing_doc_errors() {
        let (_dir, store) = tmp_store();
        let err = store.store_vector("ghost", &[1.0, 2.0]).unwrap_err();
        assert!(matches!(err, MakakooError::NotFound(_)));
    }

    #[test]
    fn to_fts5_query_single_word() {
        let q = to_fts5_query("polymarket");
        assert_eq!(q, "\"polymarket\" OR polymarket*");
    }

    #[test]
    fn to_fts5_query_filters_stopwords_and_builds_compound() {
        let q = to_fts5_query("What is the polymarket arbitrage agent?");
        // Stop-word filter drops What/is/the → phrase starts with polymarket.
        assert!(q.contains("(\"polymarket arbitrage agent\")"), "got: {q}");
        assert!(q.contains("NEAR("), "got: {q}");
        assert!(q.contains("polymarket*"), "got: {q}");
    }

    #[test]
    fn to_fts5_query_empty_when_all_short() {
        let q = to_fts5_query("a an");
        assert_eq!(q, "");
    }

    #[test]
    fn extract_wikilinks_dedupes_and_preserves_order() {
        let links = extract_wikilinks("see [[Harvey]] and [[Makakoo OS]], also [[Harvey]] again");
        assert_eq!(links, vec!["Harvey".to_string(), "Makakoo OS".to_string()]);
    }

    #[test]
    fn normalize_entities_falls_back_to_wikilink_extraction() {
        let md = json!({"not": "an array"});
        let ents = normalize_entities(&md, "hi [[Foo]] [[Bar]]");
        assert_eq!(ents, vec!["Foo".to_string(), "Bar".to_string()]);
    }

    #[test]
    fn stats_reflects_counts_and_size() {
        let (_dir, store) = tmp_store();
        store
            .write_document("a", "alpha", "page", json!([]))
            .unwrap();
        store
            .write_document("b", "beta", "page", json!([]))
            .unwrap();
        store.store_vector("a", &[0.1, 0.2, 0.3]).unwrap();
        let s = store.stats().unwrap();
        assert_eq!(s.doc_count, 2);
        assert_eq!(s.vector_count, 1);
        assert!(s.size_bytes > 0);
    }
}
