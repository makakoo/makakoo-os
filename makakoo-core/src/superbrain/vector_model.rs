//! Model/dimension tracking and strict per-model vector isolation for
//! `SuperbrainStore` (`brain_vectors`).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::folder_index::model_meta::{
    IndexModelState, META_EMBED_ALIAS, META_EMBED_DIM, META_EMBED_MODEL, META_INDEX_STATE,
    META_SHORT_EMBED_ALIAS, META_STALE_REASON,
};
use super::store::{cosine_similarity, pack_vector, unpack_vector, SuperbrainStore, VectorHit};

pub use super::vector_rebuild::{
    check_and_probe_brain_on_open, probe_and_reconcile_brain, rebuild_brain_vectors,
    search_brain_vectors_checked, BrainRebuildCoordinator,
};

#[cfg(test)]
#[path = "vector_model_tests.rs"]
mod tests;

pub const DEFAULT_EMBED_TRUNCATE_CHARS: usize = 2000;

/// Snapshot of `brain_vector_meta` state for `SuperbrainStore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrainVectorStatus {
    pub index_state: IndexModelState,
    pub embed_model: Option<String>,
    pub embed_dim: Option<usize>,
    pub embed_alias: Option<String>,
    pub short_embed_alias: Option<String>,
    pub stale_reason: Option<String>,
}

