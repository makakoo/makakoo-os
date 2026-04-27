//! `ChannelThreadingAdapter` — thread create/list/follow.
//!
//! Phase 6 / Q6. Threading semantics differ wildly across transports:
//! - Telegram: forum topics in supergroups (`createForumTopic`)
//! - Slack:    `thread_ts` rooted off any message
//! - Discord:  threads anchored to a channel or message (Phase 7)
//!
//! Implementations may legitimately return
//! `ChannelOpError::Unsupported` for ops that don't map onto the
//! transport's native API; callers are expected to handle that case
//! gracefully.

use async_trait::async_trait;

use crate::channel_ops::directory::ChannelOpError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadParent {
    /// Create a new thread anchored to a channel (Telegram forum
    /// topic, Slack thread on a top-level message, Discord thread on
    /// a text channel).
    Channel(String),
    /// Create a thread anchored to a specific message (Slack
    /// `thread_ts = parent.ts`, Discord thread-from-message).
    Message {
        channel_id: String,
        message_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSummary {
    pub id: String,
    pub channel_id: String,
    pub title: Option<String>,
    /// Approximate count — implementations may report 0 if the
    /// underlying API does not expose a cheap count.
    pub message_count: u32,
}

#[async_trait]
pub trait ChannelThreadingAdapter: Send + Sync {
    fn transport_id(&self) -> &str;
    fn transport_kind(&self) -> &'static str;

    /// Create a new thread. Returns the thread's id (Telegram
    /// message_thread_id, Slack thread_ts, Discord thread_id).
    async fn create_thread(
        &self,
        parent: &ThreadParent,
        title: Option<&str>,
    ) -> Result<String, ChannelOpError>;

    /// List threads anchored on the given channel. Implementations
    /// that have no listing API return `Unsupported`.
    async fn list_threads(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ThreadSummary>, ChannelOpError>;

    /// Mark a thread as one the bot is actively watching. For
    /// transports without an explicit follow primitive this is a
    /// local-only marker (returns `Ok(())` even when the transport
    /// has no remote subscription concept).
    async fn follow_thread(&self, thread_id: &str) -> Result<(), ChannelOpError>;
}
