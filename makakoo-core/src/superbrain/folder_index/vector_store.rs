//! Compact `i8` USearch vector store with atomic batched persistence,
//! read-only `mmap` search views, and generation-checked rebuilds from SQLite.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

use crate::error::{MakakooError, Result};

use super::quantize::bytes_to_i8_vec;
use super::sqlite_repo::ChunkIdFilter;
use super::FolderIndexConfig;

static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

const INDEX_FILE_NAME: &str = "vectors.usearch";
const GEN_FILE_NAME: &str = "vectors.gen";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct VectorSidecar {
    generation: u64,
    dim: usize,
    count: u64,
}

struct WriterState {
    mutable: Option<Index>,
    slot_watermark: usize,
    dirty_files: usize,
    dirty_ops: usize,
    pending_generation: u64,
    last_flush: Instant,
    in_batch_run: bool,
}

pub(crate) struct VectorStore {
    dir: PathBuf,
    index_path: PathBuf,
    gen_path: PathBuf,
    cfg: FolderIndexConfig,
    view: RwLock<Option<Arc<Index>>>,
    writer: Mutex<WriterState>,
}

impl VectorStore {
    pub(crate) fn open_or_rebuild(
        dir: &Path,
        cfg: FolderIndexConfig,
        conn: &Connection,
        sqlite_gen: u64,
        sqlite_count: u64,
    ) -> Result<Self> {
        let store = Self {
            dir: dir.to_path_buf(),
            index_path: dir.join(INDEX_FILE_NAME),
            gen_path: dir.join(GEN_FILE_NAME),
            cfg,
            view: RwLock::new(None),
            writer: Mutex::new(WriterState {
                mutable: None,
                slot_watermark: 0,
                dirty_files: 0,
                dirty_ops: 0,
                pending_generation: sqlite_gen,
                last_flush: Instant::now(),
                in_batch_run: false,
            }),
        };

        if store.needs_rebuild(sqlite_gen, sqlite_count) {
            store.rebuild_from_sqlite(conn, sqlite_gen, sqlite_count)?;
        } else if sqlite_count > 0 {
            store.reload_mmap_view()?;
        }

        Ok(store)
    }

    pub(crate) fn index_file_bytes(&self) -> u64 {
        std::fs::metadata(&self.index_path)
            .map(|m| m.len())
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn disk_generation(&self) -> Option<u64> {
        self.read_sidecar().map(|s| s.generation)
    }

    pub(crate) fn begin_batch_run(&self) -> Result<()> {
        let mut w = self.lock_writer()?;
        w.in_batch_run = true;
        Ok(())
    }

    pub(crate) fn finish_batch_run(&self) -> Result<()> {
        let mut w = self.lock_writer()?;
        w.in_batch_run = false;
        if w.dirty_ops > 0 {
            self.persist_locked(&mut w)?;
        }
        w.mutable = None;
        w.slot_watermark = 0;
        Ok(())
    }

    pub(crate) fn flush(&self) -> Result<()> {
        let mut w = self.lock_writer()?;
        if w.dirty_ops > 0 {
            self.persist_locked(&mut w)?;
        }
        Ok(())
    }

    pub(crate) fn flush_and_release(&self) -> Result<()> {
        let mut w = self.lock_writer()?;
        if w.dirty_ops > 0 {
            self.persist_locked(&mut w)?;
        }
        w.mutable = None;
        w.slot_watermark = 0;
        Ok(())
    }

    /// Apply removals and additions to the single in-memory mutable index,
    /// persisting atomically when the batch file count or elapsed interval
    /// threshold is reached.
    pub(crate) fn apply_delta(
        &self,
        remove_ids: &[u64],
        additions: &[(u64, Vec<i8>)],
        new_generation: u64,
        count_as_file: bool,
    ) -> Result<()> {
        if remove_ids.is_empty() && additions.is_empty() {
            return Ok(());
        }
        let mut w = self.lock_writer()?;
        self.ensure_mutable_loaded(&mut w)?;

        {
            let idx = w
                .mutable
                .as_ref()
                .ok_or_else(|| MakakooError::internal("mutable vector index missing"))?;
            for &id in remove_ids {
                if idx.contains(id) {
                    idx.remove(id).map_err(usearch_err)?;
                }
            }
        }

        if !additions.is_empty() {
            let needed = w.slot_watermark.saturating_add(additions.len());
            let idx = w
                .mutable
                .as_ref()
                .ok_or_else(|| MakakooError::internal("mutable vector index missing"))?;
            if idx.capacity() < needed {
                let current_cap = idx.capacity();
                // 1.25x growth avoids doubling memory near 200k chunks.
                let grown = needed.max(current_cap + current_cap / 4 + 256);
                idx.reserve_capacity_and_threads(grown, 1)
                    .map_err(usearch_err)?;
            }
            for (id, vec_i8) in additions {
                idx.add(*id, vec_i8.as_slice()).map_err(usearch_err)?;
            }
            w.slot_watermark = needed;
        }

        w.pending_generation = new_generation;
        w.dirty_ops = w.dirty_ops.saturating_add(1);
        if count_as_file {
            w.dirty_files = w.dirty_files.saturating_add(1);
        }

        let batch_due = self.cfg.flush_file_batch > 0 && w.dirty_files >= self.cfg.flush_file_batch;
        let time_due = self.cfg.flush_interval > Duration::ZERO
            && w.last_flush.elapsed() >= self.cfg.flush_interval;
        if batch_due || time_due {
            self.persist_locked(&mut w)?;
        }
        Ok(())
    }

    /// Nearest-neighbour search returning `(chunk_id, cosine_distance)` pairs.
    pub(crate) fn search(
        &self,
        query: &[i8],
        k: usize,
        filter: Option<&ChunkIdFilter>,
    ) -> Result<Vec<(u64, f32)>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let use_mutable = {
            let w = self.lock_writer()?;
            !w.in_batch_run && w.dirty_ops > 0 && w.mutable.is_some()
        };
        if use_mutable {
            let w = self.lock_writer()?;
            if let Some(ref idx) = w.mutable {
                return run_index_search(idx, query, k, filter);
            }
        }
        let view_arc = self
            .view
            .read()
            .map_err(|_| MakakooError::internal("vector view lock poisoned"))?
            .clone();
        match view_arc {
            Some(idx) => run_index_search(&idx, query, k, filter),
            None => Ok(Vec::new()),
        }
    }

