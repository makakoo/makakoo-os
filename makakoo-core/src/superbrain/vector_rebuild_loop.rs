//! Owner-fenced Brain vector rebuild execution loop with per-call timeouts
//! and cancel-safe inline entry points.

use std::sync::Arc;
use std::time::Duration;

use crate::embeddings::{EmbedBatchResult, EmbeddingClient, DEFAULT_EMBED_TIMEOUT, PROBE_TIMEOUT};
use crate::error::{MakakooError, Result};

use super::folder_index::model_meta::{IndexModelState, SHORT_BULK_MISMATCH_REASON};
use super::store::SuperbrainStore;
use super::vector_claim::{mark_stale_for_owner_with_retry, HeartbeatGuard};
use super::vector_model::DEFAULT_EMBED_TRUNCATE_CHARS;

const REBUILD_BATCH_SIZE: usize = 32;

/// Optional per-call timeout override (ms) for embed and probe requests inside
/// a Brain vector rebuild.
pub const REBUILD_CALL_TIMEOUT_MS_ENV: &str = "MAKAKOO_REBUILD_CALL_TIMEOUT_MS";
/// Optional per-call timeout override (s) for embed and probe requests inside
/// a Brain vector rebuild.
pub const REBUILD_CALL_TIMEOUT_SECS_ENV: &str = "MAKAKOO_REBUILD_CALL_TIMEOUT_SECS";

/// Resolve the per-call timeout for a single probe or embed request inside a
/// Brain vector rebuild.
///
/// Precedence:
/// 1. Explicit timeout configured on `client` (`EmbeddingClient::with_timeout`).
/// 2. `MAKAKOO_REBUILD_CALL_TIMEOUT_MS` (positive integer ms).
/// 3. `MAKAKOO_REBUILD_CALL_TIMEOUT_SECS` (positive integer seconds).
/// 4. `EmbeddingClient`'s existing default (`PROBE_TIMEOUT` = 5 s for probes,
///    `DEFAULT_EMBED_TIMEOUT` = 60 s for batch embeds).
///
/// There is intentionally no whole-rebuild ceiling so large Brains complete as
/// long as each individual probe/batch call finishes within its bound.
pub fn resolve_rebuild_call_timeout(client: &EmbeddingClient, is_probe: bool) -> Duration {
    resolve_rebuild_call_timeout_with(client.timeout(), is_probe, |k| std::env::var(k).ok())
}

pub(crate) fn resolve_rebuild_call_timeout_with(
    client_override: Option<Duration>,
    is_probe: bool,
    get_env: impl Fn(&str) -> Option<String>,
) -> Duration {
    if let Some(t) = client_override {
        return t;
    }
    if let Some(ms) = get_env(REBUILD_CALL_TIMEOUT_MS_ENV)
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
    {
        return Duration::from_millis(ms);
    }
    if let Some(secs) = get_env(REBUILD_CALL_TIMEOUT_SECS_ENV)
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
    {
        return Duration::from_secs(secs);
    }
    if is_probe {
        PROBE_TIMEOUT
    } else {
        DEFAULT_EMBED_TIMEOUT
    }
}

/// Format the stale reason written on rebuild failure, normalizing any call
/// timeout (`tokio::time::timeout` or `reqwest` timeout) to
/// `"rebuild_failed: timeout"`.
pub(crate) fn rebuild_failure_reason(err: &MakakooError) -> String {
    match err {
        MakakooError::RebuildCallTimeout => "rebuild_failed: timeout".to_string(),
        MakakooError::Http(e) if e.is_timeout() => "rebuild_failed: timeout".to_string(),
        other => format!("rebuild_failed: {other}"),
    }
}

async fn probe_with_timeout(
    client: &EmbeddingClient,
    timeout: Duration,
) -> Result<(String, usize)> {
    match tokio::time::timeout(timeout, client.probe_model_with_timeout(timeout)).await {
        Ok(Ok(pair)) => Ok(pair),
        Ok(Err(MakakooError::Http(e))) if e.is_timeout() => Err(MakakooError::RebuildCallTimeout),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(MakakooError::RebuildCallTimeout),
    }
}

async fn embed_batch_with_timeout(
    client: &EmbeddingClient,
    texts: &[String],
    timeout: Duration,
) -> Result<EmbedBatchResult> {
    match tokio::time::timeout(
        timeout,
        client.embed_batch_with_meta_timeout(texts, timeout),
    )
    .await
    {
        Ok(Ok(res)) => Ok(res),
        Ok(Err(MakakooError::Http(e))) if e.is_timeout() => Err(MakakooError::RebuildCallTimeout),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(MakakooError::RebuildCallTimeout),
    }
}

