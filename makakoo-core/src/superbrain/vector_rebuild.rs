//! Deduplicated background rebuild coordinator and owner-fenced Brain vector
//! rebuild loops for `SuperbrainStore`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::folder_index::model_meta::{IndexModelState, SHORT_BULK_MISMATCH_REASON};
use super::store::SuperbrainStore;
use super::vector_claim::{mark_stale_for_owner_with_retry, HeartbeatGuard, REBUILD_LEASE_MS};
use super::vector_model::DEFAULT_EMBED_TRUNCATE_CHARS;

const REBUILD_BATCH_SIZE: usize = 32;

/// Default upper bound for bounded `wait_all` calls (`4 × LEASE` = 480 s).
pub const DEFAULT_REBUILD_WAIT_TIMEOUT: Duration =
    Duration::from_millis((REBUILD_LEASE_MS as u64) * 4);

/// Outcome of a non-blocking Brain vector sync pass (SANCHO tick).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrainSyncOutcome {
    /// Index was `Ready` with a recorded model; topped up at most `limit` docs.
    ToppedUp(usize),
    /// Index was not ready; a background rebuild was spawned on the coordinator.
    RebuildScheduled,
    /// No rebuild spawned (already running, short/bulk mismatch, or empty store).
    RebuildSkipped,
}

/// Releases a coordinator's active slot when the background rebuild task exits.
struct ActiveSlotGuard {
    coord: Arc<BrainRebuildCoordinator>,
    key: PathBuf,
}

impl Drop for ActiveSlotGuard {
    fn drop(&mut self) {
        self.coord.finish(&self.key);
    }
}

/// Tracks an in-flight store-open probe on a coordinator so `wait_all` waits
/// until any probe-triggered rebuild has been scheduled and finished.
pub(crate) struct ProbeSlotGuard {
    coord: Arc<BrainRebuildCoordinator>,
}

impl Drop for ProbeSlotGuard {
    fn drop(&mut self) {
        if self.coord.active_probes.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.coord.probe_notify.notify_waiters();
        }
    }
}

