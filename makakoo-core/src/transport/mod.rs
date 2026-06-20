//! Transport layer for Makakoo subagents.
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS Phase 0 / Q12: a Makakoo-native
//! Rust contract INSPIRED BY OpenClaw's `ChannelPlugin` shape. The trait
//! seams (gateway, config, secrets, status) mirror OpenClaw's
//! responsibility split, but no source/binary compatibility is promised.
//!
//! Scope note: the never-assembled in-process runtime (the router, the
//! outbound/pairing sub-trait stubs, and the post-v1 adapter skeletons
//! for WhatsApp / Web / Twilio voice / email) was retired. What remains
//! is what production uses: the `config` + `secrets` parsing the slot
//! registry and `agent validate` read, the `frame` wire schema shared
//! with `ipc`, and the Telegram / Slack / Discord adapters that
//! `channel_ops` wraps. The live message loop runs in the Python
//! harveychat gateway, not here.

use async_trait::async_trait;

use crate::Result;

pub mod config;
pub mod discord;
pub mod frame;
pub mod gateway;
pub mod secrets;
pub mod slack;
pub mod status;
pub mod telegram;

pub use frame::{MakakooFrame, MakakooInboundFrame, MakakooOutboundFrame, ThreadKind};
pub use secrets::{ResolvedSecret, SecretRef, SecretsAdapter};

/// Spawn context handed to a transport task at start. Carries the
/// fixed (slot_id, transport_id) pair so every frame the task emits
/// can stamp them without a lookup.
#[derive(Debug, Clone)]
pub struct TransportContext {
    pub slot_id: String,
    pub transport_id: String,
}

/// The umbrella `Transport` trait. The Telegram / Slack / Discord
/// adapters implement it; `channel_ops` consumes those adapters
/// concretely (not as `dyn Transport`). The trait pairs with the
/// `gateway`, `config`, `secrets` and `status` concern boundaries.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Stable type discriminator: `"telegram"`, `"slack"`, …
    fn kind(&self) -> &'static str;

    /// The transport_id from the agent TOML.
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

    /// Default no-op for optional OpenClaw seams an adapter does not
    /// implement. Logs at `DEBUG` so adapters opt in only to the
    /// seams they actually need.
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
