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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::{params, Connection, TransactionBehavior};
use tokio::sync::Notify;

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
pub const REBUILD_LEASE_MS: i64 = 120_000;
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

pub(crate) fn clear_claim_meta(conn: &Connection) -> Result<()> {
    write_meta_opt(conn, META_REBUILD_OWNER, None)?;
    write_meta_opt(conn, META_REBUILD_HEARTBEAT, None)?;
    Ok(())
}

/// True while a rebuild claim is held and its heartbeat is within one lease of
/// `now_ms()` (`-LEASE < age < LEASE`). Accepting a small negative age avoids
/// immediately stealing a live rebuild when NTP or a VM resume steps the wall
/// clock backward within one lease window.
pub(crate) fn lease_fresh(conn: &Connection, lease_ms: i64) -> Result<bool> {
    if read_index_state(conn)? != IndexModelState::Rebuilding {
        return Ok(false);
    }
    let beat = read_meta_opt(conn, META_REBUILD_HEARTBEAT)?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let age = now_ms().saturating_sub(beat);
    Ok(age > -lease_ms && age < lease_ms)
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
        let lease_ms = self.rebuild_lease_ms();
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if lease_fresh(&tx, lease_ms)? {
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
        let lease_ms = self.rebuild_lease_ms();
        let conn = self.lock_conn()?;
        lease_fresh(&conn, lease_ms)
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

/// Concurrent heartbeat ticker running at `LEASE / 4` (30 s in production) for
/// the full owner scope of a rebuild. Stops on drop or when ownership is lost.
pub(crate) struct HeartbeatGuard {
    handle: tokio::task::JoinHandle<()>,
    lost_flag: Arc<AtomicBool>,
    lost_notify: Arc<Notify>,
}

impl HeartbeatGuard {
    pub(crate) fn start(store: Arc<SuperbrainStore>, owner: String) -> Self {
        let lease_ms = store.rebuild_lease_ms();
        let interval_dur = Duration::from_millis((lease_ms / 4).max(1) as u64);
        let lost_flag = Arc::new(AtomicBool::new(false));
        let lost_notify = Arc::new(Notify::new());
        let flag_task = Arc::clone(&lost_flag);
        let notify_task = Arc::clone(&lost_notify);
        let handle = tokio::spawn(async move {
            let start = tokio::time::Instant::now() + interval_dur;
            let mut ticker = tokio::time::interval_at(start, interval_dur);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let s = Arc::clone(&store);
                let o = owner.clone();
                match tokio::task::spawn_blocking(move || s.touch_rebuild_heartbeat(&o)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(MakakooError::RebuildClaimLost)) => {
                        flag_task.store(true, Ordering::SeqCst);
                        notify_task.notify_waiters();
                        break;
                    }
                    Ok(Err(err)) => {
                        tracing::warn!(error = %err, "brain rebuild heartbeat touch failed");
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "brain rebuild heartbeat task failed");
                    }
                }
            }
        });
        Self {
            handle,
            lost_flag,
            lost_notify,
        }
    }

    pub(crate) async fn claim_lost(&self) {
        let notified = self.lost_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.lost_flag.load(Ordering::SeqCst) {
            notified.await;
        }
    }
}

impl Drop for HeartbeatGuard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Mark the index `stale` for `owner`, retrying once after a brief backoff on
/// transient failure (e.g. `SQLITE_BUSY`) and logging at `warn` if both
/// attempts fail.
pub(crate) async fn mark_stale_for_owner_with_retry(
    store: &Arc<SuperbrainStore>,
    owner: &str,
    reason: &str,
) -> bool {
    for attempt in 0..2 {
        let s = Arc::clone(store);
        let o = owner.to_string();
        let r = reason.to_string();
        match tokio::task::spawn_blocking(move || s.mark_vectors_stale_for_owner(&o, &r)).await {
            Ok(Ok(changed)) => return changed,
            Ok(Err(err)) => {
                if attempt == 0 {
                    tracing::debug!(
                        error = %err,
                        reason,
                        "brain vector stale mark failed on first attempt; retrying once"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                } else {
                    tracing::warn!(
                        error = %err,
                        reason,
                        "failed to mark brain vectors stale after retry"
                    );
                }
            }
            Err(err) => {
                if attempt == 0 {
                    tracing::debug!(
                        error = %err,
                        reason,
                        "brain vector stale mark task failed on first attempt; retrying once"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                } else {
                    tracing::warn!(
                        error = %err,
                        reason,
                        "failed to mark brain vectors stale after retry"
                    );
                }
            }
        }
    }
    false
}