    /// Deterministically rebuild `vectors.usearch` by streaming `i8` vectors
    /// from SQLite in O(1) extra memory, save atomically, and open as `mmap`.
    pub(crate) fn rebuild_from_sqlite(
        &self,
        conn: &Connection,
        sqlite_gen: u64,
        sqlite_count: u64,
    ) -> Result<()> {
        let mut w = self.lock_writer()?;
        if sqlite_count == 0 {
            w.mutable = None;
            w.slot_watermark = 0;
            w.dirty_files = 0;
            w.dirty_ops = 0;
            w.pending_generation = sqlite_gen;
            w.last_flush = Instant::now();
            self.write_empty_disk_state(sqlite_gen)?;
            return Ok(());
        }

        let idx = self.new_empty_index()?;
        let count_usize = usize::try_from(sqlite_count)
            .map_err(|_| MakakooError::internal("chunk count overflows usize"))?;
        idx.reserve_capacity_and_threads(count_usize, 1)
            .map_err(usearch_err)?;

        let mut stmt = conn.prepare("SELECT id, vector_i8 FROM chunks ORDER BY id ASC")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id_i64: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            if blob.len() != self.cfg.dim {
                return Err(MakakooError::internal(format!(
                    "corrupt vector_i8 length {} for chunk {} (expected {})",
                    blob.len(),
                    id_i64,
                    self.cfg.dim
                )));
            }
            let vec_i8 = bytes_to_i8_vec(&blob);
            idx.add(id_i64 as u64, vec_i8.as_slice())
                .map_err(usearch_err)?;
        }

        self.atomic_save_index(&idx, sqlite_gen)?;
        drop(idx);

