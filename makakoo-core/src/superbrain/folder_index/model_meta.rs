//! Metadata persistence, alias checks, and search/upsert types for tracking
//! the active embedding model and dimension of a [`super::store::FolderIndex`].

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};

use super::fts::search_chunks_fts;
use super::sqlite_repo::read_chunk_count;
use super::{FolderIndex, Passage, SearchFilter, UpsertFileRequest, UpsertOutcome};

pub(crate) const META_EMBED_MODEL: &str = "embed_model";
pub(crate) const META_EMBED_DIM: &str = "embed_dim";
pub(crate) const META_EMBED_ALIAS: &str = "embed_alias";
pub(crate) const META_SHORT_EMBED_ALIAS: &str = "short_embed_alias";
pub(crate) const META_INDEX_STATE: &str = "index_state";
pub(crate) const META_STALE_REASON: &str = "stale_reason";
pub(crate) const SHORT_BULK_MISMATCH_REASON: &str = "short_bulk_model_mismatch";
const MAX_QUERY_MISMATCH_REASON_BYTES: usize = 256;

/// Lifecycle state of a folder or Brain vector index relative to the configured
/// embedding model and dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexModelState {
    Ready,
    Stale,
    Rebuilding,
}

impl IndexModelState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Stale => "stale",
            Self::Rebuilding => "rebuilding",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "ready" => Ok(Self::Ready),
            "stale" => Ok(Self::Stale),
            "rebuilding" => Ok(Self::Rebuilding),
            other => Err(MakakooError::internal(format!(
                "corrupt index_state: {other:?}"
            ))),
        }
    }
}

/// Which retrieval path produced a folder search response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Vector,
    Fts,
}

/// Snapshot of a folder index's model metadata and freshness state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderModelStatus {
    pub index_state: IndexModelState,
    pub embed_model: Option<String>,
    pub embed_dim: Option<usize>,
    pub embed_alias: Option<String>,
    pub short_embed_alias: Option<String>,
    pub stale_reason: Option<String>,
    pub query_mismatch_reason: Option<String>,
}

/// Model provenance recorded when upserting chunks from an embedding client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertModelMeta {
    pub model: String,
    pub embed_dim: usize,
    pub bulk_alias: String,
    pub short_alias: Option<String>,
}

/// Search request handled by [`super::FolderIndexManager::search_folder`].
#[derive(Debug, Clone)]
pub struct FolderSearchRequest {
    pub folder_id: String,
    pub dim: usize,
    pub k: usize,
    pub query: Option<String>,
    pub query_vector: Option<Vec<f32>>,
    pub query_model: Option<String>,
    pub query_dim: Option<usize>,
    pub filter: SearchFilter,
}

/// Search response returned by [`super::FolderIndexManager::search_folder`].
#[derive(Debug, Clone, Serialize)]
pub struct FolderSearchResponse {
    pub folder_id: String,
    pub passages: Vec<Passage>,
    pub mode: SearchMode,
    pub stale: bool,
    pub rebuilding: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_dim: Option<usize>,
}

