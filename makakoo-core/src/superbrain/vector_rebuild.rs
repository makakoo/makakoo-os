//! Deduplicated background rebuild coordinator, non-blocking startup probe,
//! and model-checked vector search for `SuperbrainStore`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::folder_index::model_meta::{IndexModelState, SHORT_BULK_MISMATCH_REASON};
use super::store::{SuperbrainStore, VectorHit};
use super::vector_model::{BrainVectorStatus, DEFAULT_EMBED_TRUNCATE_CHARS};

const REBUILD_BATCH_SIZE: usize = 32;

/// Coordinates deduplicated background rebuilds per Brain SQLite path.
#[derive(Default)]
pub struct BrainRebuildCoordinator {
    active: Mutex<HashMap<PathBuf, Arc<Notify>>>,
    #[cfg(test)]
    pub(crate) after_lookup: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl BrainRebuildCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self, db_path: &Path) -> bool {
        let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        guard.contains_key(db_path)
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

    pub fn spawn_rebuild(
        self: &Arc<Self>,
        store: Arc<SuperbrainStore>,
        bulk_emb: Arc<EmbeddingClient>,
        short_emb: Option<Arc<EmbeddingClient>>,
        observed_short: Option<(String, usize)>,
    ) {
        let key = store.path().to_path_buf();
        if !self.try_acquire(&key) {
            return;
        }

        // The database claim keeps another process (the CLI) from rebuilding
        // at the same time; this process's slot is released if it is taken.
        if !store
            .try_mark_vectors_rebuilding("rebuilding")
            .unwrap_or(false)
        {
            self.finish(&key);
            return;
        }
        let coord = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) =
                rebuild_brain_vectors(Arc::clone(&store), bulk_emb, short_emb, observed_short).await
            {
                let _ = store.mark_vectors_stale(&format!("rebuild_failed: {err}"));
            }
            coord.finish(&key);
        });
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

