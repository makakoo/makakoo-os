//! Owner-fenced cross-process rebuild claim and lease operations for
//! `SuperbrainStore` (`brain_vectors`).
//!
//! Each rebuild claim generates a random 128-bit owner token stored in
//! `brain_vector_meta` (`rebuild_owner`) alongside a heartbeat timestamp
//! (`rebuild_heartbeat_ms`). Every heartbeat refresh, rebuild vector write,
//! completion (`finish_rebuild`), and owner-scoped stale transition verifies
//! `index_state == 'rebuilding' AND rebuild_owner == owner` inside a
//! `BEGIN IMMEDIATE` transaction. A stalled rebuild whose lease expired and
//! was taken over by another owner stops with [`MakakooError::RebuildClaimLost`]
//! without touching the new owner's vectors or state.

use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::{params, Connection, TransactionBehavior};

use crate::error::{MakakooError, Result};

use super::folder_index::model_meta::{
    IndexModelState, META_EMBED_ALIAS, META_EMBED_DIM, META_EMBED_MODEL, META_SHORT_EMBED_ALIAS,
};
use super::store::SuperbrainStore;
use super::vector_model::{
    read_index_state, read_meta_opt, write_meta_opt, write_state, write_vector_row,
};

/// A rebuild that has not refreshed its heartbeat for this long is dead (crash,
/// kill, power loss) and its claim may be taken over.
pub(crate) const REBUILD_LEASE_MS: i64 = 120_000;
pub(crate) const META_REBUILD_HEARTBEAT: &str = "rebuild_heartbeat_ms";
pub(crate) const META_REBUILD_OWNER: &str = "rebuild_owner";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_owner_token() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn touch_heartbeat(conn: &Connection) -> Result<()> {
    write_meta_opt(conn, META_REBUILD_HEARTBEAT, Some(&now_ms().to_string()))
}

fn clear_claim_meta(conn: &Connection) -> Result<()> {
    write_meta_opt(conn, META_REBUILD_OWNER, None)?;
    write_meta_opt(conn, META_REBUILD_HEARTBEAT, None)?;
    Ok(())
}

/// True while a rebuild claim is held and its heartbeat is recent.
fn lease_fresh(conn: &Connection) -> Result<bool> {
    if read_index_state(conn)? != IndexModelState::Rebuilding {
        return Ok(false);
    }
    let beat = read_meta_opt(conn, META_REBUILD_HEARTBEAT)?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    // A heartbeat in the future (clock stepped back) is not a live owner.
    let age = now_ms() - beat;
    Ok((0..REBUILD_LEASE_MS).contains(&age))
}

/// True iff the index is currently in `rebuilding` state and owned by `owner`.
fn owns_claim(conn: &Connection, owner: &str) -> Result<bool> {
    if owner.is_empty() || read_index_state(conn)? != IndexModelState::Rebuilding {
        return Ok(false);
    }
    Ok(read_meta_opt(conn, META_REBUILD_OWNER)?.as_deref() == Some(owner))
}

impl SuperbrainStore {
    /// Claim the rebuild for this database: writes `rebuilding`, a fresh random
    /// owner token, and the initial heartbeat inside one `BEGIN IMMEDIATE`
    /// transaction whenever no live rebuild holds the lease. Returns
    /// `Ok(Some(owner))` when claimed or `Ok(None)` when another rebuild holds
    /// a fresh lease.
    pub fn try_mark_vectors_rebuilding(&self, reason: &str) -> Result<Option<String>> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if lease_fresh(&tx)? {
            return Ok(None);
        }
        let owner = new_owner_token();
        write_state(&tx, IndexModelState::Rebuilding, Some(reason))?;
        write_meta_opt(&tx, META_REBUILD_OWNER, Some(&owner))?;
        touch_heartbeat(&tx)?;
        tx.commit()?;
        Ok(Some(owner))
    }

    /// Refresh the rebuild's heartbeat iff `owner` still holds the claim.
    /// Returns [`MakakooError::RebuildClaimLost`] without mutating the DB if
    /// another owner took over or the index left `rebuilding`.
    pub fn touch_rebuild_heartbeat(&self, owner: &str) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !owns_claim(&tx, owner)? {
            return Err(MakakooError::RebuildClaimLost);
        }
        touch_heartbeat(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// True while another rebuild (any process) holds a live claim.
    pub fn rebuild_lease_fresh(&self) -> Result<bool> {
        let conn = self.lock_conn()?;
        lease_fresh(&conn)
    }

    /// Store one rebuilt vector iff `owner` still holds the active rebuild
    /// claim, checked inside the same `BEGIN IMMEDIATE` transaction as the
    /// row upsert. Returns [`MakakooError::RebuildClaimLost`] if ownership
    /// changed.
    pub fn store_vector_for_rebuild(
        &self,
        owner: &str,
        doc_id: &str,
        vec: &[f32],
        model: &str,
    ) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !owns_claim(&tx, owner)? {
            return Err(MakakooError::RebuildClaimLost);
        }
        write_vector_row(&tx, doc_id, vec, model)?;
        tx.commit()?;
        Ok(())
    }

    /// Finish a rebuild iff `owner` still holds the claim: drops vectors of
    /// other models, updates active model metadata, clears the claim metadata,
    /// and marks the index `ready` in one `BEGIN IMMEDIATE` transaction.
    /// Returns [`MakakooError::RebuildClaimLost`] without mutating anything if
    /// ownership changed.
    pub fn finish_rebuild(
        &self,
        owner: &str,
        model: &str,
        dim: usize,
        bulk_alias: &str,
        short_alias: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !owns_claim(&tx, owner)? {
            return Err(MakakooError::RebuildClaimLost);
        }
        tx.execute(
            "DELETE FROM brain_vectors WHERE model IS NOT ?1 OR dim IS NOT ?2",
            params![model, dim as i64],
        )?;
        write_meta_opt(&tx, META_EMBED_MODEL, Some(model))?;
        write_meta_opt(&tx, META_EMBED_DIM, Some(&dim.to_string()))?;
        write_meta_opt(&tx, META_EMBED_ALIAS, Some(bulk_alias))?;
        if let Some(short) = short_alias {
            write_meta_opt(&tx, META_SHORT_EMBED_ALIAS, Some(short))?;
        }
        write_state(&tx, IndexModelState::Ready, None)?;
        clear_claim_meta(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Mark the index `stale` and release the claim iff `owner` still holds it.
    /// Returns `Ok(true)` if transitioned, or `Ok(false)` (leaving DB untouched)
    /// when another owner took over or the index is no longer rebuilding.
    pub fn mark_vectors_stale_for_owner(&self, owner: &str, reason: &str) -> Result<bool> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !owns_claim(&tx, owner)? {
            return Ok(false);
        }
        write_state(&tx, IndexModelState::Stale, Some(reason))?;
        clear_claim_meta(&tx)?;
        tx.commit()?;
        Ok(true)
    }
}
