//! Background re-embedding rebuild orchestration and model-checked search
//! with FTS fallback for [`super::store::FolderIndex`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Notify;

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};

use super::model_meta::{
    clear_persisted_rebuild_retry, read_model_status, read_persisted_automatic_rebuild_success,
    read_persisted_rebuild_retry, record_observed_bulk, truncate_utf8, write_model_state,
    write_persisted_automatic_rebuild_success, write_persisted_rebuild_retry,
    write_ready_model_meta, FolderSearchRequest, FolderSearchResponse, IndexModelState,
    PersistedAutomaticRebuildSuccess, PersistedRebuildRetry, SearchMode,
    SHORT_BULK_MISMATCH_REASON,
};
use super::rebuild_pages::{all_chunks_parked, reembed_pages, reset_parked, swap_in_parked};
use super::sqlite_repo::{bump_generation_tx, read_chunk_count, read_generation};
use super::FolderIndex;

/// Commit attempts before a rebuild that keeps racing writers gives up stale.
const MAX_REBUILD_COMMIT_ATTEMPTS: usize = 8;
pub(crate) const DEFAULT_MODEL_REPROBE_INTERVAL: Duration = Duration::from_secs(10 * 60);
const DEFAULT_PROBE_FAILURE_BACKOFF: Duration = Duration::from_secs(5);
const DEFAULT_REBUILD_FAILURE_BACKOFF: Duration = Duration::from_secs(30 * 60);
const DEFAULT_REBUILD_FAILURE_CAP: Duration = Duration::from_secs(6 * 60 * 60);
const DEFAULT_MAX_SAME_CLASS_REBUILD_FAILURES: u32 = 3;
const MAX_MODEL_CHANGE_CONFIRMATION_DELAY: Duration = Duration::from_secs(60);
const AUTOMATIC_REBUILD_SUCCESS_WINDOW: Duration = Duration::from_secs(60 * 60);
const MAX_PERSISTED_ERROR_BYTES: usize = 256;

