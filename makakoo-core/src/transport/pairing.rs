//! `ChannelPairingAdapter` analog (Q12).
//!
//! v1: pairing collapses to a per-transport `allowed_users` allow-
//! list (see `TransportEntry::allowed_users`). The trait exists so
//! that future adapters (Slack OAuth user-token, WhatsApp pairing
//! flows) can plug in without touching the umbrella `Transport`
//! trait.

use async_trait::async_trait;

use crate::Result;

#[async_trait]
pub trait Pairing: Send + Sync {
    /// Returns true iff the given canonical sender id is currently
    /// permitted to interact with this transport.
    async fn is_paired(&self, sender_id: &str) -> Result<bool>;
}
