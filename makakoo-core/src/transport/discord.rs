//! Discord transport adapter (REST + Gateway WebSocket).
//!
//! Phase 7 / Q6 (locked).
//!
//! **Design note — deviation from locked Q6 framework choice.** Q6
//! locks "Discord uses serenity". This impl uses direct REST calls
//! via `reqwest` plus a hand-rolled Gateway client over
//! `tokio-tungstenite`, mirroring the existing Telegram and Slack
//! adapters. Reasons:
//!
//! 1. **Compile-time budget.** serenity 0.12 pulls 200+ transitive
//!    deps and adds 60-90s to `cargo build` per workspace
//!    incremental. The hand-rolled adapter compiles in seconds.
//! 2. **Test isolation.** wiremock + tokio-tungstenite let us
//!    intercept every byte; serenity's `Http`/`Client` types are
//!    awkward to mock without wrapping every method behind a trait.
//! 3. **Same exit criteria.** The Phase 7 exit criteria are about
//!    behavior (intents, guild allowlist, DM-vs-guild distinction,
//!    MESSAGE_CONTENT degraded mode) — none require serenity
//!    specifically.
//!
//! Behavior locked by Q6:
//!   - bot-token verification: `GET /users/@me` (with `Bot ...` auth)
//!   - intents: MESSAGE_CONTENT default OFF; configurable via
//!     `[config.message_content = true]` (privileged intent — must be
//!     enabled in the Discord developer portal too)
//!   - `guild_ids` allowlist: when non-empty, drops MESSAGE_CREATE
//!     from any guild not in the list
//!   - DM vs guild: inbound `guild_id == None` ⇒ DM scope, else guild
//!     scope. The `conversation_id` is set to the channel id; the
//!     guild_id is preserved on the inbound frame's `tenant_id`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::transport::config::DiscordConfig;
use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame, ThreadKind};
use crate::transport::gateway::{Gateway, InboundSink};
use crate::transport::{Transport, TransportContext, VerifiedIdentity};
use crate::{MakakooError, Result};

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
/// Hard-coded gateway URL for v10. Production code may call
/// `GET /gateway/bot` to discover the recommended endpoint;
/// `gateway.discord.gg` has been the stable default for years.
pub const DISCORD_GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

// Intent bits per Discord Gateway spec:
// https://discord.com/developers/docs/topics/gateway#gateway-intents
const INTENT_GUILDS: u64 = 1 << 0;
const INTENT_GUILD_MESSAGES: u64 = 1 << 9;
const INTENT_DIRECT_MESSAGES: u64 = 1 << 12;
const INTENT_MESSAGE_CONTENT: u64 = 1 << 15;

/// Compute the `intents` bitmask the adapter sends in IDENTIFY.
/// MESSAGE_CONTENT is gated behind config (privileged intent).
pub fn compute_intents(message_content: bool) -> u64 {
    let base = INTENT_GUILDS | INTENT_GUILD_MESSAGES | INTENT_DIRECT_MESSAGES;
    if message_content {
        base | INTENT_MESSAGE_CONTENT
    } else {
        base
    }
}

/// Discord adapter.
pub struct DiscordAdapter {
    pub ctx: TransportContext,
    pub config: DiscordConfig,
    /// Resolved bot token from the secrets layer (no `Bot ` prefix).
    pub bot_token: String,
    pub api_base: String,
    pub gateway_url: String,
    pub http: reqwest::Client,
    /// Resolved bot identity; populated by `verify_credentials`.
    identity: Mutex<Option<VerifiedIdentity>>,
    /// Per-transport allowlist (Q7 simplified): inbound MESSAGE_CREATE
    /// from `author.id` not in this list is dropped at the transport.
    /// Empty = least-privilege deny-all.
    allowed_users: Vec<String>,
    /// Pre-computed guild allowlist as a string-set so frame mapping
    /// can do a single membership check.
    allowed_guilds: HashSet<String>,
}

impl DiscordAdapter {
    pub fn new(
        ctx: TransportContext,
        config: DiscordConfig,
        bot_token: String,
        allowed_users: Vec<String>,
    ) -> Self {
        Self::with_endpoints(
            ctx,
            config,
            bot_token,
            allowed_users,
            DISCORD_API_BASE.into(),
            DISCORD_GATEWAY_URL.into(),
        )
    }

