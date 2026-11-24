//! Deduplicated background rebuild coordinator for `SuperbrainStore`.
//!
//! Claims, rebuild execution, and panic/cancel cleanup run inside one detached
//! task per active DB path so dropping a caller's `spawn_rebuild` future can
//! never orphan a committed DB claim or its heartbeat.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::folder_index::model_meta::{IndexModelState, SHORT_BULK_MISMATCH_REASON};
use super::store::SuperbrainStore;
use super::vector_claim::{mark_stale_for_owner_with_retry, REBUILD_LEASE_MS};
use super::vector_rebuild_loop::rebuild_failure_reason;

pub(crate) use super::vector_rebuild_loop::rebuild_brain_vectors_with_owner;
pub use super::vector_rebuild_loop::{
    embed_pending_brain, rebuild_brain_vectors, resolve_rebuild_call_timeout,
    REBUILD_CALL_TIMEOUT_MS_ENV, REBUILD_CALL_TIMEOUT_SECS_ENV,
};

/// Default upper bound for bounded `wait_all` calls (`4 × LEASE` = 480 s).
pub const DEFAULT_REBUILD_WAIT_TIMEOUT: Duration =
    Duration::from_millis((REBUILD_LEASE_MS as u64) * 4);

const CANCEL_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Active coordinator slot for a single Brain SQLite path.
struct ActiveRebuild {
    notify: Arc<Notify>,
    store: Option<Arc<SuperbrainStore>>,
    owner: Mutex<Option<String>>,
    worker_abort: Mutex<Option<tokio::task::AbortHandle>>,
    cancelled: AtomicBool,
}

