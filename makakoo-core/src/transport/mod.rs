//! Transport layer for Makakoo subagents.
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS Phase 0 / Q12: a Makakoo-native
//! Rust contract INSPIRED BY OpenClaw's `ChannelPlugin` shape. The trait
//! seams (gateway, outbound, config, secrets, status, pairing) mirror
//! OpenClaw's responsibility split, but no source/binary compatibility
//! is promised.
//!
//! Optional handlers in OpenClaw map to default trait impls in Rust
//! that return `Ok(())` and emit a `DEBUG` log so adapters opt in to
//! the seams they actually need.
//!
//! v1 ships `Telegram` and `Slack` (Socket Mode). Discord, WhatsApp,
//! and the deferred adapters listed in Q12 are post-v1.

use async_trait::async_trait;

use crate::Result;

pub mod config;
pub mod frame;
pub mod gateway;
pub mod outbound;
pub mod pairing;
pub mod router;
pub mod secrets;
pub mod slack;
pub mod status;
pub mod telegram;

pub use frame::{MakakooFrame, MakakooInboundFrame, MakakooOutboundFrame, ThreadKind};
pub use router::{RouterError, TransportRouter};
pub use secrets::{ResolvedSecret, SecretRef, SecretsAdapter};

/// Spawn context handed to a transport task at start. Carries the
/// fixed (slot_id, transport_id) pair so every frame the task emits
/// can stamp them without a lookup.
#[derive(Debug, Clone)]
pub struct TransportContext {
    pub slot_id: String,
    pub transport_id: String,
}

/// The umbrella `Transport` trait. Every adapter (Telegram, Slack, …)
/// implements this. The trait composes the smaller adapters from
/// `gateway`, `outbound`, `config`, `secrets`, `status`, `pairing`
/// modules — they are not required for v1 to be separate trait
/// objects, just separate concern boundaries.
///
/// In v1 the umbrella trait directly exposes the methods needed by
/// the router. Phase-3+ may split these into sub-traits as the
/// surface grows.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Stable type discriminator: `"telegram"`, `"slack"`, …
    fn kind(&self) -> &'static str;

    /// The transport_id from the agent TOML. Used by the router to
    /// match outbound frames back to their adapter instance.
    fn transport_id(&self) -> &str;

    /// Verify the credentials in the adapter's config (e.g. Telegram
    /// `getMe`, Slack `auth.test`). MUST run before the adapter is
    /// considered ready. Returns the resolved bot identity on
    /// success.
    async fn verify_credentials(&self) -> Result<VerifiedIdentity>;

    /// Send an outbound frame. Implementations coerce
    /// `reply_to_message_id` to the transport's native type and drop
    /// it (with WARN) if the format doesn't match.
    async fn send(&self, frame: &MakakooOutboundFrame) -> Result<()>;

    /// Default no-op for optional OpenClaw seams that haven't been
    /// implemented yet for this adapter. Adapters that need pairing
    /// override `pairing::PairingAdapter` separately; this hook is a
    /// placeholder for the trait-level handler-presence check.
    async fn on_unimplemented_handler(&self, name: &str) -> Result<()> {
        tracing::debug!(
            target: "makakoo_core::transport",
            adapter = self.kind(),
            transport_id = self.transport_id(),
            handler = name,
            "transport adapter does not implement optional handler"
        );
        Ok(())
    }
}

/// Resolved identity returned by `Transport::verify_credentials`.
/// The fields fill in the inbound frame's `account_id` and
/// `tenant_id` for diagnostic visibility.
#[derive(Debug, Clone)]
pub struct VerifiedIdentity {
    /// `getMe.id` for Telegram, `auth.test.bot_id` for Slack.
    pub account_id: String,
    /// Slack `team_id`; `None` for transports that don't have
    /// tenant scoping.
    pub tenant_id: Option<String>,
    /// Display name (informational only — not used for ACL).
    pub display_name: Option<String>,
}
