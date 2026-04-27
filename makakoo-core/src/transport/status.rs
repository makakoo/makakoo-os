//! Per-transport status reporting (`ChannelStatusAdapter` analog).
//!
//! v1: Phase 1 stubs the trait so adapters compile; Phase 4 wires
//! the live status into `makakoo agent status <slot>` per-`transport.id`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransportRunState {
    Connected,
    Reconnecting,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportStatus {
    pub transport_id: String,
    pub kind: String,
    pub state: TransportRunState,
    pub last_inbound_at: Option<DateTime<Utc>>,
    pub errors_1h: u32,
}
