//! Metadata persistence, alias checks, and search/upsert types for tracking
//! the active embedding model and dimension of a [`super::store::FolderIndex`].

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};

use super::fts::search_chunks_fts;
use super::sqlite_repo::{read_chunk_count, read_generation};
use super::{FolderIndex, Passage, SearchFilter, UpsertFileRequest, UpsertOutcome};

pub(crate) const META_EMBED_MODEL: &str = "embed_model";
pub(crate) const META_EMBED_DIM: &str = "embed_dim";
pub(crate) const META_EMBED_ALIAS: &str = "embed_alias";
pub(crate) const META_SHORT_EMBED_ALIAS: &str = "short_embed_alias";
pub(crate) const META_INDEX_STATE: &str = "index_state";
pub(crate) const META_STALE_REASON: &str = "stale_reason";
pub(crate) const OBSERVED_BULK_UPSERT_REASON: &str = "observed_bulk_model_upsert";
pub(crate) const SHORT_BULK_MISMATCH_REASON: &str = "short_bulk_model_mismatch";
pub(crate) const UNPROVEN_VECTOR_UPSERT_REASON: &str = "unproven_vector_upsert";
const META_OBSERVED_BULK: &str = "observed_bulk";
const META_REBUILD_RETRY: &str = "rebuild_retry";
const META_AUTOMATIC_REBUILD_SUCCESS: &str = "automatic_rebuild_success";
const META_UNPROVEN_VECTORS_SINCE_GEN: &str = "unproven_vectors_since_gen";
pub(crate) const AUTOMATIC_REBUILD_SUCCESS_WINDOW_MS: u64 = 60 * 60 * 1_000;
const MAX_QUERY_MISMATCH_REASON_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ObservedBulkModel {
    #[serde(default)]
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) dim: usize,
    #[serde(default)]
    pub(crate) consecutive: u32,
    #[serde(default)]
    pub(crate) first_seen_unix_ms: u64,
    #[serde(default)]
    pub(crate) last_seen_unix_ms: u64,
    #[serde(default)]
    pub(crate) probe_expedited: bool,
}

