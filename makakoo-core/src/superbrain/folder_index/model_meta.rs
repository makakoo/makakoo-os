//! Metadata persistence, alias checks, and search/upsert types for tracking
//! the active embedding model and dimension of a [`super::store::FolderIndex`].

use std::time::{SystemTime, UNIX_EPOCH};

use rand::rngs::OsRng;
use rand::RngCore;
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
const META_PROVENANCE_SCHEMA: &str = "provenance_schema";
const PROVENANCE_SCHEMA_VERSION: &str = "1";
const META_REBUILD_CLAIM: &str = "folder_rebuild_claim";
pub(crate) const FOLDER_REBUILD_LEASE_MS: u64 = 120_000;
pub(crate) const AUTOMATIC_REBUILD_SUCCESS_WINDOW_MS: u64 = 60 * 60 * 1_000;
pub(crate) const MANUAL_REBUILD_WINDOW_MS: u64 = 60 * 60 * 1_000;
pub(crate) const DEFAULT_REBUILD_FAILURE_CAP_MS: u64 = 6 * 60 * 60 * 1_000;
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
    /// Index generation at which this retry record was written. A newer
    /// provenance flag invalidates an older terminal decision.
    #[serde(default)]
    pub(crate) failure_generation: u64,
    /// Monotonic backoff exponent for repeated same-class failures. This stays
    /// above `failures` when a cooldown temporarily re-arms the halt threshold.
    #[serde(default)]
    pub(crate) backoff_failures: u32,
    /// Writer collisions have their own backoff sequence and never alter the
    /// genuine same-class failure streak.
    #[serde(default)]
    pub(crate) writer_races: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedAutomaticRebuildSuccess {
    #[serde(default)]
    pub(crate) last_success_unix_ms: u64,
    #[serde(default)]
    pub(crate) last_manual_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RebuildRetryVersion {
    pub(crate) failure_generation: u64,
    pub(crate) not_before_unix_ms: u64,
}

impl From<&PersistedRebuildRetry> for RebuildRetryVersion {
    fn from(value: &PersistedRebuildRetry) -> Self {
        Self {
            failure_generation: value.failure_generation,
            not_before_unix_ms: value.not_before_unix_ms,
        }
    }
}

/// Durable, cross-process rebuild ownership. `token` identifies resumable
/// parked rows for one model pair; `owner` fences one live process. A stale
/// lease may replace the owner while retaining the token for the same pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FolderRebuildClaim {
    pub(crate) token: String,
    pub(crate) owner: String,
    pub(crate) model: String,
    pub(crate) dim: usize,
    pub(crate) lease_expires_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RebuildClaimMode {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimedFolderRebuild {
    pub(crate) claim: FolderRebuildClaim,
    pub(crate) retry_version: Option<RebuildRetryVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RebuildClaimDecision {
    Claimed(ClaimedFolderRebuild),
    Busy,
    DeferredUntil(u64),
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
    pub generation: u64,
    pub embed_model: Option<String>,
    pub embed_dim: Option<usize>,
    pub embed_alias: Option<String>,
    pub short_embed_alias: Option<String>,
    pub stale_reason: Option<String>,
    pub query_mismatch_reason: Option<String>,
    pub unproven_vectors_since_gen: Option<u64>,
    pub halted: bool,
    pub rebuild_deferred_until: Option<u64>,
    pub rebuild_error_class: Option<String>,
    pub rebuild_failures: u32,
    pub rebuild_backoff_failures: u32,
    pub rebuild_writer_races: u32,
}

impl FolderModelStatus {
    /// Agent/CLI-safe diagnostic. Raw provider/HTTP text remains in logs and
    /// is never returned through public tool surfaces.
    pub fn public_stale_reason(&self) -> Option<String> {
        let raw = self.stale_reason.as_deref()?;
        if raw.starts_with("rebuild_failed:")
            || raw.starts_with("rebuild_suppressed:")
            || raw.starts_with("rebuild_deferred:")
        {
            return Some(match self.rebuild_error_class.as_deref() {
                Some(class) => format!(
                    "error_class={class} failures={} backoff_failures={} writer_races={}",
                    self.rebuild_failures, self.rebuild_backoff_failures, self.rebuild_writer_races
                ),
                None => "rebuild_error".to_string(),
            });
        }
        Some(raw.to_string())
    }
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
    read_model_status_at(conn, unix_time_ms(), DEFAULT_REBUILD_FAILURE_CAP_MS, None)
}

pub(crate) fn read_model_status_at(
    conn: &Connection,
    now_unix_ms: u64,
    rebuild_failure_cap_ms: u64,
    expected_retry_key: Option<(&str, usize)>,
) -> Result<FolderModelStatus> {
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
    let generation = read_generation(conn)?;
    let unproven_vectors_since_gen = read_unproven_vectors_since_gen(conn)?;
    let observed_retry_key = read_observed_bulk(conn).ok().flatten();
    let expected_retry_key = expected_retry_key.or_else(|| {
        observed_retry_key
            .as_ref()
            .map(|observed| (observed.model.as_str(), observed.dim))
    });
    let retry = read_persisted_rebuild_retry_lossy(conn)?;
    let halted = retry.as_ref().is_some_and(|retry| {
        rebuild_retry_is_halted(
            retry,
            expected_retry_key,
            unproven_vectors_since_gen,
            now_unix_ms,
            rebuild_failure_cap_ms,
        )
    });
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
        generation,
        embed_model,
        embed_dim,
        embed_alias,
        short_embed_alias,
        stale_reason,
        query_mismatch_reason: None,
        unproven_vectors_since_gen,
        halted,
        rebuild_deferred_until,
        rebuild_error_class: retry.as_ref().map(|value| value.error_class.clone()),
        rebuild_failures: retry.as_ref().map_or(0, |value| value.failures),
        rebuild_backoff_failures: retry.as_ref().map_or(0, |value| value.backoff_failures),
        rebuild_writer_races: retry.as_ref().map_or(0, |value| value.writer_races),
    })
}

pub(crate) fn rebuild_retry_is_halted(
    retry: &PersistedRebuildRetry,
    expected_key: Option<(&str, usize)>,
    unproven_vectors_since_gen: Option<u64>,
    now_unix_ms: u64,
    rebuild_failure_cap_ms: u64,
) -> bool {
    let key_matches =
        expected_key.is_none_or(|(model, dim)| retry.model == model && retry.dim == dim);
    let flag_does_not_supersede = unproven_vectors_since_gen
        .is_none_or(|flag_generation| flag_generation <= retry.failure_generation);
    let deadline_is_bounded =
        retry.not_before_unix_ms <= now_unix_ms.saturating_add(rebuild_failure_cap_ms);
    retry.halted
        && key_matches
        && flag_does_not_supersede
        && deadline_is_bounded
        && now_unix_ms < retry.not_before_unix_ms
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

fn new_rebuild_token() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub(crate) fn read_folder_rebuild_claim(conn: &Connection) -> Result<Option<FolderRebuildClaim>> {
    let Some(raw) = read_meta_opt(conn, META_REBUILD_CLAIM)? else {
        return Ok(None);
    };
    // A malformed claim cannot safely fence a writer. Treat it as expired;
    // the next IMMEDIATE claimant replaces it before doing any rebuild work.
    Ok(serde_json::from_str(&raw).ok())
}

pub(crate) fn folder_rebuild_claim_is_fresh(conn: &Connection, now_unix_ms: u64) -> Result<bool> {
    if read_model_status(conn)?.index_state != IndexModelState::Rebuilding {
        return Ok(false);
    }
    Ok(read_folder_rebuild_claim(conn)?
        .is_some_and(|claim| now_unix_ms < claim.lease_expires_unix_ms))
}

/// Claim `Rebuilding` and its owner lease in the same IMMEDIATE transaction.
/// A fresh claim refuses the contender. A stale same-pair claim keeps its
/// stable token so the new owner resumes already parked rows.
#[cfg(test)]
pub(crate) fn try_claim_folder_rebuild(
    conn: &mut Connection,
    model: &str,
    dim: usize,
    now_unix_ms: u64,
    automatic_attempt_unix_ms: Option<u64>,
) -> Result<Option<FolderRebuildClaim>> {
    let mode = if automatic_attempt_unix_ms.is_some() {
        RebuildClaimMode::Automatic
    } else {
        RebuildClaimMode::Manual
    };
    Ok(
        match try_claim_folder_rebuild_with_policy(
            conn,
            model,
            dim,
            now_unix_ms,
            mode,
            DEFAULT_REBUILD_FAILURE_CAP_MS,
            3,
        )? {
            RebuildClaimDecision::Claimed(claimed) => Some(claimed.claim),
            RebuildClaimDecision::Busy | RebuildClaimDecision::DeferredUntil(_) => None,
        },
    )
}

pub(crate) fn try_claim_folder_rebuild_with_policy(
    conn: &mut Connection,
    model: &str,
    dim: usize,
    now_unix_ms: u64,
    mode: RebuildClaimMode,
    rebuild_failure_cap_ms: u64,
    max_same_class_failures: u32,
) -> Result<RebuildClaimDecision> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = read_folder_rebuild_claim(&tx)?;
    if read_model_status(&tx)?.index_state == IndexModelState::Rebuilding
        && current
            .as_ref()
            .is_some_and(|claim| now_unix_ms < claim.lease_expires_unix_ms)
    {
        return Ok(RebuildClaimDecision::Busy);
    }

    let flag_generation = read_unproven_vectors_since_gen(&tx)?;
    let mut retry = match read_persisted_rebuild_retry(&tx) {
        Ok(retry) => retry,
        Err(_) => {
            clear_persisted_rebuild_retry(&tx)?;
            None
        }
    };
    if retry
        .as_ref()
        .is_some_and(|state| state.model != model || state.dim != dim)
    {
        clear_persisted_rebuild_retry(&tx)?;
        retry = None;
    }
    if let Some(state) = retry.as_mut() {
        let newer_flag =
            flag_generation.is_some_and(|generation| generation > state.failure_generation);
        let halt_active = rebuild_retry_is_halted(
            state,
            Some((model, dim)),
            flag_generation,
            now_unix_ms,
            rebuild_failure_cap_ms,
        );
        if newer_flag {
            // New provenance invalidates only the same-class halt streak. Keep
            // the per-key backoff and writer-collision history monotonic.
            state.failures = 0;
            state.halted = false;
            state.not_before_unix_ms = now_unix_ms;
            write_persisted_rebuild_retry(&tx, state)?;
        } else if state.halted && !halt_active {
            state.halted = false;
            state.failures = max_same_class_failures.saturating_sub(1);
            state.not_before_unix_ms = now_unix_ms;
            write_persisted_rebuild_retry(&tx, state)?;
        }
        let halt_active = rebuild_retry_is_halted(
            state,
            Some((model, dim)),
            flag_generation,
            now_unix_ms,
            rebuild_failure_cap_ms,
        );
        if mode == RebuildClaimMode::Automatic
            && (halt_active || now_unix_ms < state.not_before_unix_ms)
        {
            let until = state.not_before_unix_ms;
            tx.commit()?;
            return Ok(RebuildClaimDecision::DeferredUntil(until));
        }
    }

    let mut attempts = read_persisted_automatic_rebuild_success(&tx)?.unwrap_or(
        PersistedAutomaticRebuildSuccess {
            last_success_unix_ms: 0,
            last_manual_unix_ms: 0,
        },
    );
    let (last_attempt, window_ms) = match mode {
        RebuildClaimMode::Automatic => (
            attempts.last_success_unix_ms,
            AUTOMATIC_REBUILD_SUCCESS_WINDOW_MS,
        ),
        RebuildClaimMode::Manual => (attempts.last_manual_unix_ms, MANUAL_REBUILD_WINDOW_MS),
    };
    if last_attempt <= now_unix_ms && now_unix_ms < last_attempt.saturating_add(window_ms) {
        tx.commit()?;
        return Ok(RebuildClaimDecision::DeferredUntil(
            last_attempt.saturating_add(window_ms),
        ));
    }

    let token = current
        .as_ref()
        .filter(|claim| claim.model == model && claim.dim == dim)
        .map_or_else(new_rebuild_token, |claim| claim.token.clone());
    let claim = FolderRebuildClaim {
        token,
        owner: new_rebuild_token(),
        model: model.to_string(),
        dim,
        lease_expires_unix_ms: now_unix_ms.saturating_add(FOLDER_REBUILD_LEASE_MS),
    };
    let previous_reason = read_model_status(&tx)?
        .stale_reason
        .unwrap_or_else(|| "rebuilding".to_string());
    write_model_state(&tx, IndexModelState::Rebuilding, Some(&previous_reason))?;
    write_meta_opt(
        &tx,
        META_REBUILD_CLAIM,
        Some(&serde_json::to_string(&claim)?),
    )?;
    match mode {
        RebuildClaimMode::Automatic => attempts.last_success_unix_ms = now_unix_ms,
        RebuildClaimMode::Manual => attempts.last_manual_unix_ms = now_unix_ms,
    }
    // The legacy key/field name is retained for on-disk compatibility. Both
    // budgets are checked and charged under the same IMMEDIATE claim lock.
    write_persisted_automatic_rebuild_success(&tx, &attempts)?;
    if mode == RebuildClaimMode::Manual {
        if let Some(state) = retry.as_mut() {
            state.failures = 0;
            state.halted = false;
            state.not_before_unix_ms = now_unix_ms;
            write_persisted_rebuild_retry(&tx, state)?;
        }
    }
    tx.execute(
        "DELETE FROM rebuild_vectors WHERE claim_token <> ?1",
        params![&claim.token],
    )?;
    tx.commit()?;
    Ok(RebuildClaimDecision::Claimed(ClaimedFolderRebuild {
        claim,
        retry_version: retry.as_ref().map(RebuildRetryVersion::from),
    }))
}

pub(crate) fn folder_rebuild_claim_owned(
    conn: &Connection,
    claim: &FolderRebuildClaim,
) -> Result<bool> {
    Ok(
        read_model_status(conn)?.index_state == IndexModelState::Rebuilding
            && read_folder_rebuild_claim(conn)?.is_some_and(|current| {
                current.token == claim.token
                    && current.owner == claim.owner
                    && current.model == claim.model
                    && current.dim == claim.dim
            }),
    )
}

pub(crate) fn touch_folder_rebuild_claim(
    conn: &mut Connection,
    claim: &FolderRebuildClaim,
    now_unix_ms: u64,
) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !folder_rebuild_claim_owned(&tx, claim)? {
        return Err(MakakooError::RebuildClaimLost);
    }
    let mut refreshed = claim.clone();
    refreshed.lease_expires_unix_ms = now_unix_ms.saturating_add(FOLDER_REBUILD_LEASE_MS);
    write_meta_opt(
        &tx,
        META_REBUILD_CLAIM,
        Some(&serde_json::to_string(&refreshed)?),
    )?;
    tx.commit()?;
    Ok(())
}

