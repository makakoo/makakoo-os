//! `ChannelMessagingAdapter` — DM / channel send + broadcast.
//!
//! Phase 6 / Q6. Distinct from the Phase-1 outbound `Transport::send`
//! which carries a generic `MakakooOutboundFrame`. This trait lets
//! the LLM call out to a specific user (DM) vs a specific channel
//! without having to construct the lower-level frame.

use async_trait::async_trait;

use crate::channel_ops::directory::ChannelOpError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRef {
    pub channel_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone)]
pub struct BroadcastResult {
    pub channel_id: String,
    pub outcome: Result<MessageRef, String>,
}

#[async_trait]
pub trait ChannelMessagingAdapter: Send + Sync {
    fn transport_id(&self) -> &str;
    fn transport_kind(&self) -> &'static str;

    /// Send a direct message to a user. Implementations resolve the
    /// user_id → DM channel as needed (Slack `conversations.open`).
    async fn send_dm(&self, user_id: &str, text: &str) -> Result<MessageRef, ChannelOpError>;

    /// Send a message to a channel (public/private/group).
    async fn send_channel(
        &self,
        channel_id: &str,
        text: &str,
    ) -> Result<MessageRef, ChannelOpError>;

    /// Send the same message to multiple channels. Returns one
    /// `BroadcastResult` per input id, preserving order. Errors on
    /// individual sends are captured per-channel — the function
    /// itself never short-circuits on partial failure.
    async fn broadcast(
        &self,
        channel_ids: &[String],
        text: &str,
    ) -> Vec<BroadcastResult>;
}
