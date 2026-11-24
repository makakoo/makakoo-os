//! Background re-embedding rebuild orchestration and model-checked search
//! with FTS fallback for [`super::store::FolderIndex`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::model_meta::{
    read_model_status, write_model_state, write_ready_model_meta, FolderModelStatus,
    FolderSearchRequest, FolderSearchResponse, IndexModelState, SearchMode,
    SHORT_BULK_MISMATCH_REASON,
};
use super::rebuild_pages::{all_chunks_parked, reembed_pages, reset_parked, swap_in_parked};
use super::sqlite_repo::{bump_generation_tx, read_chunk_count, read_generation};
use super::FolderIndex;

/// Commit attempts before a rebuild that keeps racing writers gives up stale.
const MAX_REBUILD_COMMIT_ATTEMPTS: usize = 8;

/// Per-folder deduplication and completion notification registry for background
/// re-embedding rebuilds.
#[derive(Default)]
pub(crate) struct FolderRebuildRegistry {
    active: Mutex<HashMap<String, Arc<Notify>>>,
    probed: Mutex<HashSet<String>>,
    /// Test hook: runs in `wait` after the lookup, before the waiter registers.
    #[cfg(test)]
    pub(crate) after_lookup: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl FolderRebuildRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn mark_probed(&self, folder_id: &str) -> bool {
        let mut guard = self.probed.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(folder_id.to_string())
    }

    pub(crate) fn try_acquire(&self, folder_id: &str) -> Option<Arc<Notify>> {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if guard.contains_key(folder_id) {
            return None;
        }
        let notify = Arc::new(Notify::new());
        guard.insert(folder_id.to_string(), Arc::clone(&notify));
        Some(notify)
    }

    pub(crate) fn finish(&self, folder_id: &str) {
        let notify = {
            let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
            guard.remove(folder_id)
        };
        if let Some(n) = notify {
            n.notify_waiters();
        }
    }

    pub(crate) fn is_active(&self, folder_id: &str) -> bool {
        let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        guard.contains_key(folder_id)
    }