#[derive(Debug, Clone, Copy)]
struct RetryState {
    failures: u32,
    next_allowed: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RebuildKey {
    model: String,
    dim: usize,
}

impl RebuildKey {
    fn new(model: impl Into<String>, dim: usize) -> Self {
        Self {
            model: model.into(),
            dim,
        }
    }
}

#[derive(Debug, Clone)]
struct RebuildRetryState {
    key: RebuildKey,
    error_class: String,
    failures: u32,
    next_allowed: Instant,
    halted: bool,
}

#[derive(Debug, Clone, Copy)]
struct RebuildFailureDisposition {
    failures: u32,
    halted: bool,
}

#[derive(Default)]
struct ProbeState {
    active: HashMap<String, Arc<Notify>>,
    retry: HashMap<String, RetryState>,
}

/// Per-folder deduplication and completion notification registry for background
/// re-embedding rebuilds.
pub(crate) struct FolderRebuildRegistry {
    active: Mutex<HashMap<String, Arc<Notify>>>,
    rebuild_retry: Mutex<HashMap<String, RebuildRetryState>>,
    probes: Mutex<ProbeState>,
    probe_interval: Duration,
    probe_failure_backoff: Duration,
    rebuild_failure_backoff: Duration,
    rebuild_failure_cap: Duration,
    max_same_class_rebuild_failures: u32,
    #[cfg(test)]
    clock_offset: Mutex<Duration>,
    /// Test hook: runs in `wait` after the lookup, before the waiter registers.
    #[cfg(test)]
    pub(crate) after_lookup: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl FolderRebuildRegistry {
    pub(crate) fn new() -> Self {
        Self::with_timings(
            DEFAULT_MODEL_REPROBE_INTERVAL,
            DEFAULT_PROBE_FAILURE_BACKOFF,
            DEFAULT_REBUILD_FAILURE_BACKOFF,
            DEFAULT_REBUILD_FAILURE_CAP,
            DEFAULT_MAX_SAME_CLASS_REBUILD_FAILURES,
        )
    }

    pub(crate) fn with_timings(
        probe_interval: Duration,
        probe_failure_backoff: Duration,
        rebuild_failure_backoff: Duration,
        rebuild_failure_cap: Duration,
        max_same_class_rebuild_failures: u32,
    ) -> Self {
        let rebuild_failure_backoff = rebuild_failure_backoff.max(probe_interval.saturating_mul(2));
        Self {
            active: Mutex::new(HashMap::new()),
            rebuild_retry: Mutex::new(HashMap::new()),
            probes: Mutex::new(ProbeState::default()),
            probe_interval,
            probe_failure_backoff,
            rebuild_failure_backoff,
            rebuild_failure_cap: rebuild_failure_cap.max(rebuild_failure_backoff),
            max_same_class_rebuild_failures: max_same_class_rebuild_failures.max(1),
            #[cfg(test)]
            clock_offset: Mutex::new(Duration::ZERO),
            #[cfg(test)]
            after_lookup: Mutex::new(None),
        }
    }

    pub(crate) fn with_probe_interval(probe_interval: Duration) -> Self {
        let probe_interval = probe_interval.max(Duration::from_secs(1));
        Self::with_timings(
            probe_interval,
            DEFAULT_PROBE_FAILURE_BACKOFF.min(probe_interval),
            DEFAULT_REBUILD_FAILURE_BACKOFF,
            DEFAULT_REBUILD_FAILURE_CAP,
            DEFAULT_MAX_SAME_CLASS_REBUILD_FAILURES,
        )
    }

    pub(crate) fn with_rebuild_failure_policy(
        &self,
        initial_backoff: Duration,
        backoff_cap: Duration,
        max_same_class_failures: u32,
    ) -> Self {
        Self::with_timings(
            self.probe_interval,
            self.probe_failure_backoff,
            initial_backoff,
            backoff_cap,
            max_same_class_failures,
        )
    }

    fn now(&self) -> Instant {
        #[cfg(test)]
        {
            let offset = *self.clock_offset.lock().unwrap_or_else(|e| e.into_inner());
            Instant::now()
                .checked_add(offset)
                .unwrap_or_else(Instant::now)
        }
        #[cfg(not(test))]
        {
            Instant::now()
        }
    }

    fn now_unix_ms(&self) -> u64 {
        let base = unix_time_ms();
        #[cfg(test)]
        {
            let offset = *self.clock_offset.lock().unwrap_or_else(|e| e.into_inner());
            base.saturating_add(offset.as_millis().min(u128::from(u64::MAX)) as u64)
        }
        #[cfg(not(test))]
        {
            base
        }
    }

    fn model_change_confirmation_delay_ms(&self) -> u64 {
        self.probe_interval
            .min(MAX_MODEL_CHANGE_CONFIRMATION_DELAY)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    #[cfg(test)]
    pub(crate) fn advance_time(&self, duration: Duration) {
        let mut offset = self.clock_offset.lock().unwrap_or_else(|e| e.into_inner());
        *offset = offset.saturating_add(duration);
    }

    pub(crate) fn clear_rebuild_retry(&self, idx: &FolderIndex) {
        self.rebuild_retry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(idx.folder_id());
        if let Ok(conn) = idx.lock_conn() {
            let _ = clear_persisted_rebuild_retry(&conn);
        }
    }

    fn try_acquire(
        &self,
        idx: &FolderIndex,
        key: &RebuildKey,
        automatic: bool,
    ) -> Option<Arc<Notify>> {
        let folder_id = idx.folder_id();
        let now = self.now();
        if automatic {
            let last_success = idx.lock_conn().ok().and_then(|conn| {
                read_persisted_automatic_rebuild_success(&conn)
                    .ok()
                    .flatten()
            });
            if last_success.is_some_and(|success| {
                self.now_unix_ms()
                    < success.last_success_unix_ms.saturating_add(
                        AUTOMATIC_REBUILD_SUCCESS_WINDOW
                            .as_millis()
                            .min(u128::from(u64::MAX)) as u64,
                    )
            }) {
                return None;
            }
        }
        let persisted = idx
            .lock_conn()
            .ok()
            .and_then(|conn| read_persisted_rebuild_retry(&conn).ok().flatten());
        {
            let mut retry = self.rebuild_retry.lock().unwrap_or_else(|e| e.into_inner());
            if !retry.contains_key(folder_id) {
                if let Some(state) = persisted {
                    let remaining = Duration::from_millis(
                        state.not_before_unix_ms.saturating_sub(unix_time_ms()),
                    )
                    .min(self.rebuild_failure_cap);
                    retry.insert(
                        folder_id.to_string(),
                        RebuildRetryState {
                            key: RebuildKey::new(state.model, state.dim),
                            error_class: state.error_class,
                            failures: state.failures,
                            next_allowed: now + remaining,
                            halted: state.halted,
                        },
                    );
                }
            }
            if retry.get(folder_id).is_some_and(|state| state.key != *key) {
                retry.remove(folder_id);
                if let Ok(conn) = idx.lock_conn() {
                    let _ = clear_persisted_rebuild_retry(&conn);
                }
            }
            if retry
                .get(folder_id)
                .is_some_and(|state| state.halted || now < state.next_allowed)
            {
                return None;
            }
        }
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if guard.contains_key(folder_id) {
            return None;
        }
        let notify = Arc::new(Notify::new());
        guard.insert(folder_id.to_string(), Arc::clone(&notify));
        Some(notify)
    }

    #[cfg(test)]
    pub(crate) fn try_acquire_for_test(&self, folder_id: &str) -> Option<Arc<Notify>> {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if guard.contains_key(folder_id) {
            return None;
        }
        let notify = Arc::new(Notify::new());
        guard.insert(folder_id.to_string(), Arc::clone(&notify));
        Some(notify)
    }

    fn record_failure(
        &self,
        idx: &FolderIndex,
        key: &RebuildKey,
        error: &MakakooError,
    ) -> RebuildFailureDisposition {
        let folder_id = idx.folder_id();
        let error_class = rebuild_error_class(error);
        let mut retry = self.rebuild_retry.lock().unwrap_or_else(|e| e.into_inner());
        let failures = retry.get(folder_id).map_or(1, |state| {
            if state.key == *key && state.error_class == error_class {
                state.failures.saturating_add(1)
            } else {
                1
            }
        });
        let halted = failures >= self.max_same_class_rebuild_failures;
        let delay = exponential_backoff(
            self.rebuild_failure_backoff,
            self.rebuild_failure_cap,
            failures,
        );
        retry.insert(
            folder_id.to_string(),
            RebuildRetryState {
                key: key.clone(),
                error_class: error_class.clone(),
                failures,
                next_allowed: self.now() + delay,
                halted,
            },
        );
        drop(retry);
        if let Ok(conn) = idx.lock_conn() {
            let _ = write_persisted_rebuild_retry(
                &conn,
                &PersistedRebuildRetry {
                    model: key.model.clone(),
                    dim: key.dim,
                    error_class,
                    failures,
                    halted,
                    not_before_unix_ms: unix_time_ms()
                        .saturating_add(delay.as_millis().min(u128::from(u64::MAX)) as u64),
                },
            );
        }
        RebuildFailureDisposition { failures, halted }
    }

    pub(crate) fn finish(&self, folder_id: &str) {
        let notify = {
            let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
            guard.remove(folder_id)
        };
        if let Some(n) = notify {
            n.notify_waiters();
        }
    }

    pub(crate) fn is_active(&self, folder_id: &str) -> bool {
        let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        guard.contains_key(folder_id)
    }

    pub(crate) async fn wait(&self, folder_id: &str) {
        loop {
            let notify = {
                let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
                match guard.get(folder_id) {
                    Some(n) => Arc::clone(n),
                    None => return,
                }
            };
            #[cfg(test)]
            if let Some(hook) = self
                .after_lookup
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                hook();
            }
            // `finish` removes the entry before notifying, so re-check after the
            // waiter is registered: a finish between lookup and await is not lost.
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let still_same = {
                let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .get(folder_id)
                    .is_some_and(|n| Arc::ptr_eq(n, &notify))
            };
            if !still_same {
                // Finished (or replaced by a newer rebuild): look again.
                continue;
            }
            notified.await;
        }
    }

    pub(crate) fn try_probe(&self, folder_id: &str) -> bool {
        let now = self.now();
        let mut probes = self.probes.lock().unwrap_or_else(|e| e.into_inner());
        if probes.active.contains_key(folder_id)
            || probes
                .retry
                .get(folder_id)
                .is_some_and(|state| now < state.next_allowed)
        {
            return false;
        }
        probes
            .active
            .insert(folder_id.to_string(), Arc::new(Notify::new()));
        true
    }

    pub(crate) fn expedite_probe(&self, folder_id: &str) {
        let mut probes = self.probes.lock().unwrap_or_else(|e| e.into_inner());
        if !probes.active.contains_key(folder_id) {
            probes.retry.remove(folder_id);
        }
    }

    pub(crate) fn finish_probe(&self, folder_id: &str, reached_bulk: bool) {
        let notify = {
            let mut probes = self.probes.lock().unwrap_or_else(|e| e.into_inner());
            let notify = probes.active.remove(folder_id);
            let failures = if reached_bulk {
                0
            } else {
                probes
                    .retry
                    .get(folder_id)
                    .map_or(1, |state| state.failures.saturating_add(1))
            };
            let delay = if reached_bulk {
                self.probe_interval
            } else {
                exponential_backoff(self.probe_failure_backoff, self.probe_interval, failures)
            };
            probes.retry.insert(
                folder_id.to_string(),
                RetryState {
                    failures,
                    next_allowed: self.now() + delay,
                },
            );
            notify
        };
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
    }

    pub(crate) async fn wait_for_probe(&self, folder_id: &str) {
        loop {
            let notify = {
                let probes = self.probes.lock().unwrap_or_else(|e| e.into_inner());
                match probes.active.get(folder_id) {
                    Some(notify) => Arc::clone(notify),
                    None => return,
                }
            };
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let still_same = {
                let probes = self.probes.lock().unwrap_or_else(|e| e.into_inner());
                probes
                    .active
                    .get(folder_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &notify))
            };
            if !still_same {
                continue;
            }
            notified.await;
        }
    }
}

pub(crate) struct RebuildActivityGuard {
    rebuilds: Arc<FolderRebuildRegistry>,
    folder_id: String,
}

impl RebuildActivityGuard {
    pub(crate) fn new(rebuilds: Arc<FolderRebuildRegistry>, folder_id: String) -> Self {
        Self {
            rebuilds,
            folder_id,
        }
    }
}

impl Drop for RebuildActivityGuard {
    fn drop(&mut self) {
        self.rebuilds.finish(&self.folder_id);
    }
}

pub(crate) struct ProbeActivityGuard {
    rebuilds: Arc<FolderRebuildRegistry>,
    folder_id: String,
    reached_bulk: bool,
}

impl ProbeActivityGuard {
    pub(crate) fn new(rebuilds: Arc<FolderRebuildRegistry>, folder_id: String) -> Self {
        Self {
            rebuilds,
            folder_id,
            reached_bulk: false,
        }
    }

