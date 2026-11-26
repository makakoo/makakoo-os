//! SQLite schema, metadata, chunk/state persistence helpers, and compact
//! chunk-ID filter for [`super::store::FolderIndex`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use crate::error::{MakakooError, Result};

use super::fts::{delete_file_fts_tx, ensure_chunks_fts_backfilled, insert_chunk_fts_tx};
use super::quantize::i8_slice_to_bytes;
use super::{
    Anchor, AnchorKind, ChunkInput, FileStateRecord, FileStatus, SearchFilter, SkipReason,
    SourceKind, TrustLevel,
};

pub(crate) const DB_FILE_NAME: &str = "chunks.sqlite";
const BITSET_MAX_ID: u64 = 2_000_000;

pub(crate) const FOLDER_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS folder_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS file_states (
    file_path TEXT PRIMARY KEY,
    file_version TEXT NOT NULL,
    state TEXT NOT NULL,
    done INTEGER,
    total INTEGER,
    skip_reason TEXT,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path TEXT NOT NULL,
    file_version TEXT NOT NULL,
    anchor_kind TEXT NOT NULL,
    anchor_value TEXT NOT NULL,
    char_start INTEGER NOT NULL,
    char_end INTEGER NOT NULL,
    text TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    trust TEXT NOT NULL,
    owner TEXT NOT NULL,
    vector_i8 BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_chunks_file_path ON chunks(file_path);
CREATE INDEX IF NOT EXISTS idx_chunks_provenance ON chunks(source_kind, trust, owner);

-- Durable parking for resumable, owner-fenced model rebuilds.  Rows are
-- scoped to a stable claim token and the probed model pair; a replacement
-- process can resume a stale claim without trusting another model's work.
CREATE TABLE IF NOT EXISTS rebuild_vectors (
    claim_token TEXT NOT NULL,
    model TEXT NOT NULL,
    dim INTEGER NOT NULL,
    chunk_id INTEGER NOT NULL,
    text TEXT NOT NULL,
    vector_i8 BLOB NOT NULL,
    PRIMARY KEY (claim_token, chunk_id)
);

CREATE INDEX IF NOT EXISTS idx_rebuild_vectors_claim
    ON rebuild_vectors(claim_token, model, dim);
"#;

/// Compact O(1) membership predicate passed into `usearch::Index::filtered_search`.
/// Uses a dense bitset (25 KB at 200k chunks) up to 2M IDs, falling back to a
/// HashSet only for sparse high IDs.
pub(crate) enum ChunkIdFilter {
    Bitset { words: Vec<u64>, max_id: u64 },
    Set(HashSet<u64>),
}

impl ChunkIdFilter {
    pub(crate) fn from_ids(ids: &[u64]) -> Self {
        let max_id = ids.iter().copied().max().unwrap_or(0);
        if max_id <= BITSET_MAX_ID {
            let mut words = vec![0u64; (max_id as usize / 64) + 1];
            for &id in ids {
                words[(id as usize) / 64] |= 1u64 << ((id as usize) % 64);
            }
            Self::Bitset { words, max_id }
        } else {
            Self::Set(ids.iter().copied().collect())
        }
    }

    #[inline]
    pub(crate) fn contains(&self, key: u64) -> bool {
        match self {
            Self::Bitset { words, max_id } => {
                key <= *max_id
                    && (words[(key as usize) / 64] & (1u64 << ((key as usize) % 64))) != 0
            }
            Self::Set(set) => set.contains(&key),
        }
    }
}

pub(crate) struct HydratedChunkRow {
    pub file_path: String,
    pub file_version: String,
    pub anchor: Anchor,
    pub char_start: usize,
    pub char_end: usize,
    pub text: String,
    pub source_kind: SourceKind,
    pub trust: TrustLevel,
    pub owner: String,
}

pub(crate) fn open_folder_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    for pragma in [
        "PRAGMA busy_timeout = 5000",
        "PRAGMA journal_mode = WAL",
        "PRAGMA synchronous = NORMAL",
        "PRAGMA cache_size = -8000",
        "PRAGMA temp_store = FILE",
    ] {
        let mut s = conn.prepare(pragma)?;
        let mut rows = s.query([])?;
        while rows.next()?.is_some() {}
    }
    conn.execute_batch(FOLDER_SCHEMA_SQL)?;
    ensure_chunks_fts_backfilled(&conn)?;
    Ok(conn)
}