    pub(crate) async fn wait(&self, folder_id: &str) {
        loop {
            let notify = {
                let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
                match guard.get(folder_id) {
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
            // `finish` removes the entry before notifying, so re-check after the
            // waiter is registered: a finish between lookup and await is not lost.
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let still_same = {
                let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .get(folder_id)
                    .is_some_and(|n| Arc::ptr_eq(n, &notify))
            };
            if !still_same {
                // Finished (or replaced by a newer rebuild): look again.
                continue;
            }
            notified.await;
        }
    }
}

/// Non-blocking startup/open probe: verifies configured endpoint(s) against stored
/// model+dim. Unreachable endpoints leave `index_state` untouched.
pub(crate) async fn probe_and_reconcile(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    rebuilds: Arc<FolderRebuildRegistry>,
) {
    if idx.chunk_count().unwrap_or(0) == 0 {
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

    // A rebuild already running owns the model state; its own probe and the next
    // probe after it finishes decide. Writing `stale` here would hide `rebuilding`.
    if rebuilds.is_active(idx.folder_id()) {
        return;
    }

    if let Some((ref short_model, short_dim)) = short_result {
        if short_model != &bulk_model || short_dim != bulk_dim {
            let _ = idx.mark_stale_unless_rebuilding(SHORT_BULK_MISMATCH_REASON);
            return;
        }
    }

    let Ok(status) = idx.model_status() else {
        return;
    };
    if status.embed_model.is_none() && status.embed_dim == Some(bulk_dim) {
        // Rows were written without a model id by the bulk alias: name it now.
        if let Ok(conn) = idx.lock_conn() {
            let current = read_model_status(&conn);
            if matches!(&current, Ok(c) if c.embed_model.is_none()
                && c.index_state == IndexModelState::Ready)
            {
                let _ = write_ready_model_meta(
                    &conn,
                    &bulk_model,
                    bulk_dim,
                    bulk_emb.model(),
                    short_emb.as_deref().map(EmbeddingClient::model),
                );
            }
        }
        idx.update_sidecar_if_ready();
        return;
    }
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
            if let Ok(conn) = idx.lock_conn() {
                let _ = write_ready_model_meta(
                    &conn,
                    &bulk_model,
                    bulk_dim,
                    bulk_emb.model(),
                    short_emb.as_deref().map(EmbeddingClient::model),
                );
            }
            idx.update_sidecar_if_ready();
            return;
        }
        if status.index_state == IndexModelState::Stale {
            spawn_folder_rebuild(idx, short_emb, bulk_emb, rebuilds, short_result);
        }
        return;
    }

    if !matches!(
        idx.mark_stale_unless_rebuilding("model_changed_on_probe"),
        Ok(true)
    ) {
        return;
    }
    spawn_folder_rebuild(idx, short_emb, bulk_emb, rebuilds, short_result);
}

pub(crate) fn spawn_folder_rebuild(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    rebuilds: Arc<FolderRebuildRegistry>,
    observed_short: Option<(String, usize)>,
) {
    let folder_id = idx.folder_id().to_string();
    if rebuilds.try_acquire(&folder_id).is_none() {
        return;
    }

    if let Ok(conn) = idx.lock_conn() {
        let prev_reason = read_model_status(&conn)
            .ok()
            .and_then(|s| s.stale_reason)
            .unwrap_or_else(|| "rebuilding".to_string());
        let _ = write_model_state(&conn, IndexModelState::Rebuilding, Some(&prev_reason));
    }

    let rebuilds_clone = Arc::clone(&rebuilds);
    tokio::spawn(async move {
        let fid = idx.folder_id().to_string();
        if let Err(err) =
            run_folder_rebuild(Arc::clone(&idx), short_emb, bulk_emb, observed_short).await
        {
            let _ = idx.mark_stale(&format!("rebuild_failed: {err}"));
        }
        rebuilds_clone.finish(&fid);
    });
}

async fn run_folder_rebuild(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    mut observed_short: Option<(String, usize)>,
) -> Result<()> {
    let (start_gen, chunk_count) = {
        let conn = idx.lock_conn()?;
        reset_parked(&conn)?;
        (read_generation(&conn)?, read_chunk_count(&conn)?)
    };

    if chunk_count == 0 {
        let conn = idx.lock_conn()?;
        write_model_state(&conn, IndexModelState::Ready, None)?;
        return Ok(());
    }

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
            idx.mark_stale(SHORT_BULK_MISMATCH_REASON)?;
            return Ok(());
        }
    }

    reembed_pages(&idx, &bulk_emb, &b_model, b_dim).await?;

    // Upserts replace a file's chunks with new, higher ids and bump the
    // generation. Commit only while the generation is the one whose rows were
    // all re-embedded, checked under the commit's own lock; otherwise re-embed
    // the rows that landed meanwhile and try again.
    let mut seen_gen = start_gen;
    for _ in 0..MAX_REBUILD_COMMIT_ATTEMPTS {
        #[cfg(test)]
        if let Some(hook) = idx
            .before_rebuild_commit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            hook(&idx);
        }
        {
            let mut conn = idx.lock_conn()?;
            let gen_now = read_generation(&conn)?;
            if gen_now == seen_gen
                && commit_rebuild(
                    &idx, &mut conn, seen_gen, &b_model, b_dim, &bulk_emb, &short_emb,
                )?
            {
                drop(conn);
                idx.checkpoint_wal()?;
                return Ok(());
            }
            seen_gen = read_generation(&conn)?;
        }
        // Ids are reused once the top rows are deleted, so the tail is not
        // "ids above the last one": rescan every chunk, skipping those parked
        // with the same text.
        reembed_pages(&idx, &bulk_emb, &b_model, b_dim).await?;
    }
    Err(MakakooError::internal(
        "rebuild kept racing writers; left stale for the next probe",
    ))
}

/// Commit the parked vectors if the index is still at `seen_gen` and every
/// chunk holds a parked vector for its current text. Returns false when the
/// caller must rescan (a writer got in, or a row is not covered).
fn commit_rebuild(
    idx: &FolderIndex,
    conn: &mut rusqlite::Connection,
    seen_gen: u64,
    b_model: &str,
    b_dim: usize,
    bulk_emb: &EmbeddingClient,
    short_emb: &Option<Arc<EmbeddingClient>>,
) -> Result<bool> {
    // Vectors, generation and the ready model record commit together: a crash
    // leaves the old state, never new vectors under the old model's record.
    let new_gen = {
        // Immediate: no other process can write between the checks and the swap.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if read_generation(&tx)? != seen_gen || !all_chunks_parked(&tx)? {
            return Ok(false);
        }
        swap_in_parked(&tx)?;
        let new_gen = bump_generation_tx(&tx)?;
        write_ready_model_meta(
            &tx,
            b_model,
            b_dim,
            bulk_emb.model(),
            short_emb.as_deref().map(EmbeddingClient::model),
        )?;
        tx.commit()?;
        new_gen
    };
    let count = read_chunk_count(conn)?;
    idx.vectors
        .set_model_meta(Some(b_model.to_string()), Some(b_dim));
    idx.vectors.rebuild_from_sqlite(conn, new_gen, count)?;
    Ok(true)
}

