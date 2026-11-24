//! Paged re-embedding for a folder rebuild.
//!
//! Rows are read one page at a time by id (keyset), re-embedded, and parked in
//! a temp table on the index's own connection (`temp_store = FILE`, so on
//! disk). Memory stays flat however large the folder is, and the connection
//! lock is held only while a page is read or parked, never across an embed
//! call. The commit moves the parked vectors onto their chunks in one short
//! transaction ([`swap_in_parked`]).

use rusqlite::{params, Connection, Transaction};

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::quantize::{i8_slice_to_bytes, truncate_and_quantize_i8};
use super::FolderIndex;

/// Chunks read per page.
pub(crate) const REBUILD_PAGE_ROWS: usize = 256;
/// Texts per embedding request.
const EMBED_BATCH: usize = 32;

/// Create (or empty) the parking table for this connection's rebuild.
pub(crate) fn reset_parked(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS rebuild_vectors (
             id INTEGER PRIMARY KEY,
             vector_i8 BLOB NOT NULL
         );
         DELETE FROM temp.rebuild_vectors;",
    )?;
    Ok(())
}

/// Re-embed every chunk with an id above `after_id`, page by page, with the
/// bulk model `(b_model, b_dim)`. Returns the last id it parked (or `after_id`
/// when there was nothing) and the number of rows parked. Fails if the
/// endpoint answers with another model or size mid-way.
pub(crate) async fn reembed_pages(
    idx: &FolderIndex,
    bulk: &EmbeddingClient,
    b_model: &str,
    b_dim: usize,
    mut after_id: i64,
) -> Result<(i64, usize)> {
    let mut parked = 0usize;
    loop {
        let page = read_page(idx, after_id)?;
        let Some(&(last_id, _)) = page.last() else {
            return Ok((after_id, parked));
        };
        let mut vectors: Vec<(i64, Vec<i8>)> = Vec::with_capacity(page.len());
        for batch in page.chunks(EMBED_BATCH) {
            let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
            let res = bulk.embed_batch_with_meta(&texts).await?;
            if res.vectors.len() != batch.len() {
                return Err(MakakooError::llm(format!(
                    "bulk rebuild expected {} vectors, got {}",
                    batch.len(),
                    res.vectors.len()
                )));
            }
            if res.model != b_model || res.dim != b_dim {
                return Err(MakakooError::llm("bulk embedder model changed mid-rebuild"));
            }
            for ((id, _), vec) in batch.iter().zip(res.vectors) {
                vectors.push((*id, truncate_and_quantize_i8(&vec, idx.dim())?));
            }
        }
        park(idx, &vectors)?;
        parked += vectors.len();
        after_id = last_id;
    }
}

fn read_page(idx: &FolderIndex, after_id: i64) -> Result<Vec<(i64, String)>> {
    let conn = idx.lock_conn()?;
    let mut stmt =
        conn.prepare_cached("SELECT id, text FROM chunks WHERE id > ?1 ORDER BY id ASC LIMIT ?2")?;
    let rows = stmt
        .query_map(params![after_id, REBUILD_PAGE_ROWS as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn park(idx: &FolderIndex, vectors: &[(i64, Vec<i8>)]) -> Result<()> {
    let mut conn = idx.lock_conn()?;
    let tx = conn.transaction()?;
    {
        let mut ins = tx.prepare_cached(
            "INSERT OR REPLACE INTO temp.rebuild_vectors (id, vector_i8) VALUES (?1, ?2)",
        )?;
        for (id, q) in vectors {
            ins.execute(params![id, i8_slice_to_bytes(q)])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Inside the commit transaction: put every parked vector on its chunk (rows
/// deleted since are skipped) and empty the parking table.
pub(crate) fn swap_in_parked(tx: &Transaction<'_>) -> Result<usize> {
    let moved = tx.execute(
        "UPDATE chunks
            SET vector_i8 = (SELECT r.vector_i8 FROM temp.rebuild_vectors r WHERE r.id = chunks.id)
          WHERE id IN (SELECT id FROM temp.rebuild_vectors)",
        [],
    )?;
    tx.execute("DELETE FROM temp.rebuild_vectors", [])?;
    Ok(moved)
}

/// Rows parked and not yet swapped in (tests: memory stays on disk).
#[cfg(test)]
pub(crate) fn parked_count(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM temp.rebuild_vectors", [], |r| {
            r.get(0)
        })?,
    )
}