impl ObservedBulkModel {
    pub(crate) fn is_confirmed(&self, min_elapsed_ms: u64) -> bool {
        self.consecutive >= 2
            && self.first_seen_unix_ms > 0
            && self
                .last_seen_unix_ms
                .saturating_sub(self.first_seen_unix_ms)
                >= min_elapsed_ms
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PersistedRebuildRetry {
    pub(crate) model: String,
    pub(crate) dim: usize,
    pub(crate) error_class: String,
    pub(crate) failures: u32,
    pub(crate) halted: bool,
    pub(crate) not_before_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedAutomaticRebuildSuccess {
    #[serde(default)]
    pub(crate) last_success_unix_ms: u64,
}

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
    pub unproven_vectors_since_gen: Option<u64>,
    pub rebuild_deferred_until: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadyPublicationKind {
    Ordinary,
    RebuildCommit,
}

/// Complete compare-and-set input for the single model-metadata publication
/// door. Callers must hold an IMMEDIATE transaction. The door re-reads every
/// safety input instead of trusting an earlier snapshot.
pub(crate) struct ReadyPublication<'a> {
    pub(crate) expected_generation: u64,
    pub(crate) expected_state: IndexModelState,
    pub(crate) expected_reason: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) embed_dim: Option<usize>,
    pub(crate) bulk_alias: Option<&'a str>,
    pub(crate) short_alias: Option<&'a str>,
    pub(crate) final_stale_reason: Option<&'a str>,
    pub(crate) kind: ReadyPublicationKind,
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
    let unproven_vectors_since_gen = read_unproven_vectors_since_gen(conn)?;
    let now_unix_ms = unix_time_ms();
    let rebuild_deferred_until =
        read_persisted_automatic_rebuild_success(conn)?.and_then(|success| {
            let deferred_until = success
                .last_success_unix_ms
                .saturating_add(AUTOMATIC_REBUILD_SUCCESS_WINDOW_MS);
            (success.last_success_unix_ms <= now_unix_ms && now_unix_ms < deferred_until)
                .then_some(deferred_until)
        });
    Ok(FolderModelStatus {
        index_state,
        embed_model,
        embed_dim,
        embed_alias,
        short_embed_alias,
        stale_reason,
        query_mismatch_reason: None,
        unproven_vectors_since_gen,
        rebuild_deferred_until,
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

pub(crate) fn read_unproven_vectors_since_gen(conn: &Connection) -> Result<Option<u64>> {
    let Some(raw) = read_meta_opt(conn, META_UNPROVEN_VECTORS_SINCE_GEN)? else {
        return Ok(None);
    };
    // Fail closed. Corrupt provenance metadata can never authorize Ready.
    Ok(Some(raw.parse::<u64>().unwrap_or(0)))
}

fn mark_unproven_vectors_tx(conn: &Connection) -> Result<()> {
    let next_generation = read_generation(conn)?.saturating_add(1);
    write_meta_opt(
        conn,
        META_UNPROVEN_VECTORS_SINCE_GEN,
        Some(&next_generation.to_string()),
    )
}

/// Single door for publishing model metadata and, when safe, `Ready`.
///
/// Every caller runs this inside the same IMMEDIATE transaction as the work
/// that justifies publication. The generation/state/reason/provenance checks
/// are deliberately repeated here to close cross-process TOCTOU windows.
pub(crate) fn publish_ready_model_meta_tx(
    conn: &Connection,
    publication: &ReadyPublication<'_>,
) -> Result<bool> {
    let status = read_model_status(conn)?;
    if read_generation(conn)? != publication.expected_generation
        || status.index_state != publication.expected_state
        || status.stale_reason.as_deref() != publication.expected_reason
    {
        return Ok(false);
    }

    let unproven = read_unproven_vectors_since_gen(conn)?;
    if unproven.is_some() && publication.kind != ReadyPublicationKind::RebuildCommit {
        return Ok(false);
    }

    if publication.kind == ReadyPublicationKind::RebuildCommit {
        // Coverage and generation were checked by the rebuild commit before it
        // entered this door. This is the only provenance-flag clear in code.
        write_meta_opt(conn, META_UNPROVEN_VECTORS_SINCE_GEN, None)?;
    }
    write_meta_opt(conn, META_EMBED_MODEL, publication.model)?;
    write_meta_opt(
        conn,
        META_EMBED_DIM,
        publication.embed_dim.map(|dim| dim.to_string()).as_deref(),
    )?;
    write_meta_opt(conn, META_EMBED_ALIAS, publication.bulk_alias)?;
    write_meta_opt(conn, META_SHORT_EMBED_ALIAS, publication.short_alias)?;
    match publication.final_stale_reason {
        Some(reason) => write_model_state(conn, IndexModelState::Stale, Some(reason))?,
        None => write_model_state(conn, IndexModelState::Ready, None)?,
    }
    Ok(true)
}

pub(crate) fn read_observed_bulk(conn: &Connection) -> Result<Option<ObservedBulkModel>> {
    let Some(raw) = read_meta_opt(conn, META_OBSERVED_BULK)? else {
        return Ok(None);
    };
    // A manual edit or a future schema must not wedge every later probe/upsert.
    // The next successful observation replaces an unreadable value.
    Ok(serde_json::from_str(&raw).ok())
}

pub(crate) fn record_observed_bulk(
    conn: &mut Connection,
    model: &str,
    dim: usize,
    now_unix_ms: u64,
) -> Result<ObservedBulkModel> {
    // IMMEDIATE makes the cross-process read/modify/write one serialization
    // point. Two probes cannot both read the same counter then lose a sighting.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous = read_observed_bulk(&tx)?;
    let observed = match previous {
        Some(previous)
            if previous.model == model
                && previous.dim == dim
                && previous.first_seen_unix_ms > 0 =>
        {
            ObservedBulkModel {
                model: model.to_string(),
                dim,
                consecutive: previous.consecutive.saturating_add(1),
                first_seen_unix_ms: previous.first_seen_unix_ms,
                last_seen_unix_ms: now_unix_ms,
                probe_expedited: previous.probe_expedited,
            }
        }
        _ => ObservedBulkModel {
            model: model.to_string(),
            dim,
            consecutive: 1,
            first_seen_unix_ms: now_unix_ms,
            last_seen_unix_ms: now_unix_ms,
            probe_expedited: false,
        },
    };
    write_meta_opt(
        &tx,
        META_OBSERVED_BULK,
        Some(&serde_json::to_string(&observed)?),
    )?;
    tx.commit()?;
    Ok(observed)
}

/// Atomically claim the one expedited probe allowed for an observed pair.
/// Returns false for stored/foreign pairs, corrupt metadata, or a pair whose
/// expedition was already claimed by this or another process.
pub(crate) fn claim_observed_bulk_probe_expedite(
    conn: &mut Connection,
    model: &str,
    dim: usize,
) -> Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(mut observed) = read_observed_bulk(&tx)? else {
        return Ok(false);
    };
    if observed.model != model || observed.dim != dim || observed.probe_expedited {
        return Ok(false);
    }
    observed.probe_expedited = true;
    write_meta_opt(
        &tx,
        META_OBSERVED_BULK,
        Some(&serde_json::to_string(&observed)?),
    )?;
    tx.commit()?;
    Ok(true)
}

pub(crate) fn read_persisted_rebuild_retry(
    conn: &Connection,
) -> Result<Option<PersistedRebuildRetry>> {
    let Some(raw) = read_meta_opt(conn, META_REBUILD_RETRY)? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&raw)?))
}