impl ActiveRebuild {
    fn new(store: Option<Arc<SuperbrainStore>>) -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            store,
            owner: Mutex::new(None),
            worker_abort: Mutex::new(None),
            cancelled: AtomicBool::new(false),
        }
    }
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
    active: Mutex<HashMap<PathBuf, Arc<ActiveRebuild>>>,
    spawned: AtomicUsize,
    active_probes: AtomicUsize,
    probe_notify: Notify,
    #[cfg(test)]
    pub(crate) after_lookup: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    pub(crate) before_rebuild: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    pub(crate) after_claim: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
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
                    Some(entry) => Arc::clone(&entry.notify),
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
                guard
                    .get(db_path)
                    .is_some_and(|e| Arc::ptr_eq(&e.notify, &notify))
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

    /// Cancel every active rebuild tracked by this coordinator, aborting its
    /// worker and ensuring an owner-checked `rebuild_cancelled` stale mark is
    /// written so the next tick or claimant can claim immediately.
    pub async fn cancel_active_rebuilds(&self) {
        let snapshot: Vec<(PathBuf, Arc<ActiveRebuild>)> = {
            let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };
        if snapshot.is_empty() {
            return;
        }
        for (_, entry) in &snapshot {
            entry.cancelled.store(true, Ordering::SeqCst);
            if let Some(abort) = entry
                .worker_abort
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                abort.abort();
            }
        }
        let _ = tokio::time::timeout(CANCEL_DRAIN_TIMEOUT, async {
            for (path, _) in &snapshot {
                self.wait(path).await;
            }
        })
        .await;
        // Belt-and-suspenders: if any watcher did not drain within the grace
        // window, still release its claim via the owner-checked stale mark.
        for (_, entry) in &snapshot {
            let owner = entry
                .owner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let (Some(store), Some(owner)) = (entry.store.as_ref(), owner) {
                mark_stale_for_owner_with_retry(store, &owner, "rebuild_cancelled").await;
            }
        }
    }

    /// Claim and spawn a background rebuild for `store`.
    ///
    /// Claim, worker launch, and watcher ownership all live inside one detached
    /// `tokio::spawn` task: dropping the caller's future while waiting for the
    /// claim cannot orphan a committed DB claim or its heartbeat.
    pub async fn spawn_rebuild(
        self: &Arc<Self>,
        store: Arc<SuperbrainStore>,
        bulk_emb: Arc<EmbeddingClient>,
        short_emb: Option<Arc<EmbeddingClient>>,
        observed_short: Option<(String, usize)>,
    ) -> bool {
        let key = store.path().to_path_buf();
        let Some(entry) = self.try_acquire_entry(&key, Some(Arc::clone(&store))) else {
            return false;
        };
        let guard = ActiveSlotGuard {
            coord: Arc::clone(self),
            key,
        };
        let coord = Arc::clone(self);
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();

        tokio::spawn(async move {
            let _slot = guard;
            let store_claim = Arc::clone(&store);
            #[cfg(test)]
            let after_claim = coord
                .after_claim
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(Arc::clone);

            let owner = match tokio::task::spawn_blocking(move || {
                let res = store_claim.try_mark_vectors_rebuilding("rebuilding");
                #[cfg(test)]
                if let Some(hook) = after_claim {
                    hook();
                }
                res
            })
            .await
            {
                Ok(Ok(Some(owner))) => owner,
                Ok(Ok(None)) => {
                    let _ = tx.send(false);
                    return;
                }
                Ok(Err(err)) => {
                    tracing::warn!(error = %err, "brain vector rebuild claim failed");
                    let _ = tx.send(false);
                    return;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "brain vector rebuild claim task failed");
                    let _ = tx.send(false);
                    return;
                }
            };

            {
                let mut slot_owner = entry.owner.lock().unwrap_or_else(|e| e.into_inner());
                *slot_owner = Some(owner.clone());
            }
            if entry.cancelled.load(Ordering::SeqCst) {
                mark_stale_for_owner_with_retry(&store, &owner, "rebuild_cancelled").await;
                let _ = tx.send(false);
                return;
            }

            coord.spawned.fetch_add(1, Ordering::SeqCst);
            let owner_worker = owner.clone();
            let store_worker = Arc::clone(&store);
            #[cfg(test)]
            let coord_for_hook = Arc::clone(&coord);
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
                        let reason = rebuild_failure_reason(&err);
                        mark_stale_for_owner_with_retry(&store_worker, &owner_worker, &reason)
                            .await;
                    }
                }
            });

            {
                let mut abort_slot = entry.worker_abort.lock().unwrap_or_else(|e| e.into_inner());
                *abort_slot = Some(worker.abort_handle());
            }
            if entry.cancelled.load(Ordering::SeqCst) {
                worker.abort();
            }

            // Caller cancellation ignores the oneshot; detached task keeps
            // watching the worker and holding `_slot` until completion.
            let _ = tx.send(true);

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

        rx.await.unwrap_or(false)
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
    ) -> Result<BrainSyncOutcome> {
        let status = store.vector_status()?;
        let has_model = status.embed_model.is_some() && status.embed_dim.is_some();
        if status.index_state == IndexModelState::Ready && has_model {
            let n = store.sync_vectors_with_embedder(&bulk, limit).await?;
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

    fn try_acquire_entry(
        &self,
        key: &Path,
        store: Option<Arc<SuperbrainStore>>,
    ) -> Option<Arc<ActiveRebuild>> {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if guard.contains_key(key) {
            return None;
        }
        let entry = Arc::new(ActiveRebuild::new(store));
        guard.insert(key.to_path_buf(), Arc::clone(&entry));
        Some(entry)
    }

    /// Claim the rebuild slot for `key`; false when one is already running.
    #[cfg(test)]
    pub(crate) fn try_acquire(&self, key: &Path) -> bool {
        self.try_acquire_entry(key, None).is_some()
    }

    /// Release the slot, then wake every waiter (removal first: see `wait`).
    pub(crate) fn finish(&self, key: &Path) {
        let notify = {
            let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
            guard.remove(key).map(|e| Arc::clone(&e.notify))
        };
        if let Some(n) = notify {
            n.notify_waiters();
        }
    }
}