/// Release one owner's live lease but retain the stable token and parked rows
/// so a later process can resume the same model pair.
pub(crate) fn expire_folder_rebuild_claim(
    conn: &mut Connection,
    claim: &FolderRebuildClaim,
    reason: &str,
) -> Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !folder_rebuild_claim_owned(&tx, claim)? {
        return Ok(false);
    }
    let mut expired = claim.clone();
    expired.lease_expires_unix_ms = 0;
    write_meta_opt(
        &tx,
        META_REBUILD_CLAIM,
        Some(&serde_json::to_string(&expired)?),
    )?;
    write_model_state(&tx, IndexModelState::Stale, Some(reason))?;
    tx.commit()?;
    Ok(true)
}

pub(crate) fn clear_folder_rebuild_claim_tx(
    conn: &Connection,
    claim: &FolderRebuildClaim,
) -> Result<()> {
    if !folder_rebuild_claim_owned(conn, claim)? {
        return Err(MakakooError::RebuildClaimLost);
    }
    write_meta_opt(conn, META_REBUILD_CLAIM, None)
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

/// Status/search paths must remain available when a legacy or partially
/// written diagnostic record is corrupt. Claim paths call the strict reader
/// and delete malformed data while holding an IMMEDIATE transaction.
pub(crate) fn read_persisted_rebuild_retry_lossy(
    conn: &Connection,
) -> Result<Option<PersistedRebuildRetry>> {
    let Some(raw) = read_meta_opt(conn, META_REBUILD_RETRY)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&raw).ok())
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