    fn set_reached_bulk(&mut self, reached_bulk: bool) {
        self.reached_bulk = reached_bulk;
    }
}

impl Drop for ProbeActivityGuard {
    fn drop(&mut self) {
        self.rebuilds
            .finish_probe(&self.folder_id, self.reached_bulk);
    }
}

fn exponential_backoff(initial: Duration, cap: Duration, failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(31);
    initial.saturating_mul(1_u32 << shift).min(cap)
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn rebuild_error_class(error: &MakakooError) -> String {
    match error {
        MakakooError::Http(err) => err.status().map_or_else(
            || "http_transport".to_string(),
            |status| format!("http_{}", status.as_u16()),
        ),
        MakakooError::Llm(message) => message
            .strip_prefix("http ")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|status| status.parse::<u16>().ok())
            .map_or_else(|| "llm".to_string(), |status| format!("http_{status}")),
        MakakooError::Io(err) => format!("io_{:?}", err.kind()),
        MakakooError::Db(err) => format!("sqlite_{:?}", err.sqlite_error_code()),
        MakakooError::Json(_) => "json".to_string(),
        MakakooError::Config(_) => "config".to_string(),
        MakakooError::NotFound(_) => "not_found".to_string(),
        MakakooError::InvalidInput(_) => "invalid_input".to_string(),
        MakakooError::RebuildClaimLost => "rebuild_claim_lost".to_string(),
        MakakooError::RebuildCallTimeout => "rebuild_call_timeout".to_string(),
        MakakooError::Internal(_) => "internal".to_string(),
    }
}

/// Non-blocking startup/open probe: verifies configured endpoint(s) against stored
/// model+dim. Unreachable endpoints leave `index_state` untouched.
pub(crate) async fn probe_and_reconcile(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    rebuilds: Arc<FolderRebuildRegistry>,
) -> bool {
    if idx.chunk_count().unwrap_or(0) == 0 {
        return true;
    }

    let Ok((bulk_model, bulk_dim)) = bulk_emb.probe_model().await else {
        return false;
    };
    let observed = idx.lock_conn().ok().and_then(|mut conn| {
        record_observed_bulk(&mut conn, &bulk_model, bulk_dim, rebuilds.now_unix_ms()).ok()
    });

    let (short_result, short_probe_failed) = match short_emb.as_ref() {
        Some(s) if s.model() != bulk_emb.model() || s.base_url() != bulk_emb.base_url() => {
            match s.probe_model().await {
                Ok(pair) => (Some(pair), false),
                Err(_) => (None, true),
            }
        }
        Some(_) => (Some((bulk_model.clone(), bulk_dim)), false),
        None => (None, false),
    };

    // A rebuild already running owns the model state; its own probe and the next
    // probe after it finishes decide. Writing `stale` here would hide `rebuilding`.
    if rebuilds.is_active(idx.folder_id()) {
        return true;
    }

    let short_mismatch = short_result
        .as_ref()
        .is_some_and(|(short_model, short_dim)| {
            short_model != &bulk_model || *short_dim != bulk_dim
        });

    let Ok(status) = idx.model_status() else {
        return true;
    };
    if status.embed_model.is_none() && status.embed_dim == Some(bulk_dim) {
        // Rows were written without a model id by the bulk alias: name it now.
        if let Ok(conn) = idx.lock_conn() {
            let current = read_model_status(&conn);
            if matches!(&current, Ok(c) if c.embed_model.is_none()
                && c.index_state == IndexModelState::Ready)
            {
                let _ = write_ready_model_meta(
                    &conn,
                    &bulk_model,
                    bulk_dim,
                    bulk_emb.model(),
                    short_emb.as_deref().map(EmbeddingClient::model),
                );
            }
        }
        idx.update_sidecar_if_ready();
        return true;
    }
    let matches_stored =
        status.embed_model.as_deref() == Some(&bulk_model) && status.embed_dim == Some(bulk_dim);

    if matches_stored {
        if short_mismatch {
            let _ = idx.mark_stale_unless_rebuilding(SHORT_BULK_MISMATCH_REASON);
            return true;
        }
        if short_probe_failed && status.stale_reason.as_deref() == Some(SHORT_BULK_MISMATCH_REASON)
        {
            return true;
        }
        if status.index_state == IndexModelState::Stale
            && status
                .stale_reason
                .as_deref()
                .is_some_and(is_metadata_only_stale_reason)
        {
            if let Ok(conn) = idx.lock_conn() {
                let _ = write_ready_model_meta(
                    &conn,
                    &bulk_model,
                    bulk_dim,
                    bulk_emb.model(),
                    short_emb.as_deref().map(EmbeddingClient::model),
                );
            }
            idx.update_sidecar_if_ready();
            return true;
        }
        if status.index_state == IndexModelState::Stale {
            spawn_folder_rebuild(
                idx,
                short_emb,
                bulk_emb,
                rebuilds,
                Some((bulk_model, bulk_dim)),
                short_result,
                true,
            );
        }
        return true;
    }

    if status.index_state == IndexModelState::Ready
        || status.stale_reason.as_deref() == Some("bulk_model_change_pending")
    {
        let _ = idx.mark_stale_unless_rebuilding("bulk_model_change_pending");
    }
    if observed
        .as_ref()
        .is_none_or(|value| !value.is_confirmed(rebuilds.model_change_confirmation_delay_ms()))
    {
        return true;
    }

    spawn_folder_rebuild(
        idx,
        short_emb,
        bulk_emb,
        rebuilds,
        Some((bulk_model, bulk_dim)),
        short_result,
        true,
    );
    true
}

fn is_metadata_only_stale_reason(reason: &str) -> bool {
    matches!(
        reason,
        "embed_alias_changed"
            | "short_embed_alias_changed"
            | "bulk_model_change_pending"
            | SHORT_BULK_MISMATCH_REASON
    ) || reason.starts_with("model_mismatch:")
        || reason.starts_with("dim_mismatch:")
}

/// Schedule one rate-limited endpoint reconciliation. Query metadata is never
/// passed here: only the bulk endpoint can establish that stored vectors changed.
pub(crate) fn schedule_probe(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    rebuilds: Arc<FolderRebuildRegistry>,
) {
    if idx.chunk_count().unwrap_or(0) == 0 || !rebuilds.try_probe(idx.folder_id()) {
        return;
    }
    tokio::spawn(async move {
        let folder_id = idx.folder_id().to_string();
        let mut activity = ProbeActivityGuard::new(Arc::clone(&rebuilds), folder_id);
        let reached_bulk = probe_and_reconcile(idx, short_emb, bulk_emb, rebuilds).await;
        activity.set_reached_bulk(reached_bulk);
    });
}

pub(crate) fn spawn_folder_rebuild(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    rebuilds: Arc<FolderRebuildRegistry>,
    observed_bulk: Option<(String, usize)>,
    observed_short: Option<(String, usize)>,
    automatic: bool,
) {
    let Some((model, dim)) = observed_bulk.as_ref() else {
        return;
    };
    let rebuild_key = RebuildKey::new(model.clone(), *dim);
    if rebuilds
        .try_acquire(&idx, &rebuild_key, automatic)
        .is_none()
    {
        return;
    }

    if let Ok(conn) = idx.lock_conn() {
        let prev_reason = read_model_status(&conn)
            .ok()
            .and_then(|s| s.stale_reason)
            .unwrap_or_else(|| "rebuilding".to_string());
        let _ = write_model_state(&conn, IndexModelState::Rebuilding, Some(&prev_reason));
    }

    let rebuilds_clone = Arc::clone(&rebuilds);
    tokio::spawn(async move {
        let fid = idx.folder_id().to_string();
        let _activity = RebuildActivityGuard::new(Arc::clone(&rebuilds_clone), fid);
        let succeeded = match run_folder_rebuild(
            Arc::clone(&idx),
            short_emb,
            bulk_emb,
            observed_bulk,
            observed_short,
            automatic.then(|| Arc::clone(&rebuilds_clone)),
        )
        .await
        {
            Ok(()) => true,
            Err(err) => {
                let error = err.to_string();
                let disposition = rebuilds_clone.record_failure(&idx, &rebuild_key, &err);
                let reason = if disposition.halted {
                    format!(
                        "rebuild_suppressed: model={} dim={} after {} same-class failures: {}",
                        rebuild_key.model, rebuild_key.dim, disposition.failures, error
                    )
                } else {
                    format!("rebuild_failed: {error}")
                };
                let _ = idx.mark_stale(&truncate_utf8(&reason, MAX_PERSISTED_ERROR_BYTES));
                false
            }
        };
        if succeeded {
            rebuilds_clone.clear_rebuild_retry(&idx);
        }
    });
}

async fn run_folder_rebuild(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    bulk_emb: Arc<EmbeddingClient>,
    observed_bulk: Option<(String, usize)>,
    mut observed_short: Option<(String, usize)>,
    automatic_success_clock: Option<Arc<FolderRebuildRegistry>>,
) -> Result<()> {
    let (start_gen, chunk_count) = {
        let conn = idx.lock_conn()?;
        reset_parked(&conn)?;
        (read_generation(&conn)?, read_chunk_count(&conn)?)
    };

    if chunk_count == 0 {
        let conn = idx.lock_conn()?;
        write_model_state(&conn, IndexModelState::Ready, None)?;
        return Ok(());
    }

    let (b_model, b_dim) = match observed_bulk {
        Some(pair) => pair,
        None => bulk_emb.probe_model().await?,
    };
    if observed_short.is_none() {
        if let Some(ref s) = short_emb {
            if s.model() != bulk_emb.model() || s.base_url() != bulk_emb.base_url() {
                if let Ok(pair) = s.probe_model().await {
                    observed_short = Some(pair);
                }
            }
        }
    }

    let short_mismatch = observed_short
        .as_ref()
        .is_some_and(|(s_model, s_dim)| s_model != &b_model || *s_dim != b_dim);

    reembed_pages(&idx, &bulk_emb, &b_model, b_dim).await?;

    // Upserts replace a file's chunks with new, higher ids and bump the
    // generation. Commit only while the generation is the one whose rows were
    // all re-embedded, checked under the commit's own lock; otherwise re-embed
    // the rows that landed meanwhile and try again.
    let mut seen_gen = start_gen;
    for _ in 0..MAX_REBUILD_COMMIT_ATTEMPTS {
        #[cfg(test)]
        if let Some(hook) = idx
            .before_rebuild_commit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            hook(&idx);
        }
        {
            let mut conn = idx.lock_conn()?;
            let gen_now = read_generation(&conn)?;
            let commit_meta = RebuildCommitMeta {
                model: &b_model,
                dim: b_dim,
                bulk_emb: &bulk_emb,
                short_emb: &short_emb,
                automatic_success_unix_ms: automatic_success_clock
                    .as_ref()
                    .map(|clock| clock.now_unix_ms()),
            };
            if gen_now == seen_gen && commit_rebuild(&idx, &mut conn, seen_gen, &commit_meta)? {
                drop(conn);
                idx.checkpoint_wal()?;
                idx.clear_query_mismatch();
                if short_mismatch {
                    idx.mark_stale(SHORT_BULK_MISMATCH_REASON)?;
                }
                return Ok(());
            }
            seen_gen = read_generation(&conn)?;
        }
        // Ids are reused once the top rows are deleted, so the tail is not
        // "ids above the last one": rescan every chunk, skipping those parked
        // with the same text.
        reembed_pages(&idx, &bulk_emb, &b_model, b_dim).await?;
    }
    Err(MakakooError::internal(
        "rebuild kept racing writers; left stale for the next probe",
    ))
}