/// Coordinates deduplicated background rebuilds per Brain SQLite path.
#[derive(Default)]
pub struct BrainRebuildCoordinator {
    active: Mutex<HashMap<PathBuf, Arc<Notify>>>,
    spawned: AtomicUsize,
    active_probes: AtomicUsize,
    probe_notify: Notify,
    #[cfg(test)]
    pub(crate) after_lookup: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    pub(crate) before_rebuild: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl BrainRebuildCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self, db_path: &Path) -> bool {
        let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        guard.contains_key(db_path)
    }

    /// True while any open-probe or background rebuild is still running on this
    /// coordinator.
    pub fn has_active_work(&self) -> bool {
        if self.active_probes.load(Ordering::SeqCst) > 0 {
            return true;
        }
        let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        !guard.is_empty()
    }

    /// Number of background rebuilds spawned through this coordinator.
    pub fn spawned_count(&self) -> usize {
        self.spawned.load(Ordering::SeqCst)
    }

    pub(crate) fn enter_probe(self: &Arc<Self>) -> ProbeSlotGuard {
        self.active_probes.fetch_add(1, Ordering::SeqCst);
        ProbeSlotGuard {
            coord: Arc::clone(self),
        }
    }

    async fn wait_probes(&self) {
        loop {
            if self.active_probes.load(Ordering::SeqCst) == 0 {
                return;
            }
            let notified = self.probe_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.active_probes.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }

    pub async fn wait(&self, db_path: &Path) {
        loop {
            let notify = {
                let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
                match guard.get(db_path) {
                    Some(n) => Arc::clone(n),
                    None => return,
                }
            };
            #[cfg(test)]
            if let Some(hook) = self
                .after_lookup
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                hook();
            }
            // The rebuild task removes the entry before notifying, so re-check
            // after the waiter is registered: a finish between lookup and await
            // is not lost (`notify_waiters` stores no permit).
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let still_same = {
                let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
                guard.get(db_path).is_some_and(|n| Arc::ptr_eq(n, &notify))
            };
            if !still_same {
                continue;
            }
            notified.await;
        }
    }

    /// Wait until every in-flight open-probe and background rebuild spawned on
    /// this coordinator finishes.
    pub async fn wait_all(&self) {
        loop {
            self.wait_probes().await;
            let next = {
                let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
                guard.keys().next().cloned()
            };
            match next {
                Some(path) => self.wait(&path).await,
                None if self.active_probes.load(Ordering::SeqCst) == 0 => return,
                None => {}
            }
        }
    }

    /// Bounded variant of [`Self::wait_all`]. Returns `true` if all probes and
    /// rebuilds finished within `timeout`, or `false` if the bound elapsed.
    pub async fn wait_all_timeout(&self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, self.wait_all()).await.is_ok()
    }

    pub async fn spawn_rebuild(
        self: &Arc<Self>,
        store: Arc<SuperbrainStore>,
        bulk_emb: Arc<EmbeddingClient>,
        short_emb: Option<Arc<EmbeddingClient>>,
        observed_short: Option<(String, usize)>,
    ) -> bool {
        let key = store.path().to_path_buf();
        if !self.try_acquire(&key) {
            return false;
        }
        let guard = ActiveSlotGuard {
            coord: Arc::clone(self),
            key,
        };

        // Run the `BEGIN IMMEDIATE` claim in `spawn_blocking` so cross-process
        // SQLite busy-wait never stalls a Tokio async worker thread.
        let store_claim = Arc::clone(&store);
        let owner = match tokio::task::spawn_blocking(move || {
            store_claim.try_mark_vectors_rebuilding("rebuilding")
        })
        .await
        {
            Ok(Ok(Some(owner))) => owner,
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {
                // `guard` drops here and releases the active slot.
                return false;
            }
        };

        self.spawned.fetch_add(1, Ordering::SeqCst);
        let owner_worker = owner.clone();
        let store_worker = Arc::clone(&store);
        #[cfg(test)]
        let coord_for_hook = Arc::clone(self);
        let worker = tokio::spawn(async move {
            #[cfg(test)]
            if let Some(hook) = coord_for_hook
                .before_rebuild
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                hook();
            }
            if let Err(err) = rebuild_brain_vectors_with_owner(
                Arc::clone(&store_worker),
                &owner_worker,
                bulk_emb,
                short_emb,
                observed_short,
            )
            .await
            {
                if !matches!(err, MakakooError::RebuildClaimLost) {
                    mark_stale_for_owner_with_retry(
                        &store_worker,
                        &owner_worker,
                        &format!("rebuild_failed: {err}"),
                    )
                    .await;
                }
            }
        });

        // Hold `ActiveSlotGuard` in the JoinHandle watcher so a panic inside
        // the worker marks the index `stale` (`rebuild_panicked`) for its owner
        // BEFORE waiters unblock.
        tokio::spawn(async move {
            let _slot = guard;
            match worker.await {
                Ok(()) => {}
                Err(join_err) if join_err.is_panic() => {
                    tracing::warn!(error = %join_err, "brain vector rebuild task panicked");
                    mark_stale_for_owner_with_retry(&store, &owner, "rebuild_panicked").await;
                }
                Err(join_err) => {
                    tracing::warn!(error = %join_err, "brain vector rebuild task cancelled");
                    mark_stale_for_owner_with_retry(&store, &owner, "rebuild_cancelled").await;
                }
            }
        });
        true
    }

    /// Non-blocking Brain sync for SANCHO ticks: tops up at most `limit` docs
    /// when the index is `Ready` with a recorded model; otherwise asks this
    /// coordinator to rebuild in the background and returns immediately.
    pub async fn sync_or_spawn_rebuild(
        self: &Arc<Self>,
        store: Arc<SuperbrainStore>,
        bulk: Arc<EmbeddingClient>,
        short: Option<Arc<EmbeddingClient>>,
        limit: usize,
        truncate_chars: usize,
    ) -> Result<BrainSyncOutcome> {
        let status = store.vector_status()?;
        let has_model = status.embed_model.is_some() && status.embed_dim.is_some();
        if status.index_state == IndexModelState::Ready && has_model {
            let n = store
                .sync_vectors_with_embedder(&bulk, limit, truncate_chars)
                .await?;
            return Ok(BrainSyncOutcome::ToppedUp(n));
        }
        if status.stale_reason.as_deref() == Some(SHORT_BULK_MISMATCH_REASON) {
            return Ok(BrainSyncOutcome::RebuildSkipped);
        }
        if status.index_state == IndexModelState::Ready && !has_model && store.count()? == 0 {
            return Ok(BrainSyncOutcome::RebuildSkipped);
        }
        if self.spawn_rebuild(store, bulk, short, None).await {
            Ok(BrainSyncOutcome::RebuildScheduled)
        } else {
            Ok(BrainSyncOutcome::RebuildSkipped)
        }
    }

    /// Claim the rebuild slot for `key`; false when one is already running.
    pub(crate) fn try_acquire(&self, key: &Path) -> bool {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if guard.contains_key(key) {
            return false;
        }
        guard.insert(key.to_path_buf(), Arc::new(Notify::new()));
        true
    }

    /// Release the slot, then wake every waiter (removal first: see `wait`).
    pub(crate) fn finish(&self, key: &Path) {
        let notify = {
            let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
            guard.remove(key)
        };
        if let Some(n) = notify {
            n.notify_waiters();
        }
    }
}

