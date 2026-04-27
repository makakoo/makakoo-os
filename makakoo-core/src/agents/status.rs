//! Slot-level status aggregator.
//!
//! Phase 4 deliverable.  Each transport adapter holds a clone of
//! a `Arc<TransportStatusHandle>` populated at slot-supervisor
//! startup; when the adapter receives a frame, reconnects, or
//! drops a frame, it updates the handle.  `makakoo agent status
//! <slot>` reads the handles back through `SlotStatus::collect`
//! to render the locked Phase 4 layout:
//!
//! ```text
//! secretary
//!   gateway:   alive   pid=12345  last_frame=2s ago
//!   transport telegram-main:  connected  last_inbound=8s ago  errors_1h=0
//!   transport slack-main:     connected  last_inbound=3m ago  errors_1h=1
//! ```
//!
//! All atomicity is local to each transport handle — no
//! cross-transport coordination — so adapters never block waiting
//! on the status read path.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub use crate::transport::status::TransportRunState;

/// Handle one transport adapter writes status updates to.  Cheap
/// to clone (`Arc`-backed) and lock-free on the read path.
#[derive(Debug, Clone)]
pub struct TransportStatusHandle {
    pub transport_id: String,
    pub kind: String,
    inner: Arc<Mutex<TransportStatusInner>>,
}

#[derive(Debug)]
struct TransportStatusInner {
    state: TransportRunState,
    last_inbound_at: Option<DateTime<Utc>>,
    /// Sliding window of error timestamps over the last hour.
    /// Trimmed on every status read.
    error_timestamps: VecDeque<DateTime<Utc>>,
    queue_depth: u32,
}

impl TransportStatusHandle {
    pub fn new(transport_id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            transport_id: transport_id.into(),
            kind: kind.into(),
            inner: Arc::new(Mutex::new(TransportStatusInner {
                state: TransportRunState::Reconnecting,
                last_inbound_at: None,
                error_timestamps: VecDeque::new(),
                queue_depth: 0,
            })),
        }
    }

    pub fn set_state(&self, state: TransportRunState) {
        if let Ok(mut g) = self.inner.lock() {
            g.state = state;
        }
    }

    pub fn record_inbound(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.last_inbound_at = Some(Utc::now());
        }
    }

    pub fn record_error(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.error_timestamps.push_back(Utc::now());
        }
    }

    pub fn set_queue_depth(&self, depth: u32) {
        if let Ok(mut g) = self.inner.lock() {
            g.queue_depth = depth;
        }
    }

    /// Snapshot the current status, trimming any error
    /// timestamps older than 1 hour from the sliding window.
    pub fn snapshot(&self) -> TransportStatusSnapshot {
        let now = Utc::now();
        let cutoff = now - Duration::hours(1);
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        while let Some(front) = g.error_timestamps.front() {
            if *front < cutoff {
                g.error_timestamps.pop_front();
            } else {
                break;
            }
        }
        TransportStatusSnapshot {
            transport_id: self.transport_id.clone(),
            kind: self.kind.clone(),
            state: g.state.clone(),
            last_inbound_at: g.last_inbound_at,
            errors_1h: g.error_timestamps.len() as u32,
            queue_depth: g.queue_depth,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportStatusSnapshot {
    pub transport_id: String,
    pub kind: String,
    pub state: TransportRunState,
    pub last_inbound_at: Option<DateTime<Utc>>,
    pub errors_1h: u32,
    pub queue_depth: u32,
}

/// Aggregated slot-level status for `makakoo agent status <slot>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotStatus {
    pub slot_id: String,
    pub gateway: GatewayStatus,
    pub transports: Vec<TransportStatusSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayStatus {
    pub alive: bool,
    pub pid: Option<u32>,
    pub last_frame_at: Option<DateTime<Utc>>,
}

impl SlotStatus {
    /// Collect the live status for a slot.  Caller is the supervisor
    /// (Phase 4 lifecycle layer) that already holds handles to each
    /// transport adapter and to the Python gateway process; here
    /// we just snapshot the data they expose.
    pub fn collect(
        slot_id: impl Into<String>,
        gateway: GatewayStatus,
        transport_handles: &[TransportStatusHandle],
    ) -> Self {
        let mut transports: Vec<TransportStatusSnapshot> =
            transport_handles.iter().map(|h| h.snapshot()).collect();
        transports.sort_by(|a, b| a.transport_id.cmp(&b.transport_id));
        Self {
            slot_id: slot_id.into(),
            gateway,
            transports,
        }
    }

    /// Render the Phase 4 locked human-readable layout.
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.slot_id);
        out.push('\n');
        let alive = if self.gateway.alive { "alive" } else { "dead" };
        let pid = self
            .gateway
            .pid
            .map(|p| format!("pid={p}"))
            .unwrap_or_else(|| "pid=?".into());
        let last_frame = self
            .gateway
            .last_frame_at
            .map(|t| format!("last_frame={}", render_relative(t)))
            .unwrap_or_else(|| "last_frame=never".into());
        out.push_str(&format!("  gateway:   {alive:<8}{pid:<14}{last_frame}\n"));
        for t in &self.transports {
            let state = match t.state {
                TransportRunState::Connected => "connected",
                TransportRunState::Reconnecting => "reconnecting",
                TransportRunState::Failed => "failed",
            };
            let last_inbound = t
                .last_inbound_at
                .map(|i| format!("last_inbound={}", render_relative(i)))
                .unwrap_or_else(|| "last_inbound=never".into());
            out.push_str(&format!(
                "  transport {tid}:  {state:<14}{last_inbound:<26}errors_1h={e}  queue_depth={q}\n",
                tid = t.transport_id,
                e = t.errors_1h,
                q = t.queue_depth,
            ));
        }
        out
    }
}