pub(crate) fn write_persisted_rebuild_retry(
    conn: &Connection,
    retry: &PersistedRebuildRetry,
) -> Result<()> {
    write_meta_opt(
        conn,
        META_REBUILD_RETRY,
        Some(&serde_json::to_string(retry)?),
    )
}

pub(crate) fn clear_persisted_rebuild_retry(conn: &Connection) -> Result<()> {
    write_meta_opt(conn, META_REBUILD_RETRY, None)
}

pub(crate) fn read_persisted_automatic_rebuild_success(
    conn: &Connection,
) -> Result<Option<PersistedAutomaticRebuildSuccess>> {
    let Some(raw) = read_meta_opt(conn, META_AUTOMATIC_REBUILD_SUCCESS)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&raw).ok())
}

pub(crate) fn write_persisted_automatic_rebuild_success(
    conn: &Connection,
    success: &PersistedAutomaticRebuildSuccess,
) -> Result<()> {
    write_meta_opt(
        conn,
        META_AUTOMATIC_REBUILD_SUCCESS,
        Some(&serde_json::to_string(success)?),
    )
}

pub(crate) fn is_metadata_only_stale_reason(reason: &str) -> bool {
    matches!(
        reason,
        "embed_alias_changed"
            | "short_embed_alias_changed"
            | "bulk_model_change_pending"
            | SHORT_BULK_MISMATCH_REASON
    ) || reason.starts_with("model_mismatch:")
        || reason.starts_with("dim_mismatch:")
}

fn write_stale_preserving_provenance_reason(conn: &Connection, reason: &str) -> Result<()> {
    let status = read_model_status(conn)?;
    let reason = if is_metadata_only_stale_reason(reason)
        && status
            .stale_reason
            .as_deref()
            .is_some_and(|current| !is_metadata_only_stale_reason(current))
    {
        status.stale_reason.as_deref().unwrap_or(reason)
    } else {
        reason
    };
    write_model_state(conn, IndexModelState::Stale, Some(reason))
}