/// Commit the parked vectors if the index is still at `seen_gen` and every
/// chunk holds a parked vector for its current text. Returns false when the
/// caller must rescan (a writer got in, or a row is not covered).
struct RebuildCommitMeta<'a> {
    model: &'a str,
    dim: usize,
    bulk_emb: &'a EmbeddingClient,
    short_emb: &'a Option<Arc<EmbeddingClient>>,
    automatic_success_unix_ms: Option<u64>,
}

fn commit_rebuild(
    idx: &FolderIndex,
    conn: &mut rusqlite::Connection,
    seen_gen: u64,
    meta: &RebuildCommitMeta<'_>,
) -> Result<bool> {
    // Vectors, generation and the ready model record commit together: a crash
    // leaves the old state, never new vectors under the old model's record.
    let new_gen = {
        // Immediate: no other process can write between the checks and the swap.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if read_generation(&tx)? != seen_gen || !all_chunks_parked(&tx)? {
            return Ok(false);
        }
        swap_in_parked(&tx)?;
        let new_gen = bump_generation_tx(&tx)?;
        write_ready_model_meta(
            &tx,
            meta.model,
            meta.dim,
            meta.bulk_emb.model(),
            meta.short_emb.as_deref().map(EmbeddingClient::model),
        )?;
        if let Some(last_success_unix_ms) = meta.automatic_success_unix_ms {
            write_persisted_automatic_rebuild_success(
                &tx,
                &PersistedAutomaticRebuildSuccess {
                    last_success_unix_ms,
                },
            )?;
        }
        tx.commit()?;
        new_gen
    };
    let count = read_chunk_count(conn)?;
    idx.vectors
        .set_model_meta(Some(meta.model.to_string()), Some(meta.dim));
    idx.vectors.rebuild_from_sqlite(conn, new_gen, count)?;
    Ok(true)
}

