//! Multi-folder index manager caching open [`FolderIndex`] handles and
//! coordinating model probes and background rebuilds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::model_meta::{
    claim_observed_bulk_probe_expedite, FolderSearchRequest, FolderSearchResponse, UpsertModelMeta,
};
use super::rebuild::{self, FolderRebuildRegistry};
use super::{
    validate_dim, validate_folder_id, FolderIndex, FolderIndexConfig, IndexSizeReport,
    UpsertFileRequest, UpsertOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderRebuildRequestOutcome {
    Started,
    AlreadyRunning,
    AlreadyCurrent,
}

impl FolderRebuildRequestOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::AlreadyRunning => "already_running",
            Self::AlreadyCurrent => "already_current",
        }
    }
}

/// Multi-folder cache so long-lived callers reuse open `FolderIndex` instances
/// across calls without re-opening or rewriting on every file.
pub struct FolderIndexManager {
    base_dir: PathBuf,
    open: Mutex<HashMap<String, Arc<FolderIndex>>>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Option<Arc<EmbeddingClient>>,
    rebuilds: Arc<FolderRebuildRegistry>,
}

impl FolderIndexManager {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            open: Mutex::new(HashMap::new()),
            short_emb: None,
            bulk_emb: None,
            rebuilds: Arc::new(FolderRebuildRegistry::new()),
        }
    }

    pub fn with_embedders(
        mut self,
        short_emb: Arc<EmbeddingClient>,
        bulk_emb: Arc<EmbeddingClient>,
    ) -> Self {
        self.short_emb = Some(short_emb);
        self.bulk_emb = Some(bulk_emb);
        self
    }

    /// Set the successful bulk-model re-probe interval. The default is ten
    /// minutes; endpoint failures retry sooner with capped exponential backoff.
    pub fn with_model_reprobe_interval(mut self, interval: Duration) -> Self {
        self.rebuilds = Arc::new(FolderRebuildRegistry::with_probe_interval(interval));
        self
    }

    /// Configure automatic rebuild retries. Repeated same-class failures are
    /// halted after `max_same_class_failures` until the bulk model changes or
    /// [`Self::request_folder_rebuild`] explicitly clears the halt.
    pub fn with_model_rebuild_failure_policy(
        mut self,
        initial_backoff: Duration,
        backoff_cap: Duration,
        max_same_class_failures: u32,
    ) -> Self {
        self.rebuilds = Arc::new(self.rebuilds.with_rebuild_failure_policy(
            initial_backoff,
            backoff_cap,
            max_same_class_failures,
        ));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_model_recovery_timings_for_test(
        mut self,
        probe_interval: Duration,
        probe_failure_backoff: Duration,
        rebuild_failure_backoff: Duration,
        rebuild_failure_cap: Duration,
        max_same_class_rebuild_failures: u32,
    ) -> Self {
        self.rebuilds = Arc::new(FolderRebuildRegistry::with_timings(
            probe_interval,
            probe_failure_backoff,
            rebuild_failure_backoff,
            rebuild_failure_cap,
            max_same_class_rebuild_failures,
        ));
        self
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Open (or reuse) the folder index for `folder_id` and schedule a
    /// non-blocking model probe if embedders are wired.
    pub fn get_or_open(&self, folder_id: &str, dim: usize) -> Result<Arc<FolderIndex>> {
        self.get_or_open_with_overrides(folder_id, dim, None, None, true)
    }

    fn get_or_open_with_overrides(
        &self,
        folder_id: &str,
        dim: usize,
        short_override: Option<&Arc<EmbeddingClient>>,
        bulk_override: Option<&Arc<EmbeddingClient>>,
        allow_probe: bool,
    ) -> Result<Arc<FolderIndex>> {
        validate_folder_id(folder_id)?;
        validate_dim(dim)?;
        let short_ref = short_override.or(self.short_emb.as_ref());
        let bulk_ref = bulk_override.or(self.bulk_emb.as_ref()).or(short_ref);

        let idx = {
            let mut guard = self
                .open
                .lock()
                .map_err(|_| MakakooError::internal("folder index manager lock poisoned"))?;
            if let Some(existing) = guard.get(folder_id) {
                if existing.dim() != dim {
                    return Err(MakakooError::invalid_input(format!(
                        "folder {folder_id:?} is already open with dim {}, requested {dim}",
                        existing.dim()
                    )));
                }
                Arc::clone(existing)
            } else {
                let mut cfg = FolderIndexConfig::with_dim(dim);
                if bulk_ref.is_some() || short_ref.is_some() {
                    cfg = cfg.with_embed_aliases(
                        bulk_ref.map(|e| e.model().to_string()),
                        short_ref.map(|e| e.model().to_string()),
                    );
                }
                let opened = Arc::new(FolderIndex::open_with_config(
                    &self.base_dir,
                    folder_id,
                    cfg,
                )?);
                guard.insert(folder_id.to_string(), Arc::clone(&opened));
                opened
            }
        };

        if allow_probe && tokio::runtime::Handle::try_current().is_ok() {
            // Per-call embedders may answer this query, but only the manager's
            // configured bulk endpoint may establish index-side provenance.
            self.schedule_probe(
                &idx,
                self.short_emb.as_ref(),
                self.bulk_emb.as_ref().or(self.short_emb.as_ref()),
            );
        }
        Ok(idx)
    }

    fn schedule_probe(
        &self,
        idx: &Arc<FolderIndex>,
        short_emb: Option<&Arc<EmbeddingClient>>,
        bulk_emb: Option<&Arc<EmbeddingClient>>,
    ) {
        if let Some(bulk_emb) = bulk_emb {
            rebuild::schedule_probe(
                Arc::clone(idx),
                short_emb.cloned(),
                Arc::clone(bulk_emb),
                Arc::clone(&self.rebuilds),
            );
        }
    }

    pub async fn search_folder(
        &self,
        req: FolderSearchRequest,
        short_override: Option<&Arc<EmbeddingClient>>,
        bulk_override: Option<&Arc<EmbeddingClient>>,
    ) -> Result<FolderSearchResponse> {
        let short_ref = short_override.or(self.short_emb.as_ref()).cloned();
        let bulk_ref = bulk_override
            .or(self.bulk_emb.as_ref())
            .cloned()
            .or_else(|| short_ref.clone());
        let idx = self.get_or_open_with_overrides(
            &req.folder_id,
            req.dim,
            short_ref.as_ref(),
            bulk_ref.as_ref(),
            true,
        )?;
        rebuild::search_folder_checked(idx, short_ref, Arc::clone(&self.rebuilds), req).await
    }

    pub async fn upsert_folder_file(
        &self,
        folder_id: &str,
        dim: usize,
        req: &UpsertFileRequest,
        model_meta: Option<&UpsertModelMeta>,
    ) -> Result<UpsertOutcome> {
        let idx = self.get_or_open_with_overrides(folder_id, dim, None, None, false)?;
        let outcome = self
            .upsert_with_model_recovery(&idx, req, model_meta)
            .await?;
        self.schedule_probe_after_upsert(&idx, model_meta).await;
        Ok(outcome)
    }

    pub async fn upsert_folder_file_and_flush(
        &self,
        folder_id: &str,
        dim: usize,
        req: &UpsertFileRequest,
        model_meta: Option<&UpsertModelMeta>,
        flush: bool,
    ) -> Result<(UpsertOutcome, IndexSizeReport)> {
        let idx = self.get_or_open_with_overrides(folder_id, dim, None, None, false)?;
        let outcome = self
            .upsert_with_model_recovery(&idx, req, model_meta)
            .await?;
        self.schedule_probe_after_upsert(&idx, model_meta).await;
        if flush && !outcome.skipped_unchanged {
            idx.flush()?;
        }
        let sizes = idx.size_report()?;
        Ok((outcome, sizes))
    }

    async fn upsert_with_model_recovery(
        &self,
        idx: &Arc<FolderIndex>,
        req: &UpsertFileRequest,
        model_meta: Option<&UpsertModelMeta>,
    ) -> Result<UpsertOutcome> {
        match idx.upsert_file_with_model(req, model_meta) {
            Ok(outcome) => Ok(outcome),
            Err(MakakooError::RetryableFolderModelMismatch(_)) => {
                let Some(meta) = model_meta else {
                    return Err(MakakooError::RetryableFolderModelMismatch(
                        "unnamed upsert could not be reconciled with the configured bulk embedder"
                            .to_string(),
                    ));
                };
                self.probe_after_rejected_upsert(idx, meta).await;
                idx.upsert_file_with_model(req, model_meta)
            }
            Err(error) => Err(error),
        }
    }

    async fn probe_after_rejected_upsert(&self, idx: &Arc<FolderIndex>, meta: &UpsertModelMeta) {
        let Some(bulk_emb) = self.bulk_emb.clone().or_else(|| self.short_emb.clone()) else {
            return;
        };
        if !self
            .rebuilds
            .try_rejected_upsert_probe(idx.folder_id(), &meta.model, meta.embed_dim)
        {
            return;
        }
        rebuild::run_claimed_probe(
            Arc::clone(idx),
            self.short_emb.clone(),
            bulk_emb,
            Arc::clone(&self.rebuilds),
        )
        .await;
    }

    async fn schedule_probe_after_upsert(
        &self,
        idx: &Arc<FolderIndex>,
        model_meta: Option<&UpsertModelMeta>,
    ) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let should_expedite = model_meta.is_some_and(|meta| {
            idx.model_status().ok().is_some_and(|status| {
                (status.embed_model.as_deref() != Some(meta.model.as_str())
                    || status.embed_dim != Some(meta.embed_dim))
                    && idx.lock_conn().ok().is_some_and(|mut conn| {
                        claim_observed_bulk_probe_expedite(&mut conn, &meta.model, meta.embed_dim)
                            .unwrap_or(false)
                    })
            })
        });
        if should_expedite {
            self.rebuilds.expedite_probe(idx.folder_id());
        }
        self.schedule_probe(
            idx,
            self.short_emb.as_ref(),
            self.bulk_emb.as_ref().or(self.short_emb.as_ref()),
        );

        // A provenance-flagged ingest must not rely on a detached task that a
        // short-lived CLI/MCP process can kill at return. Await the claimed
        // probe and any rebuild this process owns. Cross-process contenders are
        // fenced by the DB lease and a later process can resume durable rows.
        if idx
            .model_status()
            .ok()
            .is_some_and(|status| status.unproven_vectors_since_gen.is_some())
        {
            self.rebuilds.wait_for_probe(idx.folder_id()).await;
            self.rebuilds.wait(idx.folder_id()).await;
        }
    }

    pub fn is_rebuilding(&self, folder_id: &str) -> bool {
        self.rebuilds.is_active(folder_id)
    }

    pub async fn wait_for_rebuild(&self, folder_id: &str) {
        self.rebuilds.wait(folder_id).await;
    }

    pub async fn wait_for_model_probe(&self, folder_id: &str) {
        self.rebuilds.wait_for_probe(folder_id).await;
    }

    /// Explicitly request a full folder rebuild. This bypasses a halt caused
    /// by repeated same-class automatic failures, while active work remains
    /// deduplicated.
    pub async fn request_folder_rebuild(
        &self,
        folder_id: &str,
        dim: usize,
    ) -> Result<FolderRebuildRequestOutcome> {
        if self.rebuilds.is_active(folder_id) {
            return Ok(FolderRebuildRequestOutcome::AlreadyRunning);
        }
        let short_emb = self.short_emb.clone();
        let bulk_emb = self
            .bulk_emb
            .clone()
            .or_else(|| short_emb.clone())
            .ok_or_else(|| MakakooError::internal("folder rebuild requires an embedder"))?;
        let idx = self.get_or_open_with_overrides(
            folder_id,
            dim,
            short_emb.as_ref(),
            Some(&bulk_emb),
            false,
        )?;
        let observed_bulk = bulk_emb.probe_model().await?;
        let observed_short = match short_emb.as_ref() {
            Some(short)
                if short.model() != bulk_emb.model() || short.base_url() != bulk_emb.base_url() =>
            {
                short.probe_model().await.ok()
            }
            Some(_) => Some(observed_bulk.clone()),
            None => None,
        };
        let status = idx.model_status()?;
        if status.index_state == super::IndexModelState::Ready
            && status.embed_model.as_deref() == Some(observed_bulk.0.as_str())
            && status.embed_dim == Some(observed_bulk.1)
            && status.unproven_vectors_since_gen.is_none()
        {
            return Ok(FolderRebuildRequestOutcome::AlreadyCurrent);
        }
        self.rebuilds.clear_rebuild_retry(&idx);
        let started = rebuild::spawn_folder_rebuild(
            idx,
            short_emb,
            bulk_emb,
            Arc::clone(&self.rebuilds),
            Some(observed_bulk),
            observed_short,
            false,
        );
        Ok(if started {
            FolderRebuildRequestOutcome::Started
        } else {
            FolderRebuildRequestOutcome::AlreadyRunning
        })
    }

    #[cfg(test)]
    pub(crate) fn advance_model_recovery_time_for_test(&self, duration: Duration) {
        self.rebuilds.advance_time(duration);
    }

    #[cfg(test)]
    pub(crate) fn get_or_open_without_probe_for_test(
        &self,
        folder_id: &str,
        dim: usize,
    ) -> Result<Arc<FolderIndex>> {
        self.get_or_open_with_overrides(folder_id, dim, None, None, false)
    }

    #[cfg(test)]
    pub(crate) async fn probe_folder_model_for_test(
        &self,
        folder_id: &str,
        dim: usize,
    ) -> Result<()> {
        let short_emb = self.short_emb.clone();
        let bulk_emb = self
            .bulk_emb
            .clone()
            .or_else(|| short_emb.clone())
            .ok_or_else(|| MakakooError::internal("folder model probe requires an embedder"))?;
        let idx = self.get_or_open_with_overrides(
            folder_id,
            dim,
            short_emb.as_ref(),
            Some(&bulk_emb),
            false,
        )?;
        let _ = rebuild::probe_and_reconcile(idx, short_emb, bulk_emb, Arc::clone(&self.rebuilds))
            .await;
        Ok(())
    }

    /// Flush all currently open folder indexes to disk.
    pub fn flush_all(&self) -> Result<()> {
        let indexes: Vec<Arc<FolderIndex>> = {
            let guard = self
                .open
                .lock()
                .map_err(|_| MakakooError::internal("folder index manager lock poisoned"))?;
            guard.values().cloned().collect()
        };
        for idx in indexes {
            idx.flush()?;
        }
        Ok(())
    }
}
