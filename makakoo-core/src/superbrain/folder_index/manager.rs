//! Multi-folder index manager caching open [`FolderIndex`] handles and
//! coordinating model probes and background rebuilds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::model_meta::{
    FolderSearchRequest, FolderSearchResponse, UpsertModelMeta, OBSERVED_BULK_UPSERT_REASON,
};
use super::rebuild::{self, FolderRebuildRegistry};
use super::{
    validate_dim, validate_folder_id, FolderIndex, FolderIndexConfig, IndexSizeReport,
    UpsertFileRequest, UpsertOutcome,
};

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
            self.schedule_probe(&idx, short_ref, bulk_ref);
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
        rebuild::search_folder_checked(idx, short_ref, bulk_ref, Arc::clone(&self.rebuilds), req)
            .await
    }

    pub fn upsert_folder_file(
        &self,
        folder_id: &str,
        dim: usize,
        req: &UpsertFileRequest,
        model_meta: Option<&UpsertModelMeta>,
    ) -> Result<UpsertOutcome> {
        let idx = self.get_or_open_with_overrides(folder_id, dim, None, None, false)?;
        let outcome = idx.upsert_file_with_model(req, model_meta)?;
        self.schedule_probe_after_upsert(&idx, model_meta);
        Ok(outcome)
    }

    pub fn upsert_folder_file_and_flush(
        &self,
        folder_id: &str,
        dim: usize,
        req: &UpsertFileRequest,
        model_meta: Option<&UpsertModelMeta>,
        flush: bool,
    ) -> Result<(UpsertOutcome, IndexSizeReport)> {
        let idx = self.get_or_open_with_overrides(folder_id, dim, None, None, false)?;
        let outcome = idx.upsert_file_with_model(req, model_meta)?;
        self.schedule_probe_after_upsert(&idx, model_meta);
        if flush && !outcome.skipped_unchanged {
            idx.flush()?;
        }
        let sizes = idx.size_report()?;
        Ok((outcome, sizes))
    }

    fn schedule_probe_after_upsert(
        &self,
        idx: &Arc<FolderIndex>,
        model_meta: Option<&UpsertModelMeta>,
    ) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if idx.model_status().ok().is_some_and(|status| {
            model_meta.is_some_and(|meta| {
                status.embed_model.as_deref() != Some(meta.model.as_str())
                    || status.embed_dim != Some(meta.embed_dim)
            }) || matches!(
                status.stale_reason.as_deref(),
                Some(OBSERVED_BULK_UPSERT_REASON) | Some("bulk_model_change_pending")
            )
        }) {
            self.rebuilds.expedite_probe(idx.folder_id());
        }
        self.schedule_probe(
            idx,
            self.short_emb.as_ref(),
            self.bulk_emb.as_ref().or(self.short_emb.as_ref()),
        );
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
    pub async fn request_folder_rebuild(&self, folder_id: &str, dim: usize) -> Result<()> {
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
        self.rebuilds.clear_rebuild_retry(&idx);
        rebuild::spawn_folder_rebuild(
            idx,
            short_emb,
            bulk_emb,
            Arc::clone(&self.rebuilds),
            Some(observed_bulk),
            observed_short,
        );
        Ok(())
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