/// Releases an inline rebuild claim with `rebuild_cancelled` if the caller
/// future is dropped mid-rebuild.
struct OwnerCancelGuard {
    store: Arc<SuperbrainStore>,
    owner: Option<String>,
}

impl OwnerCancelGuard {
    fn new(store: Arc<SuperbrainStore>, owner: String) -> Self {
        Self {
            store,
            owner: Some(owner),
        }
    }

    fn disarm(&mut self) {
        self.owner = None;
    }
}

impl Drop for OwnerCancelGuard {
    fn drop(&mut self) {
        let Some(owner) = self.owner.take() else {
            return;
        };
        let store = Arc::clone(&self.store);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                mark_stale_for_owner_with_retry(&store, &owner, "rebuild_cancelled").await;
            });
        } else {
            let _ = store.mark_vectors_stale_for_owner(&owner, "rebuild_cancelled");
        }
    }
}

/// Acquire a rebuild claim in a detached task so dropping the caller future
/// while `spawn_blocking` is running cannot orphan a committed claim.
async fn claim_cancel_safe(
    store: Arc<SuperbrainStore>,
    reason: &'static str,
) -> Result<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let store_claim = Arc::clone(&store);
        let res =
            tokio::task::spawn_blocking(move || store_claim.try_mark_vectors_rebuilding(reason))
                .await
                .map_err(|e| MakakooError::internal(format!("rebuild claim join error: {e}")))
                .and_then(|inner| inner);
        match res {
            Ok(Some(owner)) => {
                if tx.send(Ok(Some(owner.clone()))).is_err() {
                    mark_stale_for_owner_with_retry(&store, &owner, "rebuild_cancelled").await;
                }
            }
            other => {
                let _ = tx.send(other);
            }
        }
    });
    rx.await
        .map_err(|e| MakakooError::internal(format!("rebuild claim channel error: {e}")))?
}

/// Re-embed all `brain_docs` missing a `(b_model, b_dim)` vector with
/// `bulk_emb` under an already-acquired rebuild claim `owner`.
///
/// A concurrent heartbeat ticker (`LEASE / 4`) runs for the full owner scope
/// (probe, every batch, finish), stops when the rebuild ends, and cancels the
/// rebuild on [`MakakooError::RebuildClaimLost`]. Every probe and batch embed
/// call carries its own per-call timeout (`resolve_rebuild_call_timeout`).
pub(crate) async fn rebuild_brain_vectors_with_owner(
    store: Arc<SuperbrainStore>,
    owner: &str,
    bulk_emb: Arc<EmbeddingClient>,
    short_emb: Option<Arc<EmbeddingClient>>,
    observed_short: Option<(String, usize)>,
) -> Result<usize> {
    let hb = HeartbeatGuard::start(Arc::clone(&store), owner.to_string());
    let body = rebuild_brain_vectors_body(store, owner, bulk_emb, short_emb, observed_short);
    let result = tokio::select! {
        biased;
        res = body => res,
        _ = hb.claim_lost() => Err(MakakooError::RebuildClaimLost),
    };
    drop(hb);
    result
}