pub(crate) fn init_and_verify_meta(
    conn: &Connection,
    folder_id: &str,
    dim: usize,
) -> Result<(u64, u64)> {
    let existing_dim: Option<String> = conn
        .query_row("SELECT value FROM folder_meta WHERE key = 'dim'", [], |r| {
            r.get(0)
        })
        .optional()?;

    match existing_dim {
        Some(raw) => {
            let stored: usize = raw
                .parse()
                .map_err(|_| MakakooError::internal(format!("corrupt dim meta: {raw:?}")))?;
            if stored != dim {
                return Err(MakakooError::invalid_input(format!(
                    "folder {folder_id:?} was created with dim {stored}, cannot open with dim {dim}"
                )));
            }
        }
        None => {
            conn.execute(
                "INSERT OR REPLACE INTO folder_meta (key, value) VALUES ('folder_id', ?1)",
                params![folder_id],
            )?;
            conn.execute(
                "INSERT OR REPLACE INTO folder_meta (key, value) VALUES ('dim', ?1)",
                params![dim.to_string()],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO folder_meta (key, value) VALUES ('generation', '0')",
                [],
            )?;
        }
    }

    Ok((read_generation(conn)?, read_chunk_count(conn)?))
}

pub(crate) fn read_generation(conn: &Connection) -> Result<u64> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM folder_meta WHERE key = 'generation'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match raw {
        Some(s) => s
            .parse::<u64>()
            .map_err(|_| MakakooError::internal(format!("corrupt generation meta: {s:?}"))),
        None => Ok(0),
    }
}

pub(crate) fn bump_generation_tx(conn: &Connection) -> Result<u64> {
    let next = read_generation(conn)?.saturating_add(1);
    conn.execute(
        "INSERT OR REPLACE INTO folder_meta (key, value) VALUES ('generation', ?1)",
        params![next.to_string()],
    )?;
    Ok(next)
}

pub(crate) fn read_chunk_count(conn: &Connection) -> Result<u64> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    Ok(count.max(0) as u64)
}

pub(crate) fn collect_file_chunk_ids(conn: &Connection, file_path: &str) -> Result<Vec<u64>> {
    let mut stmt = conn.prepare_cached("SELECT id FROM chunks WHERE file_path = ?1")?;
    let rows = stmt.query_map(params![file_path], |r| r.get::<_, i64>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row? as u64);
    }
    Ok(ids)
}

pub(crate) fn delete_file_chunks_tx(conn: &Connection, file_path: &str) -> Result<usize> {
    delete_file_fts_tx(conn, file_path)?;
    Ok(conn.execute(
        "DELETE FROM chunks WHERE file_path = ?1",
        params![file_path],
    )?)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_chunks_tx(
    conn: &Connection,
    file_path: &str,
    file_version: &str,
    source_kind: SourceKind,
    trust: TrustLevel,
    owner: &str,
    chunks: &[ChunkInput],
    quantized: Vec<Vec<i8>>,
) -> Result<Vec<(u64, Vec<i8>)>> {
    let mut additions = Vec::with_capacity(chunks.len());
    let mut stmt = conn.prepare_cached(
        "INSERT INTO chunks (
            file_path, file_version, anchor_kind, anchor_value,
            char_start, char_end, text, source_kind, trust, owner, vector_i8
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for (chunk, vec_i8) in chunks.iter().zip(quantized) {
        stmt.execute(params![
            file_path,
            file_version,
            chunk.anchor.kind.as_str(),
            &chunk.anchor.value,
            chunk.char_start as i64,
            chunk.char_end as i64,
            &chunk.text,
            source_kind.as_str(),
            trust.as_str(),
            owner,
            i8_slice_to_bytes(&vec_i8),
        ])?;
        let rowid = conn.last_insert_rowid();
        insert_chunk_fts_tx(conn, rowid, file_path, &chunk.anchor.value, &chunk.text)?;
        additions.push((rowid as u64, vec_i8));
    }
    Ok(additions)
}

pub(crate) fn upsert_file_state_tx(
    conn: &Connection,
    file_path: &str,
    file_version: &str,
    state: &str,
    done: Option<u64>,
    total: Option<u64>,
    skip_reason: Option<&str>,
) -> Result<()> {
    let now = Utc::now().timestamp();
    conn.execute(
        "INSERT INTO file_states (file_path, file_version, state, done, total, skip_reason, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(file_path) DO UPDATE SET
            file_version = excluded.file_version,
            state = excluded.state,
            done = excluded.done,
            total = excluded.total,
            skip_reason = excluded.skip_reason,
            updated_at = excluded.updated_at",
        params![
            file_path,
            file_version,
            state,
            done.map(|v| v as i64),
            total.map(|v| v as i64),
            skip_reason,
            now,
        ],
    )?;
    Ok(())
}

pub(crate) fn query_filtered_chunk_ids(
    conn: &Connection,
    filter: &SearchFilter,
) -> Result<Vec<u64>> {
    let mut clauses: Vec<&'static str> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    if let Some(sk) = filter.source_kind {
        clauses.push("source_kind = ?");
        values.push(sk.as_str().to_string());
    }
    if let Some(trust) = filter.trust {
        clauses.push("trust = ?");
        values.push(trust.as_str().to_string());
    }
    if let Some(ref owner) = filter.owner {
        clauses.push("owner = ?");
        values.push(owner.clone());
    }
    let sql = format!("SELECT id FROM chunks WHERE {}", clauses.join(" AND "));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values.iter()), |r| r.get::<_, i64>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row? as u64);
    }
    Ok(ids)
}