fn read_meta_opt(conn: &Connection, key: &str) -> Result<Option<String>> {
    let val: Option<String> = conn
        .query_row(
            "SELECT value FROM brain_vector_meta WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(val.filter(|s| !s.is_empty()))
}

fn write_meta_opt(conn: &Connection, key: &str, value: Option<&str>) -> Result<()> {
    match value.filter(|s| !s.is_empty()) {
        Some(v) => {
            conn.execute(
                "INSERT OR REPLACE INTO brain_vector_meta (key, value) VALUES (?1, ?2)",
                params![key, v],
            )?;
        }
        None => {
            conn.execute("DELETE FROM brain_vector_meta WHERE key = ?1", params![key])?;
        }
    }
    Ok(())
}

fn write_state(conn: &Connection, state: IndexModelState, reason: Option<&str>) -> Result<()> {
    write_meta_opt(conn, META_INDEX_STATE, Some(state.as_str()))?;
    write_meta_opt(conn, META_STALE_REASON, reason)?;
    Ok(())
}

fn read_index_state(conn: &Connection) -> Result<IndexModelState> {
    match read_meta_opt(conn, META_INDEX_STATE)? {
        Some(raw) => IndexModelState::parse(&raw),
        None => Ok(IndexModelState::Ready),
    }
}

impl SuperbrainStore {
    pub fn vector_status(&self) -> Result<BrainVectorStatus> {
        let conn = self.lock_conn()?;
        let index_state = read_index_state(&conn)?;
        let embed_model = read_meta_opt(&conn, META_EMBED_MODEL)?;
        let embed_dim = match read_meta_opt(&conn, META_EMBED_DIM)? {
            Some(raw) => Some(
                raw.parse::<usize>()
                    .map_err(|_| MakakooError::internal(format!("corrupt embed_dim: {raw:?}")))?,
            ),
            None => None,
        };
        let embed_alias = read_meta_opt(&conn, META_EMBED_ALIAS)?;
        let short_embed_alias = read_meta_opt(&conn, META_SHORT_EMBED_ALIAS)?;
        let stale_reason = read_meta_opt(&conn, META_STALE_REASON)?;
        Ok(BrainVectorStatus {
            index_state,
            embed_model,
            embed_dim,
            embed_alias,
            short_embed_alias,
            stale_reason,
        })
    }

    pub fn is_vectors_ready(&self) -> Result<bool> {
        Ok(self.vector_status()?.index_state == IndexModelState::Ready)
    }

    pub fn mark_vectors_stale(&self, reason: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        write_state(&conn, IndexModelState::Stale, Some(reason))
    }

    /// Probe/query-time stale mark: never overwrites `rebuilding`. Read and
    /// write happen under one connection lock, the same lock a rebuild takes to
    /// set `rebuilding`, so a probe racing a rebuild start cannot hide it.
    pub fn mark_vectors_stale_unless_rebuilding(&self, reason: &str) -> Result<bool> {
        let conn = self.lock_conn()?;
        if read_index_state(&conn)? == IndexModelState::Rebuilding {
            return Ok(false);
        }
        write_state(&conn, IndexModelState::Stale, Some(reason))?;
        Ok(true)
    }

    /// Clear an alias-only stale mark: writes `ready` only while the index is
    /// still `stale` for one of `reasons`, under one lock. A rebuild that
    /// started in between keeps its `rebuilding` state.
    pub fn set_vectors_ready_if_stale_for(
        &self,
        reasons: &[&str],
        model: &str,
        dim: usize,
        bulk_alias: &str,
        short_alias: Option<&str>,
    ) -> Result<bool> {
        let conn = self.lock_conn()?;
        if read_index_state(&conn)? != IndexModelState::Stale {
            return Ok(false);
        }
        let reason = read_meta_opt(&conn, META_STALE_REASON)?;
        if !reason.as_deref().is_some_and(|r| reasons.contains(&r)) {
            return Ok(false);
        }
        write_meta_opt(&conn, META_EMBED_MODEL, Some(model))?;
        write_meta_opt(&conn, META_EMBED_DIM, Some(&dim.to_string()))?;
        write_meta_opt(&conn, META_EMBED_ALIAS, Some(bulk_alias))?;
        if let Some(short) = short_alias {
            write_meta_opt(&conn, META_SHORT_EMBED_ALIAS, Some(short))?;
        }
        write_state(&conn, IndexModelState::Ready, None)?;
        Ok(true)
    }

    pub fn mark_vectors_rebuilding(&self, reason: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        write_state(&conn, IndexModelState::Rebuilding, Some(reason))
    }

    pub fn set_vectors_ready(
        &self,
        model: &str,
        dim: usize,
        bulk_alias: &str,
        short_alias: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock_conn()?;
        write_meta_opt(&conn, META_EMBED_MODEL, Some(model))?;
        write_meta_opt(&conn, META_EMBED_DIM, Some(&dim.to_string()))?;
        write_meta_opt(&conn, META_EMBED_ALIAS, Some(bulk_alias))?;
        if let Some(short) = short_alias {
            write_meta_opt(&conn, META_SHORT_EMBED_ALIAS, Some(short))?;
        }
        write_state(&conn, IndexModelState::Ready, None)
    }

    /// Store (or replace) the embedding for `doc_id` with explicit model id.
    pub fn store_vector_with_model(&self, doc_id: &str, vec: &[f32], model: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        let row_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM brain_docs WHERE path = ?1",
                params![doc_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let id = row_id
            .ok_or_else(|| MakakooError::NotFound(format!("brain_docs row for doc_id={doc_id}")))?;
        let blob = pack_vector(vec);
        let dim = vec.len() as i64;
        conn.execute(
            "INSERT INTO brain_vectors (doc_id, embedding, dim, model, created_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(doc_id) DO UPDATE SET
                 embedding = excluded.embedding,
                 dim = excluded.dim,
                 model = excluded.model,
                 created_at = excluded.created_at",
            params![id, blob, dim, model],
        )?;
        Ok(())
    }

    /// Brute-force cosine similarity search restricted strictly to `(model, query_vec.len())`.
    /// Returns empty whenever `index_state != Ready` or the active model mismatches.
    pub fn vector_search_for_model(
        &self,
        query_vec: &[f32],
        model: &str,
        limit: usize,
    ) -> Result<Vec<VectorHit>> {
        let status = self.vector_status()?;
        if status.index_state != IndexModelState::Ready {
            return Ok(Vec::new());
        }
        if let Some(active) = status.embed_model.as_deref() {
            if active != model {
                return Ok(Vec::new());
            }
        }
        if let Some(active_dim) = status.embed_dim {
            if active_dim != query_vec.len() {
                return Ok(Vec::new());
            }
        }

        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT d.path, d.content, v.embedding
             FROM brain_vectors v
             JOIN brain_docs d ON v.doc_id = d.id
             WHERE v.model = ?1 AND v.dim = ?2",
        )?;
        let rows = stmt.query_map(params![model, query_vec.len() as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;

        let mut scored: Vec<VectorHit> = Vec::new();
        for row in rows {
            let (path, content, blob) = row?;
            let stored = unpack_vector(&blob);
            let sim = cosine_similarity(query_vec, &stored);
            if sim > 0.3 {
                scored.push(VectorHit {
                    doc_id: path,
                    similarity: sim,
                    content,
                });
            }
        }
        scored.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    /// Documents missing a vector OR carrying a vector from a different `(model, dim)`.
    pub fn docs_needing_vectors(
        &self,
        model: &str,
        dim: usize,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT d.path, d.content
             FROM brain_docs d
             LEFT JOIN brain_vectors v ON v.doc_id = d.id
             WHERE v.doc_id IS NULL OR v.model != ?1 OR v.dim != ?2
             ORDER BY d.updated_at DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![model, dim as i64, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Compare stored aliases on open and mark stale if configured aliases changed
    /// or legacy vectors lack model metadata.
    pub fn check_brain_alias_on_open(
        &self,
        bulk_alias: &str,
        short_alias: Option<&str>,
    ) -> Result<BrainVectorStatus> {
        let vector_count = self.stats()?.vector_count;
        let status = self.vector_status()?;
        if vector_count == 0 {
            if status.index_state != IndexModelState::Ready || status.stale_reason.is_some() {
                let conn = self.lock_conn()?;
                write_state(&conn, IndexModelState::Ready, None)?;
            }
            return self.vector_status();
        }

        let (has_unknown, distinct_models): (i64, i64) = {
            let conn = self.lock_conn()?;
            conn.query_row(
                "SELECT
                     SUM(CASE WHEN model IS NULL OR model = 'unknown' THEN 1 ELSE 0 END),
                     COUNT(DISTINCT COALESCE(model, 'unknown'))
                 FROM brain_vectors",
                [],
                |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get(1)?)),
            )?
        };
        if has_unknown > 0
            || distinct_models > 1
            || status.embed_model.is_none()
            || status.embed_dim.is_none()
        {
            self.mark_vectors_stale("missing_embed_model_metadata")?;
        } else if status.index_state == IndexModelState::Rebuilding {
            self.mark_vectors_stale("interrupted_rebuild")?;
        } else if status.embed_alias.as_deref() != Some(bulk_alias) {
            self.mark_vectors_stale("embed_alias_changed")?;
        } else if let (Some(prev), Some(cur)) = (status.short_embed_alias.as_deref(), short_alias) {
            if prev != cur {
                self.mark_vectors_stale("short_embed_alias_changed")?;
            }
        }
        self.vector_status()
    }

    /// Embed up to `limit` pending/mismatched documents, recording the endpoint's
    /// reported `model` and `dim` and updating `brain_vector_meta`.
    pub async fn sync_vectors_with_embedder(
        &self,
        embedder: &EmbeddingClient,
        short_alias: Option<&str>,
        limit: usize,
        truncate_chars: usize,
    ) -> Result<usize> {
        if limit == 0 {
            return Ok(0);
        }
        let missing = self.docs_missing_vectors(limit)?;
        let mut written = 0usize;
        let mut target_model: Option<String> = None;
        let mut target_dim: Option<usize> = None;

        for (doc_id, content) in missing {
            let truncated: String = content.chars().take(truncate_chars).collect();
            match embedder.embed_with_meta(&truncated).await {
                Ok(res) if !res.vector.is_empty() => {
                    target_model = Some(res.model.clone());
                    target_dim = Some(res.dim);
                    if self
                        .store_vector_with_model(&doc_id, &res.vector, &res.model)
                        .is_ok()
                    {
                        written += 1;
                    }
                }
                _ => continue,
            }
        }

        if target_model.is_none() {
            if self.count()? == 0 {
                return Ok(0);
            }
            let (m, d) = embedder.probe_model().await?;
            target_model = Some(m);
            target_dim = Some(d);
        }

        let (Some(model), Some(dim)) = (target_model, target_dim) else {
            return Ok(written);
        };

        let remaining_budget = limit.saturating_sub(written);
        if remaining_budget > 0 {
            let stale_docs = self.docs_needing_vectors(&model, dim, remaining_budget)?;
            if !stale_docs.is_empty() {
                let status = self.vector_status()?;
                if status.embed_model.as_deref() != Some(&model) || status.embed_dim != Some(dim) {
                    let _ = self.mark_vectors_stale("model_rebuild_in_progress");
                }
                for (doc_id, content) in stale_docs {
                    let truncated: String = content.chars().take(truncate_chars).collect();
                    match embedder.embed_with_meta(&truncated).await {
                        Ok(res) if !res.vector.is_empty() => {
                            if self
                                .store_vector_with_model(&doc_id, &res.vector, &res.model)
                                .is_ok()
                            {
                                written += 1;
                            }
                        }
                        _ => continue,
                    }
                }
            }
        }

        if self.docs_needing_vectors(&model, dim, 1)?.is_empty() {
            self.delete_vectors_not_matching(&model, dim)?;
            self.set_vectors_ready(&model, dim, embedder.model(), short_alias)?;
        }
        Ok(written)
    }

    pub(crate) fn delete_vectors_not_matching(&self, model: &str, dim: usize) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM brain_vectors WHERE model != ?1 OR dim != ?2",
            params![model, dim as i64],
        )?;
        Ok(())
    }
}