async fn rebuild_brain_vectors_body(
    store: Arc<SuperbrainStore>,
    owner: &str,
    bulk_emb: Arc<EmbeddingClient>,
    short_emb: Option<Arc<EmbeddingClient>>,
    mut observed_short: Option<(String, usize)>,
) -> Result<usize> {
    let bulk_probe_timeout = resolve_rebuild_call_timeout(&bulk_emb, true);
    let (b_model, b_dim) = probe_with_timeout(&bulk_emb, bulk_probe_timeout).await?;

    if observed_short.is_none() {
        if let Some(ref s) = short_emb {
            if s.model() != bulk_emb.model() || s.base_url() != bulk_emb.base_url() {
                let short_probe_timeout = resolve_rebuild_call_timeout(s, true);
                match probe_with_timeout(s, short_probe_timeout).await {
                    Ok(pair) => observed_short = Some(pair),
                    Err(MakakooError::RebuildCallTimeout) => {
                        return Err(MakakooError::RebuildCallTimeout);
                    }
                    Err(_) => {}
                }
            }
        }
    }

    store.touch_rebuild_heartbeat(owner)?;

    if let Some((ref s_model, s_dim)) = observed_short {
        if s_model != &b_model || s_dim != b_dim {
            if !store.mark_vectors_stale_for_owner(owner, SHORT_BULK_MISMATCH_REASON)? {
                return Err(MakakooError::RebuildClaimLost);
            }
            return Ok(0);
        }
    }

    let embed_timeout = resolve_rebuild_call_timeout(&bulk_emb, false);
    let mut written = 0usize;
    loop {
        // `docs_needing_vectors` skips docs that already carry a `(b_model, b_dim)`
        // vector, making interrupted or cancelled rebuilds resumable.
        let batch = store.docs_needing_vectors(&b_model, b_dim, REBUILD_BATCH_SIZE)?;
        if batch.is_empty() {
            break;
        }
        store.touch_rebuild_heartbeat(owner)?;
        let texts: Vec<String> = batch
            .iter()
            .map(|(_, content)| content.chars().take(DEFAULT_EMBED_TRUNCATE_CHARS).collect())
            .collect();
        let res = embed_batch_with_timeout(&bulk_emb, &texts, embed_timeout).await?;
        if res.vectors.len() != batch.len() {
            return Err(MakakooError::llm(format!(
                "brain rebuild expected {} vectors, got {}",
                batch.len(),
                res.vectors.len()
            )));
        }
        if res.model != b_model || res.dim != b_dim {
            return Err(MakakooError::llm("bulk embedder model changed mid-rebuild"));
        }
        store.touch_rebuild_heartbeat(owner)?;
        for ((doc_id, _), vec) in batch.iter().zip(res.vectors) {
            if vec.is_empty() {
                continue;
            }
            match store.store_vector_for_rebuild(owner, doc_id, &vec, &res.model) {
                Ok(()) => written += 1,
                Err(MakakooError::NotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }
    }

    store.touch_rebuild_heartbeat(owner)?;
    store.finish_rebuild(
        owner,
        &b_model,
        b_dim,
        bulk_emb.model(),
        short_emb.as_deref().map(EmbeddingClient::model),
    )?;
    Ok(written)
}

/// Re-embed all `brain_docs` needing `(b_model, b_dim)` with `bulk_emb` and
/// update `brain_vector_meta`. Acquires an owner-fenced rebuild claim; returns
/// `Ok(0)` if another rebuild holds a fresh lease.
pub async fn rebuild_brain_vectors(
    store: Arc<SuperbrainStore>,
    bulk_emb: Arc<EmbeddingClient>,
    short_emb: Option<Arc<EmbeddingClient>>,
    observed_short: Option<(String, usize)>,
) -> Result<usize> {
    let Some(owner) = claim_cancel_safe(Arc::clone(&store), "rebuilding").await? else {
        return Ok(0);
    };
    let mut cancel_guard = OwnerCancelGuard::new(Arc::clone(&store), owner.clone());
    let result = rebuild_brain_vectors_with_owner(
        Arc::clone(&store),
        &owner,
        bulk_emb,
        short_emb,
        observed_short,
    )
    .await;
    cancel_guard.disarm();
    if let Err(err) = &result {
        if !matches!(err, MakakooError::RebuildClaimLost) {
            let reason = rebuild_failure_reason(err);
            mark_stale_for_owner_with_retry(&store, &owner, &reason).await;
        }
    }
    result
}

/// Inline embedding entry point for the CLI (`makakoo sync --embed`). On a
/// ready index with a recorded model, tops up at most `limit` missing docs.
/// Otherwise claims an owner-fenced rebuild and runs it inline.
pub async fn embed_pending_brain(
    store: Arc<SuperbrainStore>,
    bulk: Arc<EmbeddingClient>,
    short: Option<Arc<EmbeddingClient>>,
    limit: usize,
    truncate_chars: usize,
) -> Result<usize> {
    let status = store.vector_status()?;
    let has_model = status.embed_model.is_some() && status.embed_dim.is_some();
    if status.index_state == IndexModelState::Ready && has_model {
        return store
            .sync_vectors_with_embedder(&bulk, limit, truncate_chars)
            .await;
    }
    if status.stale_reason.as_deref() == Some(SHORT_BULK_MISMATCH_REASON) {
        return Ok(0);
    }
    let Some(owner) = claim_cancel_safe(Arc::clone(&store), "embed_pending").await? else {
        return Ok(0);
    };
    let mut cancel_guard = OwnerCancelGuard::new(Arc::clone(&store), owner.clone());
    let result =
        rebuild_brain_vectors_with_owner(Arc::clone(&store), &owner, bulk, short, None).await;
    cancel_guard.disarm();
    if let Err(e) = &result {
        if !matches!(e, MakakooError::RebuildClaimLost) {
            let reason = rebuild_failure_reason(e);
            mark_stale_for_owner_with_retry(&store, &owner, &reason).await;
        }
    }
    result
}