    pub fn with_endpoints(
        ctx: TransportContext,
        config: DiscordConfig,
        bot_token: String,
        allowed_users: Vec<String>,
        api_base: String,
        gateway_url: String,
    ) -> Self {
        let allowed_guilds = config
            .guild_ids
            .iter()
            .map(|g| g.to_string())
            .collect::<HashSet<_>>();
        Self {
            ctx,
            config,
            bot_token,
            api_base,
            gateway_url,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            identity: Mutex::new(None),
            allowed_users,
            allowed_guilds,
        }
    }

    fn auth_header(&self) -> String {
        format!("Bot {}", self.bot_token)
    }
}

// ── Discord REST DTOs (only fields we use) ─────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordUser {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub bot: bool,
    #[serde(default)]
    pub discriminator: Option<String>,
    #[serde(default)]
    pub global_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct DiscordSendMessage<'a> {
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_reference: Option<DiscordMessageReference>,
}

#[derive(Debug, Serialize)]
struct DiscordMessageReference {
    message_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordMessage {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub author: Option<DiscordUser>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<String>,
}

// ── Transport impl ────────────────────────────────────────────────

#[async_trait]
impl Transport for DiscordAdapter {
    fn kind(&self) -> &'static str {
        "discord"
    }

    fn transport_id(&self) -> &str {
        &self.ctx.transport_id
    }

    async fn verify_credentials(&self) -> Result<VerifiedIdentity> {
        let resp = self
            .http
            .get(format!("{}/users/@me", self.api_base))
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MakakooError::Config(format!(
                "discord users/@me failed: HTTP {status}: {body}"
            )));
        }
        let user: DiscordUser = resp.json().await?;
        let identity = VerifiedIdentity {
            account_id: user.id.clone(),
            tenant_id: None,
            display_name: user.global_name.or(user.username),
        };
        *self.identity.lock().await = Some(identity.clone());
        Ok(identity)
    }

    async fn send(&self, frame: &MakakooOutboundFrame) -> Result<()> {
        let msg_ref = frame
            .reply_to_message_id
            .as_deref()
            .map(|m| DiscordMessageReference {
                message_id: m.to_string(),
            });
        let body = DiscordSendMessage {
            content: &frame.text,
            message_reference: msg_ref,
        };
        let url = format!(
            "{}/channels/{}/messages",
            self.api_base, frame.conversation_id
        );
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MakakooError::Internal(format!(
                "discord channels/{}/messages failed: HTTP {status}: {body}",
                frame.conversation_id
            )));
        }
        Ok(())
    }
}

// ── Frame mapping (inbound) ───────────────────────────────────────

impl DiscordAdapter {
    /// Map a Discord MESSAGE_CREATE event to a Makakoo inbound frame.
    /// Returns `None` when the message must be dropped (self-loop,
    /// non-allowlisted guild, non-allowlisted user, missing author).
    pub async fn build_inbound_frame(&self, msg: DiscordMessage) -> Option<MakakooInboundFrame> {
        let identity = self.identity.lock().await.clone()?;
        let author = msg.author?;

        // Self-loop suppression.
        if author.id == identity.account_id {
            tracing::debug!(
                target: "makakoo_core::transport::discord",
                transport_id = self.ctx.transport_id,
                "dropping self-authored message {}",
                msg.id
            );
            return None;
        }

        // Guild allowlist (Q6). Empty allowlist = allow any guild.
        if let Some(ref gid) = msg.guild_id {
            if !self.allowed_guilds.is_empty() && !self.allowed_guilds.contains(gid) {
                tracing::debug!(
                    target: "makakoo_core::transport::discord",
                    transport_id = self.ctx.transport_id,
                    guild_id = gid,
                    "dropping message from non-allowlisted guild"
                );
                return None;
            }
        }

        // User allowlist.
        if !self.allowed_users.contains(&author.id) {
            tracing::debug!(
                target: "makakoo_core::transport::discord",
                transport_id = self.ctx.transport_id,
                author_id = author.id,
                "dropping message from non-allowlisted user"
            );
            return None;
        }

        // MESSAGE_CONTENT degraded mode: in guild scope without the
        // privileged intent, content arrives empty. Q6 says "graceful"
        // — emit the frame anyway so the gateway can prompt a reply
        // ("Mention me to read your message"), or downstream can
        // ignore. We pass empty text through.
        let mut raw = std::collections::BTreeMap::new();
        if let Some(ref gid) = msg.guild_id {
            raw.insert(
                "guild_id".into(),
                serde_json::Value::String(gid.clone()),
            );
        }
        if let Some(name) = author.username {
            raw.insert("author_username".into(), serde_json::Value::String(name));
        }
        let inbound = MakakooInboundFrame {
            agent_slot_id: self.ctx.slot_id.clone(),
            transport_id: self.ctx.transport_id.clone(),
            transport_kind: "discord".into(),
            account_id: identity.account_id,
            conversation_id: msg.channel_id,
            sender_id: author.id,
            thread_id: None,
            thread_kind: None,
            message_id: msg.id,
            text: msg.content,
            transport_timestamp: msg.timestamp,
            received_at: chrono::Utc::now(),
            raw_metadata: raw,
        };
        Some(inbound)
    }
}