        w.mutable = None;
        w.slot_watermark = 0;
        w.dirty_files = 0;
        w.dirty_ops = 0;
        w.pending_generation = sqlite_gen;
        w.last_flush = Instant::now();
        Ok(())
    }

    fn needs_rebuild(&self, sqlite_gen: u64, sqlite_count: u64) -> bool {
        if sqlite_gen == 0 && sqlite_count == 0 {
            return false;
        }
        let Some(sidecar) = self.read_sidecar() else {
            return true;
        };
        if sidecar.generation != sqlite_gen
            || sidecar.dim != self.cfg.dim
            || sidecar.count != sqlite_count
        {
            return true;
        }
        if sqlite_count == 0 {
            return false;
        }
        let Some(path_str) = self.index_path.to_str() else {
            return true;
        };
        if !self.index_path.is_file() {
            return true;
        }
        match Index::metadata(path_str) {
            Ok(meta) => {
                meta.dimensions as usize != self.cfg.dim
                    || meta.quantization != ScalarKind::I8
                    || meta.metric != MetricKind::Cos
            }
            Err(_) => true,
        }
    }

    fn ensure_mutable_loaded(&self, w: &mut WriterState) -> Result<()> {
        if w.mutable.is_some() {
            return Ok(());
        }
        if self.index_path.is_file() {
            let path_str = path_to_str(&self.index_path)?;
            let meta = Index::metadata(path_str).map_err(usearch_err)?;
            let idx = Index::restore(path_str).map_err(usearch_err)?;
            idx.change_expansion_add(self.cfg.expansion_add);
            idx.change_expansion_search(self.cfg.expansion_search);
            w.slot_watermark = ((meta.count_present + meta.count_deleted) as usize).max(idx.size());
            w.mutable = Some(idx);
        } else {
            w.mutable = Some(self.new_empty_index()?);
            w.slot_watermark = 0;
        }
        Ok(())
    }

    fn persist_locked(&self, w: &mut WriterState) -> Result<()> {
        let gen = w.pending_generation;
        match w.mutable.as_ref() {
            Some(idx) if idx.size() > 0 => self.atomic_save_index(idx, gen)?,
            _ => self.write_empty_disk_state(gen)?,
        }
        w.dirty_files = 0;
        w.dirty_ops = 0;
        w.last_flush = Instant::now();
        Ok(())
    }

    fn atomic_save_index(&self, idx: &Index, generation: u64) -> Result<()> {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_index = self.dir.join(format!("{INDEX_FILE_NAME}.tmp.{pid}.{seq}"));
        if let Err(e) = idx.save(path_to_str(&tmp_index)?) {
            let _ = std::fs::remove_file(&tmp_index);
            return Err(usearch_err(e));
        }

        // Drop active mmap view BEFORE renaming (required on Windows).
        let mut view_guard = self
            .view
            .write()
            .map_err(|_| MakakooError::internal("vector view lock poisoned"))?;
        *view_guard = None;

        if let Err(e) = std::fs::rename(&tmp_index, &self.index_path) {
            let _ = std::fs::remove_file(&tmp_index);
            return Err(MakakooError::Io(e));
        }

        self.write_sidecar_atomic(&VectorSidecar {
            generation,
            dim: self.cfg.dim,
            count: idx.size() as u64,
        })?;

        let viewed = Index::restore_view(path_to_str(&self.index_path)?).map_err(usearch_err)?;
        viewed.change_expansion_search(self.cfg.expansion_search);
        *view_guard = Some(Arc::new(viewed));
        Ok(())
    }

    fn write_empty_disk_state(&self, generation: u64) -> Result<()> {
        let mut view_guard = self
            .view
            .write()
            .map_err(|_| MakakooError::internal("vector view lock poisoned"))?;
        *view_guard = None;
        if self.index_path.exists() {
            let _ = std::fs::remove_file(&self.index_path);
        }
        self.write_sidecar_atomic(&VectorSidecar {
            generation,
            dim: self.cfg.dim,
            count: 0,
        })
    }

    fn reload_mmap_view(&self) -> Result<()> {
        let mut view_guard = self
            .view
            .write()
            .map_err(|_| MakakooError::internal("vector view lock poisoned"))?;
        let viewed = Index::restore_view(path_to_str(&self.index_path)?).map_err(usearch_err)?;
        viewed.change_expansion_search(self.cfg.expansion_search);
        *view_guard = Some(Arc::new(viewed));
        Ok(())
    }

    fn new_empty_index(&self) -> Result<Index> {
        Index::new(&IndexOptions {
            dimensions: self.cfg.dim,
            metric: MetricKind::Cos,
            quantization: ScalarKind::I8,
            connectivity: self.cfg.connectivity,
            expansion_add: self.cfg.expansion_add,
            expansion_search: self.cfg.expansion_search,
            multi: false,
        })
        .map_err(usearch_err)
    }

    fn read_sidecar(&self) -> Option<VectorSidecar> {
        let raw = std::fs::read_to_string(&self.gen_path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn write_sidecar_atomic(&self, sidecar: &VectorSidecar) -> Result<()> {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp = self.dir.join(format!("{GEN_FILE_NAME}.tmp.{pid}.{seq}"));
        let payload = serde_json::to_vec(sidecar)?;
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&payload)?;
            f.sync_all()?;
        }
        if let Err(e) = std::fs::rename(&tmp, &self.gen_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(MakakooError::Io(e));
        }
        Ok(())
    }

    fn lock_writer(&self) -> Result<std::sync::MutexGuard<'_, WriterState>> {
        self.writer
            .lock()
            .map_err(|_| MakakooError::internal("vector writer lock poisoned"))
    }
}

fn run_index_search(
    idx: &Index,
    query: &[i8],
    k: usize,
    filter: Option<&ChunkIdFilter>,
) -> Result<Vec<(u64, f32)>> {
    if idx.size() == 0 {
        return Ok(Vec::new());
    }
    let matches = match filter {
        Some(f) => idx
            .filtered_search(query, k, |key| f.contains(key))
            .map_err(usearch_err)?,
        None => idx.search(query, k).map_err(usearch_err)?,
    };
    Ok(matches.keys.into_iter().zip(matches.distances).collect())
}

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| MakakooError::invalid_input("non-UTF-8 path not supported by vector index"))
}

fn usearch_err(err: impl std::fmt::Display) -> MakakooError {
    MakakooError::internal(format!("usearch error: {err}"))
}
