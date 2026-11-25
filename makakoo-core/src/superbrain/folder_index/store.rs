//! Per-folder index coordinator combining `chunks.sqlite` (source of truth)
//! and `vectors.usearch` (memory-mapped `i8` HNSW index).

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::error::{MakakooError, Result};

use super::model_meta::{
    init_model_meta_on_open, read_model_status, IndexModelState, UpsertModelMeta,
};
use super::quantize::{distance_to_cosine_score, quantize_chunks, truncate_and_quantize_i8};
use super::sqlite_repo::{
    bump_generation_tx, clear_file_chunks_and_upsert_state, collect_file_chunk_ids,
    delete_file_chunks_tx, fetch_chunk_row, get_file_state, init_and_verify_meta, insert_chunks_tx,
    list_file_states, open_folder_db, query_filtered_chunk_ids, read_chunk_count, read_generation,
    sqlite_files_bytes, upsert_file_state_tx, ChunkIdFilter, DB_FILE_NAME,
};
use super::vector_store::VectorStore;
use super::{
    validate_dim, validate_file_path, validate_folder_id, ChunkInput, FileStateRecord,
    FolderIndexConfig, IndexSizeReport, Passage, SearchFilter, SkipReason, SourceKind, TrustLevel,
    UpsertFileRequest, UpsertOutcome,
};

/// Compact per-folder index combining `chunks.sqlite` (source of truth) and
/// `vectors.usearch` (memory-mapped `i8` HNSW vector index).
/// Test hook type for `FolderIndex::before_rebuild_commit`.
#[cfg(test)]
pub(crate) type RebuildCommitHook = Box<dyn Fn(&FolderIndex) + Send + Sync>;

pub struct FolderIndex {
    pub(crate) dir: PathBuf,
    pub(crate) db_path: PathBuf,
    pub(crate) folder_id: String,
    pub(crate) dim: usize,
    pub(crate) conn: Mutex<Connection>,
    pub(crate) query_mismatch_reason: Mutex<Option<String>>,
    pub(crate) vectors: VectorStore,
    /// Test hook: runs before each rebuild commit attempt, lock not held.
    #[cfg(test)]
    pub(crate) before_rebuild_commit: Mutex<Option<RebuildCommitHook>>,
    /// Test hook: runs after a probe snapshot, before the Ready publication tx.
    #[cfg(test)]
    pub(crate) before_ready_publish: Mutex<Option<RebuildCommitHook>>,
    #[cfg(test)]
    pub(crate) panic_in_rebuild: AtomicBool,
    #[cfg(test)]
    pub(crate) panic_in_probe: AtomicBool,
}

impl FolderIndex {
    /// Open or create the folder index under `<base_dir>/<folder_id>/` using
    /// default batching and HNSW parameters.
    pub fn open(base_dir: &Path, folder_id: &str, dim: usize) -> Result<Self> {
        Self::open_with_config(base_dir, folder_id, FolderIndexConfig::with_dim(dim))
    }

    /// Open or create the folder index with explicit configuration.
    pub fn open_with_config(
        base_dir: &Path,
        folder_id: &str,
        cfg: FolderIndexConfig,
    ) -> Result<Self> {
        validate_folder_id(folder_id)?;
        validate_dim(cfg.dim)?;

        let dir = base_dir.join(folder_id);
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join(DB_FILE_NAME);
        let mut conn = open_folder_db(&db_path)?;

        let (sqlite_gen, sqlite_count) = init_and_verify_meta(&conn, folder_id, cfg.dim)?;
        init_model_meta_on_open(
            &mut conn,
            sqlite_count,
            cfg.embed_alias.as_deref(),
            cfg.short_embed_alias.as_deref(),
        )?;
        let status = read_model_status(&conn)?;
        let initial_meta = if status.index_state == IndexModelState::Ready {
            (status.embed_model, status.embed_dim)
        } else {
            (None, None)
        };
        let vectors = VectorStore::open_or_rebuild(
            &dir,
            cfg.clone(),
            &conn,
            sqlite_gen,
            sqlite_count,
            initial_meta,
        )?;

        Ok(Self {
            dir,
            db_path,
            folder_id: folder_id.to_string(),
            dim: cfg.dim,
            conn: Mutex::new(conn),
            query_mismatch_reason: Mutex::new(None),
            vectors,
            #[cfg(test)]
            before_rebuild_commit: Mutex::new(None),
            #[cfg(test)]
            before_ready_publish: Mutex::new(None),
            #[cfg(test)]
            panic_in_rebuild: AtomicBool::new(false),
            #[cfg(test)]
            panic_in_probe: AtomicBool::new(false),
        })
    }