pub(crate) fn rebuild_retry_version_matches(
    conn: &Connection,
    expected: Option<RebuildRetryVersion>,
) -> Result<bool> {
    let current = read_persisted_rebuild_retry(conn)?;
    Ok(current.as_ref().map(RebuildRetryVersion::from) == expected)
}

pub(crate) fn write_persisted_rebuild_retry_cas(
    conn: &Connection,
    expected: Option<RebuildRetryVersion>,
    retry: &PersistedRebuildRetry,
) -> Result<bool> {
    if !rebuild_retry_version_matches(conn, expected)? {
        return Ok(false);
    }
    write_persisted_rebuild_retry(conn, retry)?;
    Ok(true)
}

pub(crate) fn clear_persisted_rebuild_retry_cas(
    conn: &Connection,
    expected: Option<RebuildRetryVersion>,
) -> Result<bool> {
    if !rebuild_retry_version_matches(conn, expected)? {
        return Ok(false);
    }
    clear_persisted_rebuild_retry(conn)?;
    Ok(true)
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
    _chunk_count: u64,
    bulk_alias: Option<&str>,
    short_alias: Option<&str>,
) -> Result<()> {
    // One-time fail-closed migration for databases created before vector-write
    // provenance was transactional. Historical query/alias reasons could have
    // overwritten `mixed_model_upsert`, and Ready databases could have accepted
    // same-dimension unnamed vectors. With no proof left, rebuild once.
    {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_meta_opt(&tx, META_PROVENANCE_SCHEMA)?.as_deref() != Some(PROVENANCE_SCHEMA_VERSION)
        {
            if read_chunk_count(&tx)? > 0 {
                mark_unproven_vectors_tx(&tx)?;
                if !folder_rebuild_claim_is_fresh(&tx, unix_time_ms())? {
                    write_model_state(
                        &tx,
                        IndexModelState::Stale,
                        Some("provenance_migration_required"),
                    )?;
                }
            }
            write_meta_opt(&tx, META_PROVENANCE_SCHEMA, Some(PROVENANCE_SCHEMA_VERSION))?;
        }
        tx.commit()?;
    }

    let chunk_count = read_chunk_count(conn)?;
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
    // dimension with no model id is an unnamed index (see authorize_vector_write_tx).
    if status.embed_dim.is_none() {
        write_stale_preserving_provenance_reason(conn, "missing_embed_model_metadata")?;
        return Ok(());
    }

    if status.index_state == IndexModelState::Rebuilding {
        if folder_rebuild_claim_is_fresh(conn, unix_time_ms())? {
            return Ok(());
        }
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

/// The single authorization door for every chunk-vector write. It runs inside
/// the writer's IMMEDIATE transaction, so rejection rolls back the rows and
/// every accepted unproven write sets the sticky flag atomically with them.
pub(crate) fn authorize_vector_write_tx(
    conn: &Connection,
    chunks_len: usize,
    model_meta: Option<&UpsertModelMeta>,
) -> Result<()> {
    if chunks_len == 0 {
        return Ok(());
    }
    let status = read_model_status(conn)?;
    let total_chunks = read_chunk_count(conn)?;
    let other_chunks_exist = total_chunks > chunks_len as u64 || read_generation(conn)? > 0;

    let Some(m) = model_meta else {
        // A dimension and an earlier endpoint observation cannot prove which
        // model produced these bytes. Keep the flag sticky for every nonempty
        // metadata-free write; only a complete rebuild establishes provenance.
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
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        let mut status = read_model_status(&tx)?;
        tx.commit()?;
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

    pub(crate) fn authorize_vector_write_tx(
        &self,
        conn: &Connection,
        chunks_len: usize,
        model_meta: Option<&UpsertModelMeta>,
    ) -> Result<()> {
        authorize_vector_write_tx(conn, chunks_len, model_meta)
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