/// Evaluate model metadata and configured aliases on `open()`.
pub(crate) fn init_model_meta_on_open(
    conn: &mut Connection,
    chunk_count: u64,
    bulk_alias: Option<&str>,
    short_alias: Option<&str>,
) -> Result<()> {
    let status = read_model_status(conn)?;
    if chunk_count == 0 {
        if status.index_state != IndexModelState::Ready || status.stale_reason.is_some() {
            let expected_generation = read_generation(conn)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let _ = publish_ready_model_meta_tx(
                &tx,
                &ReadyPublication {
                    expected_generation,
                    expected_state: status.index_state,
                    expected_reason: status.stale_reason.as_deref(),
                    model: status.embed_model.as_deref(),
                    embed_dim: status.embed_dim,
                    bulk_alias: status.embed_alias.as_deref().or(bulk_alias),
                    short_alias: status.short_embed_alias.as_deref().or(short_alias),
                    final_stale_reason: None,
                    kind: ReadyPublicationKind::Ordinary,
                },
            )?;
            tx.commit()?;
        }
        return Ok(());
    }

    // No dimension = written before model metadata existed: rebuild once. A known
    // dimension with no model id is an unnamed index (see apply_upsert_model_meta_tx).
    if status.embed_dim.is_none() {
        write_stale_preserving_provenance_reason(conn, "missing_embed_model_metadata")?;
        return Ok(());
    }

    if status.index_state == IndexModelState::Rebuilding {
        write_model_state(conn, IndexModelState::Stale, Some("interrupted_rebuild"))?;
    }

    if let (Some(prev), Some(cur)) = (status.embed_alias.as_deref(), bulk_alias) {
        if prev != cur {
            write_stale_preserving_provenance_reason(conn, "embed_alias_changed")?;
            return Ok(());
        }
    }
    if let (Some(prev), Some(cur)) = (status.short_embed_alias.as_deref(), short_alias) {
        if prev != cur {
            write_stale_preserving_provenance_reason(conn, "short_embed_alias_changed")?;
            return Ok(());
        }
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
    let other_chunks_exist = total_chunks > chunks_len as u64 || read_generation(conn)? > 0;

    let Some(m) = model_meta else {
        if status.embed_model.is_some() && status.embed_dim == Some(first_vec_len) {
            // Incremental batches produced by the already-recorded bulk alias
            // do not carry model ids per page; matching the stored dimension is
            // the legacy contract for that path.
            return Ok(());
        }
        if status.embed_model.is_none() && status.embed_dim == Some(first_vec_len) {
            // The no-metadata ingestion contract means "configured bulk alias";
            // preserve that unnamed index until a probe gives the alias a name.
            return Ok(());
        }
        if status.embed_model.is_none()
            && status.embed_dim.is_none()
            && status.stale_reason.is_none()
            && !other_chunks_exist
        {
            let published = publish_ready_model_meta_tx(
                conn,
                &ReadyPublication {
                    expected_generation: read_generation(conn)?,
                    expected_state: status.index_state,
                    expected_reason: None,
                    model: None,
                    embed_dim: Some(first_vec_len),
                    bulk_alias: Some(
                        default_bulk_alias.unwrap_or(crate::embeddings::DEFAULT_EMBED_MODEL),
                    ),
                    short_alias: default_short_alias,
                    final_stale_reason: None,
                    kind: ReadyPublicationKind::Ordinary,
                },
            )?;
            return if published {
                Ok(())
            } else {
                Err(MakakooError::internal(
                    "fresh unnamed upsert lost the Ready publication compare-and-set",
                ))
            };
        }
        // Unnamed vectors have no proof that they belong to the stored pair. Keep
        // the flag sticky even when dimensions happen to match; only a complete
        // rebuild can establish provenance again.
        mark_unproven_vectors_tx(conn)?;
        if status.index_state != IndexModelState::Rebuilding {
            let reason = if other_chunks_exist || status.embed_model.is_some() {
                "mixed_model_upsert"
            } else {
                UNPROVEN_VECTOR_UPSERT_REASON
            };
            write_model_state(conn, IndexModelState::Stale, Some(reason))?;
        }
        return Ok(());
    };

    // Chunks written before model metadata existed have unknown provenance. A
    // client claim cannot make those old vectors Ready; the bulk probe must
    // rebuild the complete corpus first.
    if status.embed_model.is_none() && status.embed_dim.is_none() && other_chunks_exist {
        mark_unproven_vectors_tx(conn)?;
        if status.index_state != IndexModelState::Rebuilding {
            write_model_state(conn, IndexModelState::Stale, Some("mixed_model_upsert"))?;
        }
        return Ok(());
    }

    // Once index metadata exists, only the bulk probe/rebuild may change it.
    // Accept either the stored pair or the latest pair actually observed from
    // the bulk endpoint. Reject arbitrary client-claimed provenance.
    let metadata_exists =
        status.embed_model.is_some() || status.embed_dim.is_some() || status.stale_reason.is_some();
    if metadata_exists {
        let matches_stored = status.embed_dim == Some(m.embed_dim)
            && status.embed_model.as_deref() == Some(m.model.as_str());
        if matches_stored {
            return Ok(());
        }
        let matches_observed = read_observed_bulk(conn)?
            .is_some_and(|observed| observed.model == m.model && observed.dim == m.embed_dim);
        if matches_observed {
            mark_unproven_vectors_tx(conn)?;
            if status.index_state != IndexModelState::Rebuilding {
                write_model_state(
                    conn,
                    IndexModelState::Stale,
                    Some(OBSERVED_BULK_UPSERT_REASON),
                )?;
            }
            return Ok(());
        }
        return Err(MakakooError::RetryableFolderModelMismatch(
            "upsert model metadata matches neither the folder index nor the observed bulk embedder"
                .to_string(),
        ));
    }

    let published = publish_ready_model_meta_tx(
        conn,
        &ReadyPublication {
            expected_generation: read_generation(conn)?,
            expected_state: status.index_state,
            expected_reason: status.stale_reason.as_deref(),
            model: Some(&m.model),
            embed_dim: Some(m.embed_dim),
            bulk_alias: Some(&m.bulk_alias),
            short_alias: m.short_alias.as_deref(),
            final_stale_reason: None,
            kind: ReadyPublicationKind::Ordinary,
        },
    )?;
    if published {
        Ok(())
    } else {
        Err(MakakooError::internal(
            "fresh upsert lost the Ready publication compare-and-set",
        ))
    }
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
        write_stale_preserving_provenance_reason(&conn, reason)
    }

    /// Probe-time stale mark: never overwrites `rebuilding`. Read and write happen
    /// under one connection lock, the same lock the rebuild takes to set
    /// `rebuilding`, so a probe racing a rebuild start cannot hide it.
    pub(crate) fn mark_stale_unless_rebuilding(&self, reason: &str) -> Result<bool> {
        let conn = self.lock_conn()?;
        if read_model_status(&conn)?.index_state == IndexModelState::Rebuilding {
            return Ok(false);
        }
        write_stale_preserving_provenance_reason(&conn, reason)?;
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

pub(crate) fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