    pub fn folder_id(&self) -> &str {
        &self.folder_id
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Mark the start of a bulk indexing run. Concurrent searches use the last
    /// saved `mmap` view while writes batch-flush every `flush_file_batch`
    /// files or `flush_interval` seconds.
    pub fn begin_indexing_run(&self) -> Result<()> {
        self.vectors.begin_batch_run()
    }

    /// Finish a bulk indexing run: flush pending mutations, refresh the `mmap`
    /// view, checkpoint WAL, and release the mutable in-memory index.
    pub fn finish_indexing_run(&self) -> Result<()> {
        self.vectors.finish_batch_run()?;
        self.checkpoint_wal()
    }

    /// Atomically flush pending vector mutations to disk and refresh `mmap`.
    pub fn flush(&self) -> Result<()> {
        self.vectors.flush()?;
        self.checkpoint_wal()
    }

    /// Flush pending mutations and drop the mutable in-memory index so idle
    /// heap usage returns to near zero while keeping the `mmap` view active.
    pub fn flush_and_release(&self) -> Result<()> {
        self.vectors.flush_and_release()?;
        self.checkpoint_wal()
    }

    /// Rebuild `vectors.usearch` from `chunks.sqlite` without re-embedding.
    pub fn rebuild_vector_index(&self) -> Result<()> {
        let conn = self.lock_conn()?;
        let gen = read_generation(&conn)?;
        let count = read_chunk_count(&conn)?;
        self.vectors.rebuild_from_sqlite(&conn, gen, count)
    }

    /// Add or replace all chunks for a file. Idempotent when the file is
    /// already `indexed` with the same `file_version`.
    pub fn upsert_file(&self, req: &UpsertFileRequest) -> Result<UpsertOutcome> {
        self.upsert_file_inner(req, None)
    }

    pub(crate) fn upsert_file_inner(
        &self,
        req: &UpsertFileRequest,
        model_meta: Option<&UpsertModelMeta>,
    ) -> Result<UpsertOutcome> {
        validate_file_path(&req.file_path)?;
        if req.file_version.trim().is_empty() {
            return Err(MakakooError::invalid_input(
                "file_version must be non-empty",
            ));
        }
        let quantized = quantize_chunks(&req.chunks, self.dim)?;

        let (replaced, old_ids, additions, new_gen) = {
            let mut conn = self.lock_conn()?;
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT state, file_version FROM file_states WHERE file_path = ?1",
                    params![&req.file_path],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;

            if let Some((ref state, ref ver)) = existing {
                if state == "indexed" && ver == &req.file_version {
                    return Ok(UpsertOutcome {
                        replaced: false,
                        skipped_unchanged: true,
                        chunks_written: 0,
                    });
                }
            }

            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let old_ids = collect_file_chunk_ids(&tx, &req.file_path)?;
            let had_old = !old_ids.is_empty() || existing.is_some();
            if !old_ids.is_empty() {
                delete_file_chunks_tx(&tx, &req.file_path)?;
            }
            let additions = insert_chunks_tx(
                &tx,
                &req.file_path,
                &req.file_version,
                req.source_kind,
                req.trust,
                &req.owner,
                &req.chunks,
                quantized,
            )?;
            upsert_file_state_tx(
                &tx,
                &req.file_path,
                &req.file_version,
                "indexed",
                None,
                None,
                None,
            )?;
            self.authorize_vector_write_tx(&tx, req.chunks.len(), model_meta)?;
            let new_gen = bump_generation_tx(&tx)?;
            tx.commit()?;
            (had_old, old_ids, additions, new_gen)
        };

        self.update_sidecar_if_ready();
        let chunks_written = additions.len();
        self.vectors
            .apply_delta(&old_ids, &additions, new_gen, true)?;

        Ok(UpsertOutcome {
            replaced,
            skipped_unchanged: false,
            chunks_written,
        })
    }

    /// Start incremental indexing for a file, clearing any previous chunks and
    /// recording state `indexing (0 of total)`.
    pub fn begin_file_indexing(
        &self,
        file_path: &str,
        file_version: &str,
        total: u64,
    ) -> Result<()> {
        validate_file_path(file_path)?;
        if file_version.trim().is_empty() {
            return Err(MakakooError::invalid_input(
                "file_version must be non-empty",
            ));
        }

        let (old_ids, new_gen) = clear_file_chunks_and_upsert_state(
            self.lock_conn()?,
            file_path,
            file_version,
            "indexing",
            Some(0),
            Some(total),
            None,
        )?;
        if let Some(gen) = new_gen {
            self.vectors.apply_delta(&old_ids, &[], gen, false)?;
        }
        Ok(())
    }

    /// Append a batch of chunks for an in-progress file and update progress.
    #[allow(clippy::too_many_arguments)]
    pub fn put_file_chunks_batch(
        &self,
        file_path: &str,
        file_version: &str,
        source_kind: SourceKind,
        trust: TrustLevel,
        owner: &str,
        chunks: &[ChunkInput],
        model_meta: Option<&UpsertModelMeta>,
        done: u64,
        total: u64,
    ) -> Result<usize> {
        validate_file_path(file_path)?;
        let quantized = quantize_chunks(chunks, self.dim)?;

        let (additions, new_gen) = {
            let mut conn = self.lock_conn()?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let additions = insert_chunks_tx(
                &tx,
                file_path,
                file_version,
                source_kind,
                trust,
                owner,
                chunks,
                quantized,
            )?;
            upsert_file_state_tx(
                &tx,
                file_path,
                file_version,
                "indexing",
                Some(done),
                Some(total),
                None,
            )?;
            self.authorize_vector_write_tx(&tx, chunks.len(), model_meta)?;
            let new_gen = bump_generation_tx(&tx)?;
            tx.commit()?;
            (additions, new_gen)
        };

        self.update_sidecar_if_ready();
        let written = additions.len();
        self.vectors.apply_delta(&[], &additions, new_gen, false)?;
        Ok(written)
    }

    /// Mark an incrementally indexed file as `indexed`.
    pub fn finish_file_indexing(&self, file_path: &str, file_version: &str) -> Result<()> {
        validate_file_path(file_path)?;
        let conn = self.lock_conn()?;
        upsert_file_state_tx(&conn, file_path, file_version, "indexed", None, None, None)
    }

    /// Record a file as `skipped` with an explicit [`SkipReason`] and
    /// purge any stale chunks from prior versions.
    pub fn mark_file_skipped(
        &self,
        file_path: &str,
        file_version: &str,
        reason: SkipReason,
    ) -> Result<()> {
        validate_file_path(file_path)?;
        let (old_ids, new_gen) = clear_file_chunks_and_upsert_state(
            self.lock_conn()?,
            file_path,
            file_version,
            "skipped",
            None,
            None,
            Some(reason.as_str()),
        )?;
        if let Some(gen) = new_gen {
            self.vectors.apply_delta(&old_ids, &[], gen, true)?;
        }
        Ok(())
    }

    /// Remove a file's chunks and file state from the folder index.
    pub fn remove_file(&self, file_path: &str) -> Result<bool> {
        validate_file_path(file_path)?;
        let (existed, old_ids, new_gen) = {
            let mut conn = self.lock_conn()?;
            let tx = conn.transaction()?;
            let old_ids = collect_file_chunk_ids(&tx, file_path)?;
            let deleted_chunks = if old_ids.is_empty() {
                0
            } else {
                delete_file_chunks_tx(&tx, file_path)?
            };
            let deleted_state = tx.execute(
                "DELETE FROM file_states WHERE file_path = ?1",
                params![file_path],
            )?;
            let existed = deleted_chunks > 0 || deleted_state > 0;
            let new_gen = if !old_ids.is_empty() {
                Some(bump_generation_tx(&tx)?)
            } else {
                None
            };
            tx.commit()?;
            (existed, old_ids, new_gen)
        };

        if let Some(gen) = new_gen {
            self.vectors.apply_delta(&old_ids, &[], gen, true)?;
        }
        Ok(existed)
    }

    /// Nearest-neighbour search returning passages with exact places and
    /// higher-is-better cosine similarity scores, filtered by provenance.
    /// Returns empty when the index is stale or rebuilding so vectors from
    /// different models are never compared.
    pub fn search(
        &self,
        query_vec: &[f32],
        k: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<Passage>> {
        let query_i8 = truncate_and_quantize_i8(query_vec, self.dim)?;
        if k == 0 {
            return Ok(Vec::new());
        }

        let id_filter = {
            let conn = self.lock_conn()?;
            if read_model_status(&conn)?.index_state != IndexModelState::Ready {
                return Ok(Vec::new());
            }
            if filter.is_empty() {
                None
            } else {
                let ids = query_filtered_chunk_ids(&conn, filter)?;
                if ids.is_empty() {
                    return Ok(Vec::new());
                }
                Some(ChunkIdFilter::from_ids(&ids))
            }
        };

        let raw_hits = self.vectors.search(&query_i8, k, id_filter.as_ref())?;
        if raw_hits.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.lock_conn()?;
        let mut passages = Vec::with_capacity(raw_hits.len());
        for (chunk_id, dist) in raw_hits {
            let Some(row) = fetch_chunk_row(&conn, chunk_id)? else {
                continue;
            };
            if filter.source_kind.is_some_and(|sk| sk != row.source_kind)
                || filter.trust.is_some_and(|t| t != row.trust)
                || filter.owner.as_deref().is_some_and(|o| o != row.owner)
            {
                continue;
            }
            passages.push(Passage {
                folder_id: self.folder_id.clone(),
                file_path: row.file_path,
                file_version: row.file_version,
                anchor: row.anchor,
                char_start: row.char_start,
                char_end: row.char_end,
                text: row.text,
                score: distance_to_cosine_score(dist),
                source_kind: row.source_kind,
                trust: row.trust,
            });
        }

        passages.sort_by(|a, b| b.score.total_cmp(&a.score));
        passages.truncate(k);
        Ok(passages)
    }

    /// List every file's indexing state, sorted by `file_path`.
    pub fn file_states(&self) -> Result<Vec<FileStateRecord>> {
        let conn = self.lock_conn()?;
        list_file_states(&conn)
    }

    /// Look up one file's indexing state.
    pub fn file_state(&self, file_path: &str) -> Result<Option<FileStateRecord>> {
        validate_file_path(file_path)?;
        let conn = self.lock_conn()?;
        get_file_state(&conn, file_path)
    }

    /// Report vector-index size and SQLite size separately.
    pub fn size_report(&self) -> Result<IndexSizeReport> {
        let conn = self.lock_conn()?;
        let chunk_count = read_chunk_count(&conn)?;
        let generation = read_generation(&conn)?;
        drop(conn);

        Ok(IndexSizeReport {
            vector_index_bytes: self.vectors.index_file_bytes(),
            sqlite_bytes: sqlite_files_bytes(&self.db_path),
            chunk_count,
            generation,
        })
    }

    #[cfg(test)]
    pub(crate) fn vector_disk_generation(&self) -> Option<u64> {
        self.vectors.disk_generation()
    }

    pub(crate) fn checkpoint_wal(&self) -> Result<()> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare("PRAGMA wal_checkpoint(PASSIVE)")?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
        Ok(())
    }

    pub(crate) fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| MakakooError::internal("folder index sqlite lock poisoned"))
    }
}

impl Drop for FolderIndex {
    fn drop(&mut self) {
        let _ = self.vectors.flush();
    }
}