pub(crate) fn fetch_chunk_row(
    conn: &Connection,
    chunk_id: u64,
) -> Result<Option<HydratedChunkRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT file_path, file_version, anchor_kind, anchor_value,
                char_start, char_end, text, source_kind, trust, owner
         FROM chunks WHERE id = ?1",
    )?;
    let row_opt = stmt
        .query_row(params![chunk_id as i64], |r| {
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
                r.get::<_, String>(9)?,
            ))
        })
        .optional()?;
    let Some((fp, fv, ak, av, cs, ce, text, sk, tr, owner)) = row_opt else {
        return Ok(None);
    };
    Ok(Some(HydratedChunkRow {
        file_path: fp,
        file_version: fv,
        anchor: Anchor {
            kind: AnchorKind::parse(&ak)?,
            value: av,
        },
        char_start: cs as usize,
        char_end: ce as usize,
        text,
        source_kind: SourceKind::parse(&sk)?,
        trust: TrustLevel::parse(&tr)?,
        owner,
    }))
}

pub(crate) fn decode_file_state(
    file_path: String,
    file_version: String,
    state: &str,
    done: Option<i64>,
    total: Option<i64>,
    skip_reason: Option<&str>,
) -> Result<FileStateRecord> {
    let status = match state {
        "indexed" => FileStatus::Indexed,
        "indexing" => FileStatus::Indexing {
            done: done.unwrap_or(0).max(0) as u64,
            total: total.unwrap_or(0).max(0) as u64,
        },
        "skipped" => {
            let reason = skip_reason.ok_or_else(|| {
                MakakooError::internal(format!("missing skip_reason for {file_path}"))
            })?;
            FileStatus::Skipped {
                reason: SkipReason::parse(reason)?,
            }
        }
        other => {
            return Err(MakakooError::internal(format!(
                "corrupt file state {other:?} for {file_path}"
            )));
        }
    };
    Ok(FileStateRecord {
        file_path,
        file_version,
        status,
    })
}

pub(crate) fn list_file_states(conn: &Connection) -> Result<Vec<FileStateRecord>> {
    let mut stmt = conn.prepare(
        "SELECT file_path, file_version, state, done, total, skip_reason
         FROM file_states ORDER BY file_path ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (fp, fv, st, done, total, reason) = row?;
        out.push(decode_file_state(
            fp,
            fv,
            &st,
            done,
            total,
            reason.as_deref(),
        )?);
    }
    Ok(out)
}

pub(crate) fn get_file_state(
    conn: &Connection,
    file_path: &str,
) -> Result<Option<FileStateRecord>> {
    let row = conn
        .query_row(
            "SELECT file_path, file_version, state, done, total, skip_reason
             FROM file_states WHERE file_path = ?1",
            params![file_path],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;
    match row {
        Some((fp, fv, st, done, total, reason)) => Ok(Some(decode_file_state(
            fp,
            fv,
            &st,
            done,
            total,
            reason.as_deref(),
        )?)),
        None => Ok(None),
    }
}

pub(crate) fn sqlite_files_bytes(db_path: &Path) -> u64 {
    let mut total = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let base = db_path.as_os_str().to_string_lossy();
    for suffix in ["-wal", "-shm"] {
        let p = PathBuf::from(format!("{base}{suffix}"));
        total = total.saturating_add(std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0));
    }
    total
}

pub(crate) fn clear_file_chunks_and_upsert_state(
    mut conn: std::sync::MutexGuard<'_, Connection>,
    file_path: &str,
    file_version: &str,
    state: &str,
    done: Option<u64>,
    total: Option<u64>,
    skip_reason: Option<&str>,
) -> Result<(Vec<u64>, Option<u64>)> {
    let tx = conn.transaction()?;
    let old_ids = collect_file_chunk_ids(&tx, file_path)?;
    if !old_ids.is_empty() {
        delete_file_chunks_tx(&tx, file_path)?;
    }
    upsert_file_state_tx(
        &tx,
        file_path,
        file_version,
        state,
        done,
        total,
        skip_reason,
    )?;
    let new_gen = if !old_ids.is_empty() {
        Some(bump_generation_tx(&tx)?)
    } else {
        None
    };
    tx.commit()?;
    Ok((old_ids, new_gen))
}