/// Re-embed all `brain_docs` with `bulk_emb` and update `brain_vector_meta`.
pub async fn rebuild_brain_vectors(
    store: Arc<SuperbrainStore>,
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

    if let Some((ref s_model, s_dim)) = observed_short {
        if s_model != &b_model || s_dim != b_dim {
            store.mark_vectors_stale(SHORT_BULK_MISMATCH_REASON)?;
            return Ok(0);
        }
    }

    let mut written = 0usize;
    loop {
        let batch = store.docs_needing_vectors(&b_model, b_dim, REBUILD_BATCH_SIZE)?;
        if batch.is_empty() {
            break;
        }
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
        for ((doc_id, _), vec) in batch.iter().zip(res.vectors) {
            if vec.is_empty() {
                continue;
            }
            match store.store_vector_with_model(doc_id, &vec, &res.model) {
                Ok(()) => written += 1,
                Err(MakakooError::NotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }
    }

    store.delete_vectors_not_matching(&b_model, b_dim)?;
    store.set_vectors_ready(
        &b_model,
        b_dim,
        bulk_emb.model(),
        short_emb.as_deref().map(EmbeddingClient::model),
    )?;
    Ok(written)
}

/// The one entry point for background and CLI embedding. On a ready index with
/// a recorded model, tops up documents that have no vector (at most `limit`).
/// Otherwise runs the coordinated rebuild, after claiming it in the database so
/// no other process rebuilds at the same time. Returns vectors written.
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
    if !store.try_mark_vectors_rebuilding("embed_pending")? {
        return Ok(0);
    }
    let result = rebuild_brain_vectors(Arc::clone(&store), bulk, short, None).await;
    if let Err(e) = &result {
        let _ = store.mark_vectors_stale(&format!("rebuild_failed: {e}"));
    }
    result
}

/// Check stored Brain aliases on open and spawn a non-blocking endpoint probe
/// when existing vectors are present inside a Tokio runtime.
pub fn check_and_probe_brain_on_open(
    store: Arc<SuperbrainStore>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    coordinator: Arc<BrainRebuildCoordinator>,
) {
    let short_alias = short_emb.as_deref().map(EmbeddingClient::model);
    let _ = store.check_brain_alias_on_open(bulk_emb.model(), short_alias);
    if tokio::runtime::Handle::try_current().is_ok()
        && store.stats().map(|s| s.vector_count > 0).unwrap_or(false)
    {
        tokio::spawn(async move {
            probe_and_reconcile_brain(store, short_emb, bulk_emb, coordinator).await;
        });
    }
}

/// Non-blocking startup probe for `brain_vectors`. Unreachable endpoints leave
/// the current state untouched.
pub async fn probe_and_reconcile_brain(
    store: Arc<SuperbrainStore>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    coordinator: Arc<BrainRebuildCoordinator>,
) {
    let Ok(stats) = store.stats() else {
        return;
    };
    if stats.vector_count == 0 || coordinator.is_active(store.path()) {
        return;
    }
    let Ok((bulk_model, bulk_dim)) = bulk_emb.probe_model().await else {
        return;
    };

    let short_result = match short_emb.as_ref() {
        Some(s) if s.model() != bulk_emb.model() || s.base_url() != bulk_emb.base_url() => {
            s.probe_model().await.ok()
        }
        Some(_) => Some((bulk_model.clone(), bulk_dim)),
        None => None,
    };

    if let Some((ref short_model, short_dim)) = short_result {
        if short_model != &bulk_model || short_dim != bulk_dim {
            let _ = store.mark_vectors_stale_unless_rebuilding(SHORT_BULK_MISMATCH_REASON);
            return;
        }
    }

    let Ok(status) = store.vector_status() else {
        return;
    };
    let matches_stored =
        status.embed_model.as_deref() == Some(&bulk_model) && status.embed_dim == Some(bulk_dim);

    if matches_stored {
        if status.index_state == IndexModelState::Stale
            && matches!(
                status.stale_reason.as_deref(),
                Some(
                    "embed_alias_changed"
                        | "short_embed_alias_changed"
                        | SHORT_BULK_MISMATCH_REASON
                )
            )
        {
            let _ = store.set_vectors_ready_if_stale_for(
                &[
                    "embed_alias_changed",
                    "short_embed_alias_changed",
                    SHORT_BULK_MISMATCH_REASON,
                ],
                &bulk_model,
                bulk_dim,
                bulk_emb.model(),
                short_emb.as_deref().map(EmbeddingClient::model),
            );
            return;
        }
        if status.index_state == IndexModelState::Stale {
            coordinator.spawn_rebuild(store, bulk_emb, short_emb, short_result);
        }
        return;
    }

    // A rebuild that started after the is_active check owns the state.
    if !matches!(
        store.mark_vectors_stale_unless_rebuilding("model_changed_on_probe"),
        Ok(true)
    ) {
        return;
    }
    coordinator.spawn_rebuild(store, bulk_emb, short_emb, short_result);
}

/// Model-checked vector search over `SuperbrainStore`. Returns empty vector
/// hits while stale or rebuilding (and triggers background rebuild).
pub async fn search_brain_vectors_checked(
    store: &Arc<SuperbrainStore>,
    short_emb: &Arc<EmbeddingClient>,
    bulk_emb: Option<&Arc<EmbeddingClient>>,
    coordinator: &Arc<BrainRebuildCoordinator>,
    query: &str,
    limit: usize,
) -> Result<Vec<VectorHit>> {
    let status = store.vector_status()?;
    let vector_count = store.stats()?.vector_count;
    if vector_count == 0 {
        return Ok(Vec::new());
    }

    if status.index_state != IndexModelState::Ready {
        maybe_trigger_brain_rebuild(store, &status, Some(short_emb), bulk_emb, coordinator, None)
            .await;
        return Ok(Vec::new());
    }

    let res = short_emb.embed_with_meta(query).await?;
    let model_mismatch = status.embed_model.as_deref() != Some(res.model.as_str());
    let dim_mismatch = status.embed_dim != Some(res.dim);
    if model_mismatch || dim_mismatch {
        let reason = if model_mismatch {
            format!(
                "model_mismatch: index={:?} query={}",
                status.embed_model, res.model
            )
        } else {
            format!(
                "dim_mismatch: index={:?} query={}",
                status.embed_dim, res.dim
            )
        };
        // A running rebuild owns the state: answer from FTS until it lands.
        if !store.mark_vectors_stale_unless_rebuilding(&reason)? {
            return Ok(Vec::new());
        }
        let fresh_status = store.vector_status()?;
        maybe_trigger_brain_rebuild(
            store,
            &fresh_status,
            Some(short_emb),
            bulk_emb,
            coordinator,
            Some((res.model, res.dim)),
        )
        .await;
        return Ok(Vec::new());
    }

    store.vector_search_for_model(&res.vector, &res.model, limit)
}

async fn maybe_trigger_brain_rebuild(
    store: &Arc<SuperbrainStore>,
    status: &BrainVectorStatus,
    short_emb: Option<&Arc<EmbeddingClient>>,
    bulk_emb: Option<&Arc<EmbeddingClient>>,
    coordinator: &Arc<BrainRebuildCoordinator>,
    observed_short: Option<(String, usize)>,
) {
    let Some(b_emb) = bulk_emb.or(short_emb) else {
        return;
    };
    if coordinator.is_active(store.path()) {
        return;
    }
    if status.stale_reason.as_deref() == Some(SHORT_BULK_MISMATCH_REASON) {
        let Ok(b_pair) = b_emb.probe_model().await else {
            return;
        };
        match short_emb {
            Some(s_emb) => match s_emb.probe_model().await {
                Ok(s_pair) if s_pair == b_pair => {}
                _ => return,
            },
            None => return,
        }
    }
    coordinator.spawn_rebuild(
        Arc::clone(store),
        Arc::clone(b_emb),
        short_emb.cloned(),
        observed_short,
    );
}
