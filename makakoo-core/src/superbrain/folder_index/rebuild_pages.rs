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

/// Create (or empty) the parking table for this connection's rebuild.
pub(crate) fn reset_parked(conn: &Connection) -> Result<()> {
    // The parked vectors are the whole folder's worth: keep them on disk.
    conn.execute_batch(
        "PRAGMA temp_store = FILE;
         CREATE TEMP TABLE IF NOT EXISTS rebuild_vectors (
             id INTEGER PRIMARY KEY,
             text TEXT NOT NULL,
             vector_i8 BLOB NOT NULL
         );
         DELETE FROM temp.rebuild_vectors;",
    )?;
    Ok(())
}

/// Re-embed every chunk that is not already parked with its current text,
/// page by page from the first id, with the bulk model `(b_model, b_dim)`.
/// Fails if the endpoint answers with another model or size mid-way.
pub(crate) async fn reembed_pages(
    idx: &FolderIndex,
    bulk: &EmbeddingClient,
    b_model: &str,
    b_dim: usize,
) -> Result<()> {
    let mut after_id = 0i64;
    loop {
        let (last_id, page) = read_page(idx, after_id)?;
        let Some(last_id) = last_id else {
            return Ok(());
        };
        after_id = last_id;
        if page.is_empty() {
            continue;
        }
        let limits = bulk.request_limits();
        let mut vectors: Vec<(i64, String, Vec<i8>)> = Vec::with_capacity(page.len());
        for batch in page.chunks(limits.max_batch.max(1)) {
            let texts: Vec<String> = batch
                .iter()
                .map(|(_, t)| limits.truncate_input(t).to_string())
                .collect();
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
            for ((id, text), vec) in batch.iter().zip(res.vectors) {
                vectors.push((
                    *id,
                    text.clone(),
                    truncate_and_quantize_i8(&vec, idx.dim())?,
                ));
            }
        }
        park(idx, &vectors)?;
    }
}

/// One page of chunks above `after_id`: the last id scanned (None at the end)
/// and the rows in it that still need a vector for their current text.
/// A chunk row: its id and text.
type Chunk = (i64, String);

fn read_page(idx: &FolderIndex, after_id: i64) -> Result<(Option<i64>, Vec<Chunk>)> {
    let conn = idx.lock_conn()?;
    let rows = {
        let mut stmt = conn
            .prepare_cached("SELECT id, text FROM chunks WHERE id > ?1 ORDER BY id ASC LIMIT ?2")?;
        let collected = stmt
            .query_map(params![after_id, REBUILD_PAGE_ROWS as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        collected
    };
    let last = rows.last().map(|(id, _)| *id);
    let mut needs = Vec::new();
    {
        let mut parked =
            conn.prepare_cached("SELECT 1 FROM temp.rebuild_vectors WHERE id = ?1 AND text = ?2")?;
        for (id, text) in rows {
            if !parked.exists(params![id, text])? {
                needs.push((id, text));
            }
        }
    }
    Ok((last, needs))
}

fn park(idx: &FolderIndex, vectors: &[(i64, String, Vec<i8>)]) -> Result<()> {
    let mut conn = idx.lock_conn()?;
    let tx = conn.transaction()?;
    {
        let mut ins = tx.prepare_cached(
            "INSERT OR REPLACE INTO temp.rebuild_vectors (id, text, vector_i8) VALUES (?1, ?2, ?3)",
        )?;
        for (id, text, q) in vectors {
            ins.execute(params![id, text, i8_slice_to_bytes(q)])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Inside the commit transaction: put every parked vector on its chunk (rows
/// deleted since are skipped) and empty the parking table.
/// True when every chunk has a parked vector for its current text.
pub(crate) fn all_chunks_parked(tx: &Transaction<'_>) -> Result<bool> {
    let missing: i64 = tx.query_row(
        "SELECT COUNT(*) FROM chunks c
          WHERE NOT EXISTS (SELECT 1 FROM temp.rebuild_vectors r
                             WHERE r.id = c.id AND r.text = c.text)",
        [],
        |r| r.get(0),
    )?;
    Ok(missing == 0)
}

pub(crate) fn swap_in_parked(tx: &Transaction<'_>) -> Result<usize> {
    // Matched on id AND text: an id reused by a new chunk never takes the old
    // chunk's vector.
    let moved = tx.execute(
        "UPDATE chunks
            SET vector_i8 = (SELECT r.vector_i8 FROM temp.rebuild_vectors r
                              WHERE r.id = chunks.id AND r.text = chunks.text)
          WHERE EXISTS (SELECT 1 FROM temp.rebuild_vectors r
                         WHERE r.id = chunks.id AND r.text = chunks.text)",
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::reset_parked;
    use crate::superbrain::folder_index::FolderIndex;

    #[test]
    fn folder_index_temp_store_is_file_after_reset_parked() {
        let dir = TempDir::new().unwrap();
        let idx = FolderIndex::open(dir.path(), "temp-store-guard", 8).unwrap();
        let conn = idx.lock_conn().unwrap();

        let initial: i64 = conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            initial, 1,
            "FolderIndex must open with temp_store = FILE (1)"
        );

        conn.execute_batch("PRAGMA temp_store = MEMORY;").unwrap();
        let flipped: i64 = conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flipped, 2, "sanity check: switched to MEMORY (2)");

        reset_parked(&conn).unwrap();
        let after_reset: i64 = conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after_reset, 1,
            "reset_parked must enforce PRAGMA temp_store = FILE (1)"
        );
    }
}