/// Re-embed all `brain_docs` with `bulk_emb` under an already-acquired rebuild
/// claim `owner`. A concurrent heartbeat ticker (`LEASE / 4`) runs for the full
/// owner scope (probe, every batch, finish), stops when the rebuild ends, and
/// cancels the rebuild on [`MakakooError::RebuildClaimLost`]. Per-batch touches
/// also remain active.
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
    let (b_model, b_dim) = bulk_emb.probe_model().await?;

    if observed_short.is_none() {
        if let Some(ref s) = short_emb {
            if s.model() != bulk_emb.model() || s.base_url() != bulk_emb.base_url() {
                if let Ok(pair) = s.probe_model().await {
                    observed_short = Some(pair);
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

    let mut written = 0usize;
    loop {
        let batch = store.docs_needing_vectors(&b_model, b_dim, REBUILD_BATCH_SIZE)?;
        if batch.is_empty() {
            break;
        }
        store.touch_rebuild_heartbeat(owner)?;
        let texts: Vec<String> = batch
            .iter()
            .map(|(_, content)| content.chars().take(DEFAULT_EMBED_TRUNCATE_CHARS).collect())
            .collect();
        let res = bulk_emb.embed_batch_with_meta(&texts).await?;
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

/// Re-embed all `brain_docs` with `bulk_emb` and update `brain_vector_meta`.
/// Acquires an owner-fenced rebuild claim; returns `Ok(0)` if another rebuild
/// holds a fresh lease.
pub async fn rebuild_brain_vectors(
    store: Arc<SuperbrainStore>,
    bulk_emb: Arc<EmbeddingClient>,
    short_emb: Option<Arc<EmbeddingClient>>,
    observed_short: Option<(String, usize)>,
) -> Result<usize> {
    let store_claim = Arc::clone(&store);
    let Some(owner) =
        tokio::task::spawn_blocking(move || store_claim.try_mark_vectors_rebuilding("rebuilding"))
            .await
            .map_err(|e| MakakooError::internal(format!("rebuild claim join error: {e}")))??
    else {
        return Ok(0);
    };
    let result = rebuild_brain_vectors_with_owner(
        Arc::clone(&store),
        &owner,
        bulk_emb,
        short_emb,
        observed_short,
    )
    .await;
    if let Err(err) = &result {
        if !matches!(err, MakakooError::RebuildClaimLost) {
            mark_stale_for_owner_with_retry(&store, &owner, &format!("rebuild_failed: {err}"))
                .await;
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
    let store_claim = Arc::clone(&store);
    let Some(owner) = tokio::task::spawn_blocking(move || {
        store_claim.try_mark_vectors_rebuilding("embed_pending")
    })
    .await
    .map_err(|e| MakakooError::internal(format!("rebuild claim join error: {e}")))??
    else {
        return Ok(0);
    };
    let result =
        rebuild_brain_vectors_with_owner(Arc::clone(&store), &owner, bulk, short, None).await;
    if let Err(e) = &result {
        if !matches!(e, MakakooError::RebuildClaimLost) {
            mark_stale_for_owner_with_retry(&store, &owner, &format!("rebuild_failed: {e}")).await;
        }
    }
    result
}