pub(crate) fn read_meta_opt(conn: &Connection, key: &str) -> Result<Option<String>> {
    let val: Option<String> = conn
        .query_row(
            "SELECT value FROM folder_meta WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(val.filter(|s| !s.is_empty()))
}

pub(crate) fn write_meta_opt(conn: &Connection, key: &str, value: Option<&str>) -> Result<()> {
    match value.filter(|s| !s.is_empty()) {
        Some(v) => {
            conn.execute(
                "INSERT OR REPLACE INTO folder_meta (key, value) VALUES (?1, ?2)",
                params![key, v],
            )?;
        }
        None => {
            conn.execute("DELETE FROM folder_meta WHERE key = ?1", params![key])?;
        }
    }
    Ok(())
}

pub(crate) fn read_model_status(conn: &Connection) -> Result<FolderModelStatus> {
    let index_state = match read_meta_opt(conn, META_INDEX_STATE)? {
        Some(raw) => IndexModelState::parse(&raw)?,
        None => IndexModelState::Ready,
    };
    let embed_model = read_meta_opt(conn, META_EMBED_MODEL)?;
    let embed_dim = match read_meta_opt(conn, META_EMBED_DIM)? {
        Some(raw) => Some(
            raw.parse::<usize>()
                .map_err(|_| MakakooError::internal(format!("corrupt embed_dim: {raw:?}")))?,
        ),
        None => None,
    };
    let embed_alias = read_meta_opt(conn, META_EMBED_ALIAS)?;
    let short_embed_alias = read_meta_opt(conn, META_SHORT_EMBED_ALIAS)?;
    let stale_reason = read_meta_opt(conn, META_STALE_REASON)?;
    Ok(FolderModelStatus {
        index_state,
        embed_model,
        embed_dim,
        embed_alias,
        short_embed_alias,
        stale_reason,
        query_mismatch_reason: None,
    })
}

pub(crate) fn write_model_state(
    conn: &Connection,
    state: IndexModelState,
    reason: Option<&str>,
) -> Result<()> {
    write_meta_opt(conn, META_INDEX_STATE, Some(state.as_str()))?;
    write_meta_opt(conn, META_STALE_REASON, reason)?;
    Ok(())
}

pub(crate) fn write_ready_model_meta(
    conn: &Connection,
    model: &str,
    embed_dim: usize,
    bulk_alias: &str,
    short_alias: Option<&str>,
) -> Result<()> {
    write_meta_opt(conn, META_EMBED_MODEL, Some(model))?;
    write_meta_opt(conn, META_EMBED_DIM, Some(&embed_dim.to_string()))?;
    write_meta_opt(conn, META_EMBED_ALIAS, Some(bulk_alias))?;
    if let Some(short) = short_alias {
        write_meta_opt(conn, META_SHORT_EMBED_ALIAS, Some(short))?;
    }
    write_model_state(conn, IndexModelState::Ready, None)
}

/// Evaluate model metadata and configured aliases on `open()`.
pub(crate) fn init_model_meta_on_open(
    conn: &Connection,
    chunk_count: u64,
    bulk_alias: Option<&str>,
    short_alias: Option<&str>,
) -> Result<()> {
    let status = read_model_status(conn)?;
    if chunk_count == 0 {
        if status.index_state != IndexModelState::Ready || status.stale_reason.is_some() {
            write_model_state(conn, IndexModelState::Ready, None)?;
        }
        return Ok(());
    }

    // No dimension = written before model metadata existed: rebuild once. A known
    // dimension with no model id is an unnamed index (see apply_upsert_model_meta_tx).
    if status.embed_dim.is_none() {
        write_model_state(
            conn,
            IndexModelState::Stale,
            Some("missing_embed_model_metadata"),
        )?;
        return Ok(());
    }

    if status.index_state == IndexModelState::Rebuilding {
        write_model_state(conn, IndexModelState::Stale, Some("interrupted_rebuild"))?;
    }

    if let (Some(prev), Some(cur)) = (status.embed_alias.as_deref(), bulk_alias) {
        if prev != cur {
            write_model_state(conn, IndexModelState::Stale, Some("embed_alias_changed"))?;
            return Ok(());
        }
    }
    if let (Some(prev), Some(cur)) = (status.short_embed_alias.as_deref(), short_alias) {
        if prev != cur {
            write_model_state(
                conn,
                IndexModelState::Stale,
                Some("short_embed_alias_changed"),
            )?;
            return Ok(());
        }
    }
    if status.index_state == IndexModelState::Ready
        && read_meta_opt(conn, META_INDEX_STATE)?.is_none()
    {
        write_model_state(conn, IndexModelState::Ready, None)?;
    }
    Ok(())
}

pub(crate) fn apply_upsert_model_meta_tx(
    conn: &Connection,
    chunks_len: usize,
    first_vec_len: usize,
    model_meta: Option<&UpsertModelMeta>,
    default_bulk_alias: Option<&str>,
    default_short_alias: Option<&str>,
) -> Result<()> {
    if chunks_len == 0 {
        return Ok(());
    }
    let status = read_model_status(conn)?;
    let total_chunks = read_chunk_count(conn)?;
    let other_chunks_exist = total_chunks > chunks_len as u64;

    let Some(m) = model_meta else {
        // The caller sent vectors without naming their model. Never record an alias
        // as a model id: a later query would see the real id, call it a switch and
        // rebuild, and the next unnamed upsert would flip it back (endless re-embed).
        if other_chunks_exist || status.embed_model.is_some() {
            // Same index, same dimension: the rows come from the same bulk alias.
            if status.embed_dim != Some(first_vec_len)
                && status.index_state != IndexModelState::Rebuilding
            {
                write_model_state(conn, IndexModelState::Stale, Some("mixed_model_upsert"))?;
            }
            return Ok(());
        }
        // First rows of a fresh index and no model: record what is known, leave the
        // model unknown (searches compare the dimension only) and let the next bulk
        // probe name it. Indexes without an embedder keep working.
        write_meta_opt(conn, META_EMBED_MODEL, None)?;
        write_meta_opt(conn, META_EMBED_DIM, Some(&first_vec_len.to_string()))?;
        let alias = default_bulk_alias.unwrap_or(crate::embeddings::DEFAULT_EMBED_MODEL);
        write_meta_opt(conn, META_EMBED_ALIAS, Some(alias))?;
        if let Some(short) = default_short_alias {
            write_meta_opt(conn, META_SHORT_EMBED_ALIAS, Some(short))?;
        }
        write_model_state(conn, IndexModelState::Ready, None)?;
        return Ok(());
    };

    if !other_chunks_exist {
        if status.stale_reason.as_deref() == Some(SHORT_BULK_MISMATCH_REASON) {
            write_meta_opt(conn, META_EMBED_MODEL, Some(&m.model))?;
            write_meta_opt(conn, META_EMBED_DIM, Some(&m.embed_dim.to_string()))?;
            write_meta_opt(conn, META_EMBED_ALIAS, Some(&m.bulk_alias))?;
            return Ok(());
        }
        write_ready_model_meta(
            conn,
            &m.model,
            m.embed_dim,
            &m.bulk_alias,
            m.short_alias.as_deref(),
        )?;
        return Ok(());
    }

    let model_matches = status.embed_model.as_deref() == Some(m.model.as_str());
    let dim_matches = status.embed_dim == Some(m.embed_dim);
    // A running rebuild owns the state; its tail pass re-embeds rows written meanwhile.
    if (!model_matches || !dim_matches) && status.index_state != IndexModelState::Rebuilding {
        write_model_state(conn, IndexModelState::Stale, Some("mixed_model_upsert"))?;
    }
    Ok(())
}

impl FolderIndex {
    pub fn model_status(&self) -> Result<FolderModelStatus> {
        let conn = self.lock_conn()?;
        let mut status = read_model_status(&conn)?;
        drop(conn);
        status.query_mismatch_reason = self
            .query_mismatch_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Ok(status)
    }

    pub fn chunk_count(&self) -> Result<u64> {
        let conn = self.lock_conn()?;
        read_chunk_count(&conn)
    }

    pub fn mark_stale(&self, reason: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        write_model_state(&conn, IndexModelState::Stale, Some(reason))
    }

    /// Probe-time stale mark: never overwrites `rebuilding`. Read and write happen
    /// under one connection lock, the same lock the rebuild takes to set
    /// `rebuilding`, so a probe racing a rebuild start cannot hide it.
    pub(crate) fn mark_stale_unless_rebuilding(&self, reason: &str) -> Result<bool> {
        let conn = self.lock_conn()?;
        if read_model_status(&conn)?.index_state == IndexModelState::Rebuilding {
            return Ok(false);
        }
        write_model_state(&conn, IndexModelState::Stale, Some(reason))?;
        Ok(true)
    }

    /// Keep a query-side model mismatch separately from index freshness.
    /// Query metadata describes the caller's vector, not the model that wrote
    /// the index, so it must never make the index eligible for a rebuild or
    /// create SQLite write pressure. Bound the client-controlled diagnostic.
    pub(crate) fn record_query_mismatch(&self, reason: &str) {
        let bounded = truncate_utf8(reason, MAX_QUERY_MISMATCH_REASON_BYTES);
        *self
            .query_mismatch_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(bounded);
    }

    pub(crate) fn clear_query_mismatch(&self) {
        *self
            .query_mismatch_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn search_fts(&self, query: &str, k: usize, filter: &SearchFilter) -> Result<Vec<Passage>> {
        let conn = self.lock_conn()?;
        search_chunks_fts(&conn, &self.folder_id, query, k, filter)
    }

    pub fn upsert_file_with_model(
        &self,
        req: &UpsertFileRequest,
        model_meta: Option<&UpsertModelMeta>,
    ) -> Result<UpsertOutcome> {
        self.upsert_file_inner(req, model_meta)
    }

    pub(crate) fn record_upsert_model_meta_tx(
        &self,
        conn: &Connection,
        chunks_len: usize,
        first_vec_len: usize,
        model_meta: Option<&UpsertModelMeta>,
    ) -> Result<()> {
        apply_upsert_model_meta_tx(
            conn,
            chunks_len,
            first_vec_len,
            model_meta,
            self.default_bulk_alias.as_deref(),
            self.default_short_alias.as_deref(),
        )
    }

    pub(crate) fn update_sidecar_if_ready(&self) {
        if let Ok(status) = self.model_status() {
            if status.index_state == IndexModelState::Ready {
                self.vectors
                    .set_model_meta(status.embed_model, status.embed_dim);
            }
        }
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}