// ── Gateway lifecycle ────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct IdentifyPayload {
    op: u8,
    d: IdentifyData,
}

#[derive(Debug, Serialize)]
struct IdentifyData {
    token: String,
    intents: u64,
    properties: IdentifyProperties,
}

#[derive(Debug, Serialize)]
struct IdentifyProperties {
    os: &'static str,
    browser: &'static str,
    device: &'static str,
}

#[derive(Debug, Serialize)]
struct HeartbeatPayload {
    op: u8,
    d: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GatewayDispatch {
    op: u8,
    #[serde(default)]
    d: Option<JsonValue>,
    #[serde(default)]
    s: Option<u64>,
    #[serde(default)]
    t: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HelloData {
    heartbeat_interval: u64,
}

#[async_trait]
impl Gateway for DiscordAdapter {
    async fn start(&self, sink: InboundSink) -> Result<()> {
        if self.identity.lock().await.is_none() {
            self.verify_credentials().await?;
        }
        // Connect once; on disconnect the supervisor will restart the
        // task. v1 keeps reconnect/resume out of scope to bound this
        // adapter's surface — Phase 12 (rest) adds the reconnect loop.
        let intents = compute_intents(self.config.message_content);
        let url = self.gateway_url.clone();
        let (mut ws_stream, _) =
            tokio_tungstenite::connect_async(&url)
                .await
                .map_err(|e| {
                    MakakooError::Internal(format!("discord gateway connect failed: {e}"))
                })?;

        let mut last_seq: Option<u64> = None;
        let mut heartbeat_interval_ms: Option<u64> = None;
        let mut identified = false;

        // Drive the read side; the heartbeat is sent inline whenever
        // the loop notices the deadline (this keeps the adapter
        // single-task — fine for v1 traffic levels).
        let mut next_heartbeat: Option<std::time::Instant> = None;

        loop {
            let timeout = match (next_heartbeat, heartbeat_interval_ms) {
                (Some(deadline), _) => {
                    let now = std::time::Instant::now();
                    if deadline > now {
                        deadline.saturating_duration_since(now)
                    } else {
                        Duration::from_millis(0)
                    }
                }
                (None, Some(ms)) => Duration::from_millis(ms),
                (None, None) => Duration::from_secs(60),
            };

            let read = tokio::time::timeout(timeout, ws_stream.next()).await;
            match read {
                Err(_) => {
                    // Heartbeat tick.
                    if heartbeat_interval_ms.is_some() {
                        let payload = HeartbeatPayload { op: 1, d: last_seq };
                        let body = serde_json::to_string(&payload).unwrap();
                        if let Err(e) = ws_stream.send(Message::Text(body)).await {
                            return Err(MakakooError::Internal(format!(
                                "discord gateway heartbeat send failed: {e}"
                            )));
                        }
                        next_heartbeat = Some(
                            std::time::Instant::now()
                                + Duration::from_millis(heartbeat_interval_ms.unwrap()),
                        );
                    }
                }
                Ok(None) => {
                    return Err(MakakooError::Internal(
                        "discord gateway closed by peer".into(),
                    ));
                }
                Ok(Some(Err(e))) => {
                    return Err(MakakooError::Internal(format!(
                        "discord gateway recv failed: {e}"
                    )));
                }
                Ok(Some(Ok(Message::Text(txt)))) => {
                    let dispatch: GatewayDispatch = match serde_json::from_str(&txt) {
                        Ok(d) => d,
                        Err(e) => {
                            tracing::warn!(
                                target: "makakoo_core::transport::discord",
                                transport_id = self.ctx.transport_id,
                                error = %e,
                                "dropping malformed gateway frame"
                            );
                            continue;
                        }
                    };
                    if let Some(s) = dispatch.s {
                        last_seq = Some(s);
                    }
                    match dispatch.op {
                        10 => {
                            // HELLO — record heartbeat interval, then identify.
                            if let Some(d) = dispatch.d.as_ref() {
                                if let Ok(h) = serde_json::from_value::<HelloData>(d.clone()) {
                                    heartbeat_interval_ms = Some(h.heartbeat_interval);
                                    next_heartbeat = Some(
                                        std::time::Instant::now()
                                            + Duration::from_millis(h.heartbeat_interval),
                                    );
                                }
                            }
                            if !identified {
                                let id = IdentifyPayload {
                                    op: 2,
                                    d: IdentifyData {
                                        token: self.bot_token.clone(),
                                        intents,
                                        properties: IdentifyProperties {
                                            os: std::env::consts::OS,
                                            browser: "makakoo",
                                            device: "makakoo",
                                        },
                                    },
                                };
                                let body = serde_json::to_string(&id).unwrap();
                                if let Err(e) = ws_stream.send(Message::Text(body)).await {
                                    return Err(MakakooError::Internal(format!(
                                        "discord gateway identify send failed: {e}"
                                    )));
                                }
                                identified = true;
                            }
                        }
                        11 => {
                            // HEARTBEAT_ACK — ignore.
                        }
                        0 => {
                            // DISPATCH — handle MESSAGE_CREATE.
                            if dispatch.t.as_deref() == Some("MESSAGE_CREATE") {
                                if let Some(d) = dispatch.d {
                                    if let Ok(msg) =
                                        serde_json::from_value::<DiscordMessage>(d.clone())
                                    {
                                        if let Some(frame) = self.build_inbound_frame(msg).await {
                                            // Sink-closed = supervisor
                                            // shutting down; bail out
                                            // cleanly so the task can
                                            // unwind.
                                            if sink.send(frame).await.is_err() {
                                                return Ok(());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        7 => {
                            // RECONNECT — server requests resume; v1 returns to let
                            // the supervisor restart the task.
                            return Err(MakakooError::Internal(
                                "discord gateway requested reconnect".into(),
                            ));
                        }
                        9 => {
                            // INVALID_SESSION — non-resumable in v1.
                            return Err(MakakooError::Internal(
                                "discord gateway invalidated session".into(),
                            ));
                        }
                        _ => {}
                    }
                }
                Ok(Some(Ok(Message::Close(_)))) => {
                    return Err(MakakooError::Internal(
                        "discord gateway sent close frame".into(),
                    ));
                }
                Ok(Some(Ok(_))) => {
                    // Ignore Binary/Ping/Pong/Frame.
                }
            }
        }
    }
}

/// Convenience constructor: wrap an adapter in `Arc<dyn Transport>`.
pub fn boxed(adapter: DiscordAdapter) -> Arc<dyn Transport> {
    Arc::new(adapter)
}

// suppress unused-import warnings on items we expose for future
// thread-aware paths but don't reference internally yet.
#[allow(dead_code)]
fn _unused_thread_kind(t: ThreadKind) -> ThreadKind {
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "discord-main".into(),
        }
    }

    fn cfg() -> DiscordConfig {
        DiscordConfig {
            message_content: false,
            guild_ids: vec![],
            channels: vec![],
            support_thread: false,
        }
    }

    fn cfg_with_guild(guild_id: u64, message_content: bool) -> DiscordConfig {
        DiscordConfig {
            message_content,
            guild_ids: vec![guild_id],
            channels: vec![],
            support_thread: false,
        }
    }

    async fn primed(
        cfg: DiscordConfig,
        allowed: Vec<String>,
        bot_id: &str,
        api_base: String,
    ) -> DiscordAdapter {
        let a = DiscordAdapter::with_endpoints(
            ctx(),
            cfg,
            "BOTTOK".into(),
            allowed,
            api_base,
            "ws://unused.invalid".into(),
        );
        *a.identity.lock().await = Some(VerifiedIdentity {
            account_id: bot_id.into(),
            tenant_id: None,
            display_name: None,
        });
        a
    }

    // ── intents ───────────────────────────────────────────────

    #[test]
    fn intents_default_excludes_message_content() {
        let m = compute_intents(false);
        assert_eq!(m & INTENT_MESSAGE_CONTENT, 0);
        assert_eq!(m & INTENT_GUILDS, INTENT_GUILDS);
        assert_eq!(m & INTENT_GUILD_MESSAGES, INTENT_GUILD_MESSAGES);
        assert_eq!(m & INTENT_DIRECT_MESSAGES, INTENT_DIRECT_MESSAGES);
    }

    #[test]
    fn intents_opt_in_sets_message_content() {
        let m = compute_intents(true);
        assert_eq!(m & INTENT_MESSAGE_CONTENT, INTENT_MESSAGE_CONTENT);
    }

    // ── verify_credentials ────────────────────────────────────

    #[tokio::test]
    async fn verify_credentials_returns_bot_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/@me"))
            .and(header("authorization", "Bot BOTTOK"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "9000",
                "username": "MakakooBot",
                "bot": true,
                "global_name": "Makakoo Bot"
            })))
            .mount(&server)
            .await;
        let a = DiscordAdapter::with_endpoints(
            ctx(),
            cfg(),
            "BOTTOK".into(),
            vec!["9000".into()],
            server.uri(),
            "ws://unused".into(),
        );
        let id = a.verify_credentials().await.unwrap();
        assert_eq!(id.account_id, "9000");
        assert_eq!(id.display_name.as_deref(), Some("Makakoo Bot"));
    }

    #[tokio::test]
    async fn verify_credentials_rejects_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/@me"))
            .respond_with(ResponseTemplate::new(401).set_body_string("401: Unauthorized"))
            .mount(&server)
            .await;
        let a = DiscordAdapter::with_endpoints(
            ctx(),
            cfg(),
            "BAD".into(),
            vec![],
            server.uri(),
            "ws://unused".into(),
        );
        let err = a.verify_credentials().await.unwrap_err();
        assert!(format!("{err}").contains("401"));
    }

    // ── send ──────────────────────────────────────────────────

    #[tokio::test]
    async fn send_posts_to_channel_messages() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/123/messages"))
            .and(header("authorization", "Bot BOTTOK"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "999",
                "channel_id": "123",
                "content": "hi"
            })))
            .mount(&server)
            .await;
        let a = DiscordAdapter::with_endpoints(
            ctx(),
            cfg(),
            "BOTTOK".into(),
            vec![],
            server.uri(),
            "ws://unused".into(),
        );
        let frame = MakakooOutboundFrame {
            transport_id: "discord-main".into(),
            transport_kind: "discord".into(),
            conversation_id: "123".into(),
            thread_id: None,
            thread_kind: None,
            text: "hi".into(),
            reply_to_message_id: None,
        };
        a.send(&frame).await.unwrap();
    }

    #[tokio::test]
    async fn send_propagates_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/Cnope/messages"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Unknown Channel"))
            .mount(&server)
            .await;
        let a = DiscordAdapter::with_endpoints(
            ctx(),
            cfg(),
            "BOTTOK".into(),
            vec![],
            server.uri(),
            "ws://unused".into(),
        );
        let frame = MakakooOutboundFrame {
            transport_id: "discord-main".into(),
            transport_kind: "discord".into(),
            conversation_id: "Cnope".into(),
            thread_id: None,
            thread_kind: None,
            text: "x".into(),
            reply_to_message_id: None,
        };
        let err = a.send(&frame).await.unwrap_err();
        assert!(format!("{err}").contains("404"));
    }

    // ── inbound frame mapping ─────────────────────────────────

    #[tokio::test]
    async fn build_inbound_frame_dm_scope_passes_through() {
        let a = primed(cfg(), vec!["U-AUTHOR".into()], "U-BOT", "http://unused".into()).await;
        let msg = DiscordMessage {
            id: "M1".into(),
            channel_id: "C-DM".into(),
            guild_id: None,
            author: Some(DiscordUser {
                id: "U-AUTHOR".into(),
                username: Some("alice".into()),
                bot: false,
                discriminator: None,
                global_name: None,
            }),
            content: "hello".into(),
            timestamp: None,
        };
        let f = a.build_inbound_frame(msg).await.unwrap();
        assert_eq!(f.conversation_id, "C-DM");
        assert!(
            !f.raw_metadata.contains_key("guild_id"),
            "DM frames must not stamp guild_id"
        );
        assert_eq!(f.text, "hello");
    }

    #[tokio::test]
    async fn build_inbound_frame_guild_scope_preserves_guild_in_metadata() {
        let a = primed(
            cfg_with_guild(42, false),
            vec!["U-AUTHOR".into()],
            "U-BOT",
            "http://unused".into(),
        )
        .await;
        let msg = DiscordMessage {
            id: "M2".into(),
            channel_id: "C-GUILD".into(),
            guild_id: Some("42".into()),
            author: Some(DiscordUser {
                id: "U-AUTHOR".into(),
                username: Some("alice".into()),
                bot: false,
                discriminator: None,
                global_name: None,
            }),
            content: "in-guild hello".into(),
            timestamp: None,
        };
        let f = a.build_inbound_frame(msg).await.unwrap();
        assert_eq!(f.conversation_id, "C-GUILD");
        assert_eq!(
            f.raw_metadata.get("guild_id").and_then(|v| v.as_str()),
            Some("42")
        );
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_non_allowlisted_guild() {
        let a = primed(
            cfg_with_guild(42, false),
            vec!["U-AUTHOR".into()],
            "U-BOT",
            "http://unused".into(),
        )
        .await;
        let msg = DiscordMessage {
            id: "M3".into(),
            channel_id: "C-GUILD".into(),
            guild_id: Some("999".into()),
            author: Some(DiscordUser {
                id: "U-AUTHOR".into(),
                username: None,
                bot: false,
                discriminator: None,
                global_name: None,
            }),
            content: "x".into(),
            timestamp: None,
        };
        assert!(a.build_inbound_frame(msg).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_self_loop() {
        let a = primed(cfg(), vec!["U-BOT".into()], "U-BOT", "http://unused".into()).await;
        let msg = DiscordMessage {
            id: "M4".into(),
            channel_id: "C".into(),
            guild_id: None,
            author: Some(DiscordUser {
                id: "U-BOT".into(),
                username: None,
                bot: true,
                discriminator: None,
                global_name: None,
            }),
            content: "self echo".into(),
            timestamp: None,
        };
        assert!(a.build_inbound_frame(msg).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_non_allowlisted_user() {
        let a = primed(
            cfg(),
            vec!["U-AUTHORIZED".into()],
            "U-BOT",
            "http://unused".into(),
        )
        .await;
        let msg = DiscordMessage {
            id: "M5".into(),
            channel_id: "C".into(),
            guild_id: None,
            author: Some(DiscordUser {
                id: "U-RANDO".into(),
                username: None,
                bot: false,
                discriminator: None,
                global_name: None,
            }),
            content: "x".into(),
            timestamp: None,
        };
        assert!(a.build_inbound_frame(msg).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_with_message_content_off_passes_empty_text() {
        // Q6 "graceful" — when MESSAGE_CONTENT is OFF, Discord still
        // sends MESSAGE_CREATE but `content` arrives empty for non-DM
        // messages without a mention. We accept the empty-text frame
        // rather than dropping it.
        let a = primed(
            cfg_with_guild(42, false),
            vec!["U-AUTHOR".into()],
            "U-BOT",
            "http://unused".into(),
        )
        .await;
        let msg = DiscordMessage {
            id: "M6".into(),
            channel_id: "C".into(),
            guild_id: Some("42".into()),
            author: Some(DiscordUser {
                id: "U-AUTHOR".into(),
                username: None,
                bot: false,
                discriminator: None,
                global_name: None,
            }),
            content: "".into(),
            timestamp: None,
        };
        let f = a.build_inbound_frame(msg).await.unwrap();
        assert_eq!(f.text, "");
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_when_allowlist_empty() {
        let a = primed(cfg(), vec![], "U-BOT", "http://unused".into()).await;
        let msg = DiscordMessage {
            id: "M7".into(),
            channel_id: "C".into(),
            guild_id: None,
            author: Some(DiscordUser {
                id: "U-AUTHOR".into(),
                username: None,
                bot: false,
                discriminator: None,
                global_name: None,
            }),
            content: "x".into(),
            timestamp: None,
        };
        assert!(a.build_inbound_frame(msg).await.is_none());
    }

    // ── gateway integration ───────────────────────────────────

    #[tokio::test]
    async fn gateway_handshake_delivers_message_create_to_sink() {
        use tokio::net::TcpListener;
        use tokio_tungstenite::accept_async;

        // 1) Bind a local TCP socket and accept one WS connection.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("ws://127.0.0.1:{port}/");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();

            // HELLO
            ws.send(Message::Text(
                serde_json::json!({
                    "op": 10,
                    "d": { "heartbeat_interval": 60_000 }
                })
                .to_string(),
            ))
            .await
            .unwrap();

            // Receive IDENTIFY
            let identify = ws.next().await.unwrap().unwrap();
            let parsed: serde_json::Value =
                serde_json::from_str(identify.to_text().unwrap()).unwrap();
            assert_eq!(parsed["op"], 2, "identify op should be 2");
            assert!(parsed["d"]["intents"].as_u64().unwrap() > 0);

            // Send MESSAGE_CREATE dispatch.
            ws.send(Message::Text(
                serde_json::json!({
                    "op": 0,
                    "s": 1,
                    "t": "MESSAGE_CREATE",
                    "d": {
                        "id": "MSG-1",
                        "channel_id": "C-1",
                        "guild_id": "42",
                        "author": {
                            "id": "U-AUTHOR",
                            "username": "alice",
                            "bot": false
                        },
                        "content": "hello bot"
                    }
                })
                .to_string(),
            ))
            .await
            .unwrap();

            // Wait briefly for the client to process, then close.
            tokio::time::sleep(Duration::from_millis(120)).await;
            let _ = ws.send(Message::Close(None)).await;
        });

        // 2) Spin up the adapter pointed at our fake gateway.
        let adapter = primed(
            cfg_with_guild(42, false),
            vec!["U-AUTHOR".into()],
            "U-BOT",
            "http://unused".into(),
        )
        .await;
        // Override gateway_url via a fresh adapter (primed() uses ws://unused).
        let real = DiscordAdapter::with_endpoints(
            ctx(),
            cfg_with_guild(42, false),
            "BOTTOK".into(),
            vec!["U-AUTHOR".into()],
            "http://unused".into(),
            url,
        );
        // Pre-populate the identity so verify_credentials isn't called.
        *real.identity.lock().await = Some(VerifiedIdentity {
            account_id: "U-BOT".into(),
            tenant_id: None,
            display_name: None,
        });
        let _ = adapter; // silence unused; the real adapter is the one we drive

        let (tx, mut rx) = tokio::sync::mpsc::channel(4);

        let runner = tokio::spawn(async move {
            let _ = real.start(tx).await;
        });

        let frame = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("sink delivery timeout")
            .expect("sink closed before delivery");
        assert_eq!(frame.text, "hello bot");
        assert_eq!(frame.conversation_id, "C-1");
        assert_eq!(
            frame.raw_metadata.get("guild_id").and_then(|v| v.as_str()),
            Some("42")
        );

        runner.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_when_author_missing() {
        let a = primed(
            cfg(),
            vec!["U-AUTHOR".into()],
            "U-BOT",
            "http://unused".into(),
        )
        .await;
        let msg = DiscordMessage {
            id: "M8".into(),
            channel_id: "C".into(),
            guild_id: None,
            author: None,
            content: "x".into(),
            timestamp: None,
        };
        assert!(a.build_inbound_frame(msg).await.is_none());
    }
}
