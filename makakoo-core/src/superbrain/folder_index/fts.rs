//! FTS5 full-text fallback search, transactional sync, and legacy backfill
//! over `chunks.sqlite` (`chunks_fts`).

use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension};

use crate::error::Result;

use super::{Anchor, AnchorKind, Passage, SearchFilter, SourceKind, TrustLevel};

const META_FTS_VERSION_KEY: &str = "fts_version";
const CURRENT_FTS_VERSION: &str = "1";

const STOP_WORDS: &[&str] = &[
    "what", "how", "when", "where", "which", "who", "why", "does", "the", "and", "for", "are",
    "but", "not", "you", "all", "can", "has", "was", "its", "this", "that", "with", "from",
    "about", "into", "also", "been", "have", "will", "did", "our", "your", "their", "there",
    "some", "would", "could", "should", "make", "know", "just", "like", "very", "much", "more",
    "most", "only", "than", "them", "then", "they", "each", "want", "need", "tell", "please",
    "really", "think", "using", "used", "use",
];

pub(crate) const CHUNKS_FTS_SCHEMA_SQL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    file_path,
    anchor_value,
    text,
    content=chunks,
    content_rowid=id,
    tokenize='porter unicode61'
);
"#;

/// Ensure `chunks_fts` exists and is backfilled for pre-existing `chunks` rows.
pub(crate) fn ensure_chunks_fts_backfilled(conn: &Connection) -> Result<()> {
    conn.execute_batch(CHUNKS_FTS_SCHEMA_SQL)?;
    let ver: Option<String> = conn
        .query_row(
            "SELECT value FROM folder_meta WHERE key = ?1",
            params![META_FTS_VERSION_KEY],
            |r| r.get(0),
        )
        .optional()?;
    if ver.as_deref() == Some(CURRENT_FTS_VERSION) {
        return Ok(());
    }
    conn.execute("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')", [])?;
    conn.execute(
        "INSERT OR REPLACE INTO folder_meta (key, value) VALUES (?1, ?2)",
        params![META_FTS_VERSION_KEY, CURRENT_FTS_VERSION],
    )?;
    Ok(())
}

/// Mirror chunk insertion into `chunks_fts` inside the caller's transaction.
pub(crate) fn insert_chunk_fts_tx(
    conn: &Connection,
    chunk_id: i64,
    file_path: &str,
    anchor_value: &str,
    text: &str,
) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO chunks_fts(rowid, file_path, anchor_value, text)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    stmt.execute(params![chunk_id, file_path, anchor_value, text])?;
    Ok(())
}

/// Mirror chunk deletion for `file_path` into `chunks_fts` BEFORE deleting
/// rows from `chunks` inside the caller's transaction.
pub(crate) fn delete_file_fts_tx(conn: &Connection, file_path: &str) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, file_path, anchor_value, text FROM chunks WHERE file_path = ?1",
    )?;
    let rows: Vec<(i64, String, String, String)> = stmt
        .query_map(params![file_path], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut del = conn.prepare_cached(
        "INSERT INTO chunks_fts(chunks_fts, rowid, file_path, anchor_value, text)
         VALUES ('delete', ?1, ?2, ?3, ?4)",
    )?;
    for (id, fp, av, text) in rows {
        del.execute(params![id, fp, av, text])?;
    }
    Ok(())
}

/// Build an FTS5 `MATCH` expression from arbitrary user text without risking
/// FTS5 operator syntax errors.
pub(crate) fn to_folder_fts5_query(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let all_words: Vec<&str> = cleaned.split_whitespace().collect();
    if all_words.is_empty() {
        return String::new();
    }
    let non_stop: Vec<&str> = all_words
        .iter()
        .copied()
        .filter(|w| w.chars().count() > 2 && !is_stop(w))
        .collect();
    let words = if !non_stop.is_empty() {
        non_stop
    } else {
        let non_trivial: Vec<&str> = all_words.iter().copied().filter(|w| !is_stop(w)).collect();
        if non_trivial.is_empty() {
            all_words
        } else {
            non_trivial
        }
    };
    if words.len() == 1 {
        let w = words[0];
        return format!("\"{w}\" OR {w}*");
    }
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

/// Execute BM25-ranked FTS5 text search over `chunks_fts` with provenance filters.
pub(crate) fn search_chunks_fts(
    conn: &Connection,
    folder_id: &str,
    query: &str,
    k: usize,
    filter: &SearchFilter,
) -> Result<Vec<Passage>> {
    if k == 0 {
        return Ok(Vec::new());
    }
    let fts_query = to_folder_fts5_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    let mut clauses = vec!["chunks_fts MATCH ?"];
    let mut binds: Vec<SqlValue> = vec![SqlValue::from(fts_query)];
    if let Some(sk) = filter.source_kind {
        clauses.push("c.source_kind = ?");
        binds.push(SqlValue::from(sk.as_str().to_string()));
    }
    if let Some(tr) = filter.trust {
        clauses.push("c.trust = ?");
        binds.push(SqlValue::from(tr.as_str().to_string()));
    }
    if let Some(ref owner) = filter.owner {
        clauses.push("c.owner = ?");
        binds.push(SqlValue::from(owner.clone()));
    }
    binds.push(SqlValue::from(k as i64));

    let sql = format!(
        "SELECT c.file_path, c.file_version, c.anchor_kind, c.anchor_value,
                c.char_start, c.char_end, c.text, c.source_kind, c.trust,
                -bm25(chunks_fts, 1.5, 0.5, 2.0) AS score
         FROM chunks_fts f
         JOIN chunks c ON f.rowid = c.id
         WHERE {}
         ORDER BY score DESC
         LIMIT ?",
        clauses.join(" AND ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(binds), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, f64>(9)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (fp, fv, ak, av, cs, ce, text, sk, tr, score) = row?;
        out.push(Passage {
            folder_id: folder_id.to_string(),
            file_path: fp,
            file_version: fv,
            anchor: Anchor {
                kind: AnchorKind::parse(&ak)?,
                value: av,
            },
            char_start: cs as usize,
            char_end: ce as usize,
            text,
            score: (score as f32).max(0.0),
            source_kind: SourceKind::parse(&sk)?,
            trust: TrustLevel::parse(&tr)?,
        });
    }
    Ok(out)
}

fn is_stop(w: &str) -> bool {
    let lower = w.to_ascii_lowercase();
    STOP_WORDS.contains(&lower.as_str())
}
