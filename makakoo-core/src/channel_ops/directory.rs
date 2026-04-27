//! `ChannelDirectoryAdapter` — list/lookup channels and users.
//!
//! Phase 6 / Q6. The trait is dyn-compatible via `async_trait` so the
//! registry can store `Arc<dyn ChannelDirectoryAdapter>`.

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelKind {
    /// One-on-one conversation (Telegram private chat, Slack `D…`).
    Dm,
    /// Public channel (Slack public, Discord guild text channel).
    Channel,
    /// Multi-party group (Telegram group, Slack private channel).
    Group,
    /// Threaded sub-conversation.
    Thread,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSummary {
    pub id: String,
    pub name: Option<String>,
    pub kind: ChannelKind,
    /// Whether the bot is currently a member of this channel.
    pub is_member: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    pub id: String,
    pub display_name: Option<String>,
    pub handle: Option<String>,
    pub is_bot: bool,
}

/// Errors raised by any of the four channel-ops trait families.
/// Shared so the MCP layer can map all four to a single result shape.
#[derive(Debug, thiserror::Error)]
pub enum ChannelOpError {
    #[error("transport HTTP error: {0}")]
    Http(String),
    #[error("transport returned error: {0}")]
    Remote(String),
    #[error("unknown channel '{0}'")]
    UnknownChannel(String),
    #[error("unknown user '{0}'")]
    UnknownUser(String),
    #[error(
        "operation '{op}' is not supported by transport '{kind}': {reason}"
    )]
    Unsupported {
        kind: &'static str,
        op: &'static str,
        reason: String,
    },
    #[error("approval timed out after {0:?}")]
    ApprovalTimeout(std::time::Duration),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

#[async_trait]
pub trait ChannelDirectoryAdapter: Send + Sync {
    fn transport_id(&self) -> &str;
    fn transport_kind(&self) -> &'static str;

    /// List channels the bot can see. Implementations:
    /// - Telegram returns the configured allowlist enriched via
    ///   `getChat`.
    /// - Slack returns the live `conversations.list` for the team.
    async fn list_channels(&self) -> Result<Vec<ChannelSummary>, ChannelOpError>;

    /// List users the bot can see. May be `Unsupported` for transports
    /// that don't expose a user-enumeration API (e.g. Telegram).
    async fn list_users(&self) -> Result<Vec<UserSummary>, ChannelOpError>;

    /// Resolve a single user by id, handle, or email. Returns
    /// `Ok(None)` when the user does not exist; `Err(Unsupported)`
    /// when the transport has no lookup primitive.
    async fn lookup_user(
        &self,
        query: &str,
    ) -> Result<Option<UserSummary>, ChannelOpError>;
}