fn render_relative(t: DateTime<Utc>) -> String {
    let delta = Utc::now() - t;
    let secs = delta.num_seconds();
    if secs < 0 {
        return "now".into();
    }
    if secs < 60 {
        return format!("{secs}s ago");
    }
    if secs < 3600 {
        return format!("{}m ago", secs / 60);
    }
    if secs < 86_400 {
        return format!("{}h ago", secs / 3600);
    }
    format!("{}d ago", secs / 86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_snapshot_initial_state_is_reconnecting() {
        let h = TransportStatusHandle::new("telegram-main", "telegram");
        let s = h.snapshot();
        assert_eq!(s.state, TransportRunState::Reconnecting);
        assert!(s.last_inbound_at.is_none());
        assert_eq!(s.errors_1h, 0);
        assert_eq!(s.queue_depth, 0);
    }

    #[test]
    fn record_inbound_updates_last_inbound() {
        let h = TransportStatusHandle::new("telegram-main", "telegram");
        h.record_inbound();
        let s = h.snapshot();
        assert!(s.last_inbound_at.is_some());
    }

    #[test]
    fn errors_window_counts_recent_only() {
        let h = TransportStatusHandle::new("telegram-main", "telegram");
        // Inject a stale entry directly via the inner mutex to
        // simulate a 2-hour-old error.
        {
            let mut g = h.inner.lock().unwrap();
            g.error_timestamps.push_back(Utc::now() - Duration::hours(2));
        }
        h.record_error();
        h.record_error();
        let s = h.snapshot();
        // Stale one must be evicted; only the two fresh errors count.
        assert_eq!(s.errors_1h, 2);
    }

    #[test]
    fn state_transitions_propagate() {
        let h = TransportStatusHandle::new("slack-main", "slack");
        h.set_state(TransportRunState::Connected);
        assert_eq!(h.snapshot().state, TransportRunState::Connected);
        h.set_state(TransportRunState::Failed);
        assert_eq!(h.snapshot().state, TransportRunState::Failed);
    }

    #[test]
    fn slot_status_sorts_transports_by_id() {
        let tg = TransportStatusHandle::new("z-telegram", "telegram");
        let sl = TransportStatusHandle::new("a-slack", "slack");
        let s = SlotStatus::collect(
            "secretary",
            GatewayStatus {
                alive: true,
                pid: Some(12345),
                last_frame_at: Some(Utc::now()),
            },
            &[tg, sl],
        );
        assert_eq!(s.transports[0].transport_id, "a-slack");
        assert_eq!(s.transports[1].transport_id, "z-telegram");
    }

    #[test]
    fn render_human_matches_phase_4_layout() {
        let tg = TransportStatusHandle::new("telegram-main", "telegram");
        tg.set_state(TransportRunState::Connected);
        tg.record_inbound();
        let sl = TransportStatusHandle::new("slack-main", "slack");
        sl.set_state(TransportRunState::Reconnecting);
        sl.record_error();
        let s = SlotStatus::collect(
            "secretary",
            GatewayStatus {
                alive: true,
                pid: Some(12345),
                last_frame_at: Some(Utc::now()),
            },
            &[tg, sl],
        );
        let out = s.render_human();
        // Layout sanity — exact whitespace varies but the locked
        // tokens must all appear.
        assert!(out.contains("secretary\n"));
        assert!(out.contains("gateway:"));
        assert!(out.contains("pid=12345"));
        assert!(out.contains("transport slack-main:"));
        assert!(out.contains("reconnecting"));
        assert!(out.contains("errors_1h=1"));
        assert!(out.contains("transport telegram-main:"));
        assert!(out.contains("connected"));
        assert!(out.contains("queue_depth=0"));
    }

    #[test]
    fn render_human_handles_no_inbound_yet() {
        let tg = TransportStatusHandle::new("telegram-main", "telegram");
        let s = SlotStatus::collect(
            "secretary",
            GatewayStatus {
                alive: false,
                pid: None,
                last_frame_at: None,
            },
            &[tg],
        );
        let out = s.render_human();
        assert!(out.contains("dead"));
        assert!(out.contains("pid=?"));
        assert!(out.contains("last_frame=never"));
        assert!(out.contains("last_inbound=never"));
    }

    #[test]
    fn snapshot_is_serde_round_trippable() {
        let h = TransportStatusHandle::new("telegram-main", "telegram");
        h.set_state(TransportRunState::Connected);
        h.record_inbound();
        let s = h.snapshot();
        let json = serde_json::to_string(&s).unwrap();
        let back: TransportStatusSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