/// Execute a model-aware search against `idx`, triggering background rebuild only
/// when the bulk embedder differs from the index and serving FTS fallback whenever
/// the index is stale, rebuilding, or the query metadata is mismatched.
pub(crate) async fn search_folder_checked(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Option<Arc<EmbeddingClient>>,
    rebuilds: Arc<FolderRebuildRegistry>,
    req: FolderSearchRequest,
) -> Result<FolderSearchResponse> {
    let status = idx.model_status()?;
    let chunk_count = idx.chunk_count()?;

    if chunk_count == 0 {
        let mode = if req.query_vector.is_some() && req.query.is_none() {
            SearchMode::Vector
        } else {
            SearchMode::Fts
        };
        return Ok(FolderSearchResponse {
            folder_id: req.folder_id,
            passages: Vec::new(),
            mode,
            stale: false,
            rebuilding: false,
            embed_model: status.embed_model,
            embed_dim: status.embed_dim,
        });
    }

    if req.query_vector.is_none() && status.index_state != IndexModelState::Ready {
        maybe_trigger_rebuild(&idx, &status, short_emb, bulk_emb, &rebuilds, None).await;
        return fts_fallback_response(&idx, &req, &rebuilds, true);
    }

    let (q_vec, q_model, q_dim) = if let Some(ref vec) = req.query_vector {
        (Some(vec.clone()), req.query_model.clone(), req.query_dim)
    } else if let (Some(q), Some(s_emb)) = (req.query.as_deref(), short_emb.as_ref()) {
        match s_emb.embed_with_meta(q).await {
            Ok(res) => (Some(res.vector), Some(res.model), Some(res.dim)),
            Err(_) => {
                return fts_fallback_response(
                    &idx,
                    &req,
                    &rebuilds,
                    status.index_state != IndexModelState::Ready,
                );
            }
        }
    } else {
        (None, req.query_model.clone(), req.query_dim)
    };

    // An index written without a model id is matched on dimension only until a bulk
    // probe names its model (probe_and_reconcile); calling it a switch would rebuild.
    let model_mismatch = match (q_model.as_deref(), status.embed_model.as_deref()) {
        (Some(qm), Some(im)) => qm != im,
        _ => false,
    };
    let dim_mismatch = match (q_dim, status.embed_dim) {
        (Some(qd), Some(id)) => qd != id,
        (Some(_), None) => true,
        _ => false,
    };

    if model_mismatch || dim_mismatch {
        let reason = if model_mismatch {
            format!(
                "model_mismatch: index={:?} query={:?}",
                status.embed_model, q_model
            )
        } else {
            format!(
                "dim_mismatch: index={:?} query={:?}",
                status.embed_dim, q_dim
            )
        };
        idx.record_query_mismatch(&reason)?;
        return fts_fallback_response(&idx, &req, &rebuilds, true);
    }

    if status.index_state != IndexModelState::Ready {
        maybe_trigger_rebuild(&idx, &status, short_emb, bulk_emb, &rebuilds, None).await;
        return fts_fallback_response(&idx, &req, &rebuilds, true);
    }

    if let Some(vec) = q_vec {
        idx.clear_query_mismatch()?;
        let passages = idx.search(&vec, req.k, &req.filter)?;
        Ok(FolderSearchResponse {
            folder_id: req.folder_id,
            passages,
            mode: SearchMode::Vector,
            stale: false,
            rebuilding: false,
            embed_model: status.embed_model,
            embed_dim: status.embed_dim,
        })
    } else {
        fts_fallback_response(&idx, &req, &rebuilds, false)
    }
}

fn fts_fallback_response(
    idx: &FolderIndex,
    req: &FolderSearchRequest,
    rebuilds: &FolderRebuildRegistry,
    stale: bool,
) -> Result<FolderSearchResponse> {
    let cur = idx.model_status()?;
    let passages = match req.query.as_deref() {
        Some(q) => idx.search_fts(q, req.k, &req.filter)?,
        None => Vec::new(),
    };
    Ok(FolderSearchResponse {
        folder_id: req.folder_id.clone(),
        passages,
        mode: SearchMode::Fts,
        stale,
        rebuilding: cur.index_state == IndexModelState::Rebuilding
            || rebuilds.is_active(&req.folder_id),
        embed_model: cur.embed_model,
        embed_dim: cur.embed_dim,
    })
}

async fn maybe_trigger_rebuild(
    idx: &Arc<FolderIndex>,
    status: &FolderModelStatus,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Option<Arc<EmbeddingClient>>,
    rebuilds: &Arc<FolderRebuildRegistry>,
    observed_short: Option<(String, usize)>,
) {
    let Some(b_emb) = bulk_emb else {
        return;
    };
    if rebuilds.is_active(idx.folder_id()) {
        return;
    }
    if status.stale_reason.as_deref() == Some(SHORT_BULK_MISMATCH_REASON) {
        let Ok(b_pair) = b_emb.probe_model().await else {
            return;
        };
        match short_emb.as_ref() {
            Some(s_emb) => match s_emb.probe_model().await {
                Ok(s_pair) if s_pair == b_pair => {}
                _ => return,
            },
            None => return,
        }
    }
    spawn_folder_rebuild(
        Arc::clone(idx),
        short_emb,
        b_emb,
        Arc::clone(rebuilds),
        observed_short,
    );
}
