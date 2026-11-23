//! Multi-folder index manager caching open [`FolderIndex`] handles and
//! coordinating model probes and background rebuilds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::model_meta::{FolderSearchRequest, FolderSearchResponse, UpsertModelMeta};
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

        if allow_probe {
            if let Some(b_emb) = bulk_ref {
                if tokio::runtime::Handle::try_current().is_ok()
                    && idx.chunk_count().unwrap_or(0) > 0
                    && self.rebuilds.mark_probed(folder_id)
                {
                    let idx_c = Arc::clone(&idx);
                    let short_c = short_ref.cloned();
                    let bulk_c = Arc::clone(b_emb);
                    let reb_c = Arc::clone(&self.rebuilds);
                    tokio::spawn(async move {
                        rebuild::probe_and_reconcile(idx_c, short_c, bulk_c, reb_c).await;
                    });
                }
            }
        }
        Ok(idx)
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
            false,
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
        idx.upsert_file_with_model(req, model_meta)
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
        if flush && !outcome.skipped_unchanged {
            idx.flush()?;
        }
        let sizes = idx.size_report()?;
        Ok((outcome, sizes))
    }

    pub fn is_rebuilding(&self, folder_id: &str) -> bool {
        self.rebuilds.is_active(folder_id)
    }

    pub async fn wait_for_rebuild(&self, folder_id: &str) {
        self.rebuilds.wait(folder_id).await;
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
