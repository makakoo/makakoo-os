//! Paged re-embedding for a folder rebuild.
//!
//! Rows are read one page at a time by id (keyset), re-embedded, and parked in
//! a durable SQLite table keyed by the fenced rebuild claim. Memory stays flat
//! however large the folder is, interrupted processes resume parked rows, and
//! the connection
//! lock is held only while a page is read or parked, never across an embed
//! call. The commit moves the parked vectors onto their chunks in one short
//! transaction ([`swap_in_parked`]).

use rusqlite::{params, Connection, Transaction, TransactionBehavior};

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::model_meta::{folder_rebuild_claim_owned, FolderRebuildClaim};
use super::quantize::{i8_slice_to_bytes, truncate_and_quantize_i8};
use super::FolderIndex;

/// Chunks read per page.
pub(crate) const REBUILD_PAGE_ROWS: usize = 256;

/// Retain resumable rows for this claim/model and discard incompatible rows.
pub(crate) fn prepare_parked(conn: &Connection, claim: &FolderRebuildClaim) -> Result<()> {
    conn.execute(
        "DELETE FROM rebuild_vectors
          WHERE claim_token = ?1 AND (model <> ?2 OR dim <> ?3)",
        params![&claim.token, &claim.model, claim.dim as i64],
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
    claim: &FolderRebuildClaim,
) -> Result<()> {
    let mut after_id = 0i64;
    loop {
        let (last_id, page) = read_page(idx, after_id, claim)?;
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
        park(idx, claim, &vectors)?;
    }
}

/// One page of chunks above `after_id`: the last id scanned (None at the end)
/// and the rows in it that still need a vector for their current text.
/// A chunk row: its id and text.
type Chunk = (i64, String);

fn read_page(
    idx: &FolderIndex,
    after_id: i64,
    claim: &FolderRebuildClaim,
) -> Result<(Option<i64>, Vec<Chunk>)> {
    let conn = idx.lock_conn()?;
    if !folder_rebuild_claim_owned(&conn, claim)? {
        return Err(MakakooError::RebuildClaimLost);
    }
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
        let mut parked = conn.prepare_cached(
            "SELECT 1 FROM rebuild_vectors
              WHERE claim_token = ?1 AND model = ?2 AND dim = ?3
                AND chunk_id = ?4 AND text = ?5",
        )?;
        for (id, text) in rows {
            if !parked.exists(params![
                &claim.token,
                &claim.model,
                claim.dim as i64,
                id,
                &text
            ])? {
                needs.push((id, text));
            }
        }
    }
    Ok((last, needs))
}

fn park(
    idx: &FolderIndex,
    claim: &FolderRebuildClaim,
    vectors: &[(i64, String, Vec<i8>)],
) -> Result<()> {
    let mut conn = idx.lock_conn()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !folder_rebuild_claim_owned(&tx, claim)? {
        return Err(MakakooError::RebuildClaimLost);
    }
    {
        let mut ins = tx.prepare_cached(
            "INSERT OR REPLACE INTO rebuild_vectors
                (claim_token, model, dim, chunk_id, text, vector_i8)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (id, text, q) in vectors {
            ins.execute(params![
                &claim.token,
                &claim.model,
                claim.dim as i64,
                id,
                text,
                i8_slice_to_bytes(q)
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Inside the commit transaction: put every parked vector on its chunk (rows
/// deleted since are skipped) and empty the parking table.
/// True when every chunk has a parked vector for its current text.
pub(crate) fn all_chunks_parked(tx: &Transaction<'_>, claim: &FolderRebuildClaim) -> Result<bool> {
    let missing: i64 = tx.query_row(
        "SELECT COUNT(*) FROM chunks c
          WHERE NOT EXISTS (SELECT 1 FROM rebuild_vectors r
                             WHERE r.claim_token = ?1 AND r.model = ?2 AND r.dim = ?3
                               AND r.chunk_id = c.id AND r.text = c.text)",
        params![&claim.token, &claim.model, claim.dim as i64],
        |r| r.get(0),
    )?;
    Ok(missing == 0)
}

pub(crate) fn swap_in_parked(tx: &Transaction<'_>, claim: &FolderRebuildClaim) -> Result<usize> {
    // Matched on id AND text: an id reused by a new chunk never takes the old
    // chunk's vector.
    let moved = tx.execute(
        "UPDATE chunks
            SET vector_i8 = (SELECT r.vector_i8 FROM rebuild_vectors r
                              WHERE r.claim_token = ?1 AND r.model = ?2 AND r.dim = ?3
                                AND r.chunk_id = chunks.id AND r.text = chunks.text)
          WHERE EXISTS (SELECT 1 FROM rebuild_vectors r
                         WHERE r.claim_token = ?1 AND r.model = ?2 AND r.dim = ?3
                           AND r.chunk_id = chunks.id AND r.text = chunks.text)",
        params![&claim.token, &claim.model, claim.dim as i64],
    )?;
    tx.execute(
        "DELETE FROM rebuild_vectors WHERE claim_token = ?1",
        params![&claim.token],
    )?;
    Ok(moved)
}

/// Rows parked and not yet swapped in (tests: memory stays on disk).
#[cfg(test)]
pub(crate) fn parked_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM rebuild_vectors", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::prepare_parked;
    use crate::superbrain::folder_index::model_meta::FolderRebuildClaim;
    use crate::superbrain::folder_index::FolderIndex;

    #[test]
    fn folder_index_durable_parking_survives_prepare() {
        let dir = TempDir::new().unwrap();
        let idx = FolderIndex::open(dir.path(), "temp-store-guard", 8).unwrap();
        let conn = idx.lock_conn().unwrap();

        let claim = FolderRebuildClaim {
            token: "claim-a".into(),
            owner: "owner-a".into(),
            model: "model-a".into(),
            dim: 8,
            lease_expires_unix_ms: u64::MAX,
        };
        conn.execute(
            "INSERT INTO rebuild_vectors
                (claim_token, model, dim, chunk_id, text, vector_i8)
             VALUES ('claim-a', 'model-a', 8, 1, 'text', X'01')",
            [],
        )
        .unwrap();
        prepare_parked(&conn, &claim).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM rebuild_vectors", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "resumable parked row was reset");
    }
}