/// Execute a model-aware search against `idx`, triggering background rebuild only
/// when the bulk embedder differs from the index and serving FTS fallback whenever
/// the index is stale, rebuilding, or the query metadata is mismatched.
pub(crate) async fn search_folder_checked(
    idx: Arc<FolderIndex>,
    short_emb: Option<Arc<EmbeddingClient>>,
    rebuilds: Arc<FolderRebuildRegistry>,
    req: FolderSearchRequest,
) -> Result<FolderSearchResponse> {
    let status = idx.model_status()?;
    let chunk_count = idx.chunk_count()?;

    if chunk_count == 0 {
        let mode = if req.query_vector.is_some() && req.query.is_none() {
            SearchMode::Vector
        } else {
            SearchMode::Fts
        };
        return Ok(FolderSearchResponse {
            folder_id: req.folder_id,
            passages: Vec::new(),
            mode,
            stale: false,
            rebuilding: false,
            embed_model: status.embed_model,
            embed_dim: status.embed_dim,
        });
    }

    if req.query_vector.is_none() && status.index_state != IndexModelState::Ready {
        return fts_fallback_response(&idx, &req, &rebuilds, true);
    }

    let (q_vec, q_model, q_dim) = if let Some(ref vec) = req.query_vector {
        (Some(vec.clone()), req.query_model.clone(), req.query_dim)
    } else if let (Some(q), Some(s_emb)) = (req.query.as_deref(), short_emb.as_ref()) {
        match s_emb.embed_with_meta(q).await {
            Ok(res) => (Some(res.vector), Some(res.model), Some(res.dim)),
            Err(_) => {
                return fts_fallback_response(
                    &idx,
                    &req,
                    &rebuilds,
                    status.index_state != IndexModelState::Ready,
                );
            }
        }
    } else {
        (None, req.query_model.clone(), req.query_dim)
    };

    // An index written without a model id is matched on dimension only until a bulk
    // probe names its model (probe_and_reconcile); calling it a switch would rebuild.
    let model_mismatch = match (q_model.as_deref(), status.embed_model.as_deref()) {
        (Some(qm), Some(im)) => qm != im,
        _ => false,
    };
    let dim_mismatch = match (q_dim, status.embed_dim) {
        (Some(qd), Some(id)) => qd != id,
        (Some(_), None) => true,
        _ => false,
    };

    if model_mismatch || dim_mismatch {
        let reason = if model_mismatch {
            format!(
                "model_mismatch: index={:?} query={:?}",
                status.embed_model, q_model
            )
        } else {
            format!(
                "dim_mismatch: index={:?} query={:?}",
                status.embed_dim, q_dim
            )
        };
        if status.index_state == IndexModelState::Ready {
            idx.record_query_mismatch(&reason);
        }
        return fts_fallback_response(&idx, &req, &rebuilds, true);
    }

    if status.index_state != IndexModelState::Ready {
        return fts_fallback_response(&idx, &req, &rebuilds, true);
    }

    if let Some(vec) = q_vec {
        idx.clear_query_mismatch();
        let passages = idx.search(&vec, req.k, &req.filter)?;
        Ok(FolderSearchResponse {
            folder_id: req.folder_id,
            passages,
            mode: SearchMode::Vector,
            stale: false,
            rebuilding: false,
            embed_model: status.embed_model,
            embed_dim: status.embed_dim,
        })
    } else {
        fts_fallback_response(&idx, &req, &rebuilds, false)
    }
}

fn fts_fallback_response(
    idx: &FolderIndex,
    req: &FolderSearchRequest,
    rebuilds: &FolderRebuildRegistry,
    stale: bool,
) -> Result<FolderSearchResponse> {
    let cur = idx.model_status()?;
    let passages = match req.query.as_deref() {
        Some(q) => idx.search_fts(q, req.k, &req.filter)?,
        None => Vec::new(),
    };
    Ok(FolderSearchResponse {
        folder_id: req.folder_id.clone(),
        passages,
        mode: SearchMode::Fts,
        stale,
        rebuilding: cur.index_state == IndexModelState::Rebuilding
            || rebuilds.is_active(&req.folder_id),
        embed_model: cur.embed_model,
        embed_dim: cur.embed_dim,
    })
}
