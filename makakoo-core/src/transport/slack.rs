//! Slack transport adapter (Socket Mode).
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS Q11 + Phase 1:
//!   - bot-token verification: `auth.test` HTTP call (rejects on
//!     `team_id` mismatch).
//!   - app-token Socket Mode probe: `apps.connections.open` HTTP
//!     call (returns a `wss://` URL).
//!   - WebSocket lifecycle: dial the wss URL, send `acknowledge`
//!     for each envelope, exponential-backoff reconnect (1 s → 60 s
//!     jittered) on disconnect, emit `status.reconnecting` on
//!     reconnect path.
//!   - Inbound de-dup: 5-minute sliding window keyed on
//!     `(channel, event.ts)`.
//!   - Self-loop suppression: drop events where `event.user` matches
//!     our resolved bot user_id, OR where `event.bot_id` is set.
//!   - `dm_only` / `channels` allowlist enforcement on inbound.
//!   - Outbound `chat.postMessage` with `thread_ts` and reply-target
//!     coercion.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::transport::config::SlackConfig;
use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame, ThreadKind};
use crate::transport::gateway::{Gateway, InboundSink};
use crate::transport::{Transport, TransportContext, VerifiedIdentity};
use crate::{MakakooError, Result};

const SLACK_API_BASE: &str = "https://slack.com/api";
const RECONNECT_INITIAL_MS: u64 = 1_000;
const RECONNECT_CAP_MS: u64 = 60_000;
const DEDUP_WINDOW: Duration = Duration::from_secs(300);

/// Slack adapter (Socket Mode).
pub struct SlackAdapter {
    pub ctx: TransportContext,
    pub config: SlackConfig,
    pub bot_token: String,
    pub app_token: String,
    pub api_base: String,
    pub http: reqwest::Client,
    /// Resolved bot identity (`auth.test.bot_id`/`user_id` +
    /// `team_id`) — populated by `verify_credentials`.  Used to
    /// stamp `account_id` on inbound frames and to suppress
    /// self-loop events.
    identity: Mutex<Option<VerifiedIdentity>>,
    /// Recent `(channel, ts)` keys we've already delivered.  Used
    /// to drop duplicates that arrive over multiple Socket Mode
    /// connections within `DEDUP_WINDOW`.
    dedup: Mutex<HashMap<(String, String), Instant>>,
}

impl SlackAdapter {
    pub fn new(
        ctx: TransportContext,
        config: SlackConfig,
        bot_token: String,
        app_token: String,
    ) -> Self {
        Self::with_api_base(ctx, config, bot_token, app_token, SLACK_API_BASE.into())
    }

    pub fn with_api_base(
        ctx: TransportContext,
        config: SlackConfig,
        bot_token: String,
        app_token: String,
        api_base: String,
    ) -> Self {
        Self {
            ctx,
            config,
            bot_token,
            app_token,
            api_base,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            identity: Mutex::new(None),
            dedup: Mutex::new(HashMap::new()),
        }
    }
}

// ── Slack API DTOs (only the fields we use) ────────────────────────

#[derive(Debug, Deserialize)]
struct SlackResponse {
    ok: bool,
    error: Option<String>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SlackAuthTest {
    pub team_id: String,
    pub bot_id: Option<String>,
    pub user_id: Option<String>,
    pub team: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppsConnectionsOpenResp {
    url: String,
}

#[derive(Debug, Serialize)]
struct ChatPostMessage<'a> {
    channel: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<&'a str>,
}

/// One inbound Events API envelope as delivered over Socket Mode.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct SlackEnvelope {
    pub envelope_id: Option<String>,
    pub payload: SlackEventPayload,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct SlackEventPayload {
    pub team_id: String,
    pub event: SlackEvent,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SlackEvent {
    Message(SlackMessageEvent),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct SlackMessageEvent {
    pub channel: String,
    /// Some Slack message subtypes (`message_changed`,
    /// `message_deleted`, `bot_message` from older apps) omit
    /// `user` — we drop those events at frame-mapping time.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    pub ts: String,
    pub thread_ts: Option<String>,
    pub channel_type: Option<String>,
    pub bot_id: Option<String>,
    /// Subtype tag — `Some("message_changed")`, `"message_deleted"`,
    /// etc.  v1 only delivers events with no subtype (regular
    /// user messages).
    pub subtype: Option<String>,
}

/// Socket Mode envelope wrapper used both for inbound payloads and
/// the `acknowledge` outbound.  We tag-decode on the `type` field.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SocketFrame {
    EventsApi {
        envelope_id: String,
        payload: SlackEventPayload,
    },
    Hello {
        #[serde(default)]
        debug_info: Option<serde_json::Value>,
    },
    Disconnect {
        #[serde(default)]
        reason: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Serialize)]
struct SocketAck<'a> {
    envelope_id: &'a str,
}

// ── Transport impl ────────────────────────────────────────────────

#[async_trait]
impl Transport for SlackAdapter {
    fn kind(&self) -> &'static str {
        "slack"
    }

    fn transport_id(&self) -> &str {
        &self.ctx.transport_id
    }

    async fn verify_credentials(&self) -> Result<VerifiedIdentity> {
        // Step 1: auth.test with bot token.
        let resp: SlackResponse = self
            .http
            .post(format!("{}/auth.test", self.api_base))
            .bearer_auth(&self.bot_token)
            .send()
            .await?
            .json()
            .await?;
        if !resp.ok {
            return Err(MakakooError::Config(format!(
                "slack auth.test failed: {}",
                resp.error.unwrap_or_else(|| "unknown".into())
            )));
        }
        let auth: SlackAuthTest =
            serde_json::from_value(serde_json::Value::Object(resp.rest)).map_err(|e| {
                MakakooError::Internal(format!("slack auth.test response parse: {}", e))
            })?;
        if auth.team_id != self.config.team_id {
            return Err(MakakooError::Config(format!(
                "slack team_id mismatch: TOML='{}' but auth.test returned '{}'",
                self.config.team_id, auth.team_id
            )));
        }

        // Step 2: apps.connections.open with app token (probe Socket Mode).
        let probe: SlackResponse = self
            .http
            .post(format!("{}/apps.connections.open", self.api_base))
            .bearer_auth(&self.app_token)
            .send()
            .await?
            .json()
            .await?;
        if !probe.ok {
            return Err(MakakooError::Config(format!(
                "slack apps.connections.open (Socket Mode probe) failed: {}",
                probe.error.unwrap_or_else(|| "unknown".into())
            )));
        }

        let account_id = auth
            .bot_id
            .clone()
            .or(auth.user_id.clone())
            .unwrap_or_else(|| "unknown".into());
        let identity = VerifiedIdentity {
            account_id,
            tenant_id: Some(auth.team_id.clone()),
            display_name: auth.team.clone(),
        };
        *self.identity.lock().await = Some(identity.clone());
        Ok(identity)
    }

    async fn send(&self, frame: &MakakooOutboundFrame) -> Result<()> {
        let thread_ts = if self.config.support_thread {
            match (&frame.thread_id, &frame.thread_kind) {
                (Some(s), Some(ThreadKind::SlackThread)) => Some(s.as_str()),
                (Some(_), Some(ThreadKind::TelegramForum)) => {
                    tracing::warn!(
                        target: "makakoo_core::transport::slack",
                        transport_id = self.ctx.transport_id,
                        "outbound thread_kind=telegram_forum cannot ride a slack transport — dropping thread anchor"
                    );
                    None
                }
                _ => None,
            }
        } else {
            None
        };

        // Reply-to coercion (Q11): only honor when looks like a
        // valid Slack thread_ts (`^\d+\.\d+$`).
        let reply_thread_ts = if thread_ts.is_none() {
            frame.reply_to_message_id.as_deref().and_then(|raw| {
                let looks_like_ts = raw
                    .split_once('.')
                    .map(|(a, b)| {
                        !a.is_empty() && !b.is_empty()
                            && a.chars().all(|c| c.is_ascii_digit())
                            && b.chars().all(|c| c.is_ascii_digit())
                    })
                    .unwrap_or(false);
                if looks_like_ts {
                    Some(raw)
                } else {
                    tracing::warn!(
                        target: "makakoo_core::transport::slack",
                        transport_id = self.ctx.transport_id,
                        reply_to_message_id = raw,
                        "non-thread_ts reply_to_message_id — dropping thread anchor, sending without reply target"
                    );
                    None
                }
            })
        } else {
            None
        };

        let body = ChatPostMessage {
            channel: &frame.conversation_id,
            text: &frame.text,
            thread_ts: thread_ts.or(reply_thread_ts),
        };
        let resp: SlackResponse = self
            .http
            .post(format!("{}/chat.postMessage", self.api_base))
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if !resp.ok {
            return Err(MakakooError::Internal(format!(
                "slack chat.postMessage failed: {}",
                resp.error.unwrap_or_else(|| "unknown".into())
            )));
        }
        Ok(())
    }
}

// ── Gateway (Socket Mode loop) ────────────────────────────────────

#[async_trait]
impl Gateway for SlackAdapter {
    async fn start(&self, sink: InboundSink) -> Result<()> {
        // Verify credentials (also caches the bot identity).
        if self.identity.lock().await.is_none() {
            self.verify_credentials().await?;
        }
        let mut backoff_ms = RECONNECT_INITIAL_MS;
        loop {
            // Open a fresh Socket Mode session for each connection.
            let url = match self.open_socket_url().await {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(
                        target: "makakoo_core::transport::slack",
                        transport_id = self.ctx.transport_id,
                        error = %e,
                        backoff_ms,
                        "apps.connections.open failed — reconnecting after backoff"
                    );
                    self.sleep_backoff(backoff_ms).await;
                    backoff_ms = (backoff_ms.saturating_mul(2)).min(RECONNECT_CAP_MS);
                    continue;
                }
            };
            tracing::info!(
                target: "makakoo_core::transport::slack",
                transport_id = self.ctx.transport_id,
                event = "status.reconnecting",
                "dialing slack socket mode wss"
            );
            match self.run_socket_session(&url, &sink).await {
                Ok(reason) => {
                    tracing::info!(
                        target: "makakoo_core::transport::slack",
                        transport_id = self.ctx.transport_id,
                        reason = %reason,
                        "slack socket session ended cleanly — reconnecting"
                    );
                    backoff_ms = RECONNECT_INITIAL_MS;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "makakoo_core::transport::slack",
                        transport_id = self.ctx.transport_id,
                        error = %e,
                        backoff_ms,
                        "slack socket session error — reconnecting after backoff"
                    );
                    self.sleep_backoff(backoff_ms).await;
                    backoff_ms = (backoff_ms.saturating_mul(2)).min(RECONNECT_CAP_MS);
                }
            }
        }
    }
}

impl SlackAdapter {
    async fn sleep_backoff(&self, base_ms: u64) {
        use rand::Rng;
        let jitter = rand::thread_rng().gen_range(base_ms / 2..=base_ms);
        tokio::time::sleep(Duration::from_millis(jitter)).await;
    }

    async fn open_socket_url(&self) -> Result<String> {
        let resp: SlackResponse = self
            .http
            .post(format!("{}/apps.connections.open", self.api_base))
            .bearer_auth(&self.app_token)
            .send()
            .await?
            .json()
            .await?;
        if !resp.ok {
            return Err(MakakooError::Config(format!(
                "slack apps.connections.open failed: {}",
                resp.error.unwrap_or_else(|| "unknown".into())
            )));
        }
        let parsed: AppsConnectionsOpenResp =
            serde_json::from_value(serde_json::Value::Object(resp.rest))
                .map_err(|e| MakakooError::Internal(format!("apps.connections.open parse: {}", e)))?;
        Ok(parsed.url)
    }

    /// Run one Socket Mode WebSocket session.  Returns when the
    /// server sends a `disconnect` frame (with the reason) or
    /// errors when the WebSocket closes unexpectedly.
    async fn run_socket_session(&self, url: &str, sink: &InboundSink) -> Result<String> {
        let (ws_stream, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| MakakooError::Internal(format!("slack ws connect: {}", e)))?;
        let (mut writer, mut reader) = ws_stream.split();
        while let Some(msg) = reader.next().await {
            let msg = msg.map_err(|e| MakakooError::Internal(format!("slack ws recv: {}", e)))?;
            let text = match msg {
                Message::Text(t) => t,
                Message::Ping(payload) => {
                    let _ = writer.send(Message::Pong(payload)).await;
                    continue;
                }
                Message::Close(_) => {
                    return Ok("ws-close".into());
                }
                _ => continue,
            };
            let frame: SocketFrame = match serde_json::from_str(&text) {
                Ok(f) => f,
                Err(e) => {
                    tracing::debug!(
                        target: "makakoo_core::transport::slack",
                        transport_id = self.ctx.transport_id,
                        error = %e,
                        text = %text,
                        "slack ws frame parse failed — skipping"
                    );
                    continue;
                }
            };
            match frame {
                SocketFrame::Hello { .. } => continue,
                SocketFrame::Disconnect { reason } => {
                    return Ok(reason.unwrap_or_else(|| "disconnect".into()));
                }
                SocketFrame::Other => continue,
                SocketFrame::EventsApi {
                    envelope_id,
                    payload,
                } => {
                    // Acknowledge first — Slack expects an ack
                    // within ~3s or it'll redeliver.
                    let ack = serde_json::to_string(&SocketAck {
                        envelope_id: &envelope_id,
                    })
                    .map_err(|e| MakakooError::Internal(format!("ack serialise: {}", e)))?;
                    if let Err(e) = writer.send(Message::Text(ack)).await {
                        return Err(MakakooError::Internal(format!("slack ws ack: {}", e)));
                    }
                    let envelope = SlackEnvelope {
                        envelope_id: Some(envelope_id),
                        payload,
                    };
                    if let Some(frame) = self.build_inbound_frame(envelope).await {
                        if sink.send(frame).await.is_err() {
                            tracing::error!(
                                target: "makakoo_core::transport::slack",
                                transport_id = self.ctx.transport_id,
                                "inbound sink closed — slack listener exiting"
                            );
                            return Ok("sink-closed".into());
                        }
                    }
                }
            }
        }
        Ok("ws-eof".into())
    }

    /// Build an inbound frame from a Socket Mode Events API
    /// envelope.  Async because it consults the dedup map and the
    /// cached identity.  Returns `None` for events we don't deliver
    /// in v1 (non-`message`, subtype != regular, bot echo, self-loop,
    /// blocked by allowlists, dedup hit, team mismatch).
    pub(crate) async fn build_inbound_frame(
        &self,
        envelope: SlackEnvelope,
    ) -> Option<MakakooInboundFrame> {
        let team_id = envelope.payload.team_id.clone();
        let SlackEvent::Message(msg) = envelope.payload.event else {
            return None;
        };
        // Drop edited / deleted / channel-system messages.
        if msg.subtype.is_some() {
            tracing::debug!(
                target: "makakoo_core::transport::slack",
                transport_id = self.ctx.transport_id,
                subtype = ?msg.subtype,
                "dropping slack message with non-empty subtype"
            );
            return None;
        }
        // Suppress bot echoes — neither generic bot_id nor our
        // own user_id should be delivered to the LLM.
        if msg.bot_id.is_some() {
            return None;
        }
        let identity = self.identity.lock().await.clone();
        if let (Some(user), Some(id)) = (msg.user.as_ref(), identity.as_ref()) {
            if user == &id.account_id {
                tracing::debug!(
                    target: "makakoo_core::transport::slack",
                    transport_id = self.ctx.transport_id,
                    "suppressing slack self-loop event (user matches own bot id)"
                );
                return None;
            }
            // Also reject if the inbound team_id doesn't match the
            // adapter's verified tenant.
            if let Some(expected) = identity.as_ref().and_then(|i| i.tenant_id.as_ref()) {
                if expected != &team_id {
                    tracing::warn!(
                        target: "makakoo_core::transport::slack",
                        transport_id = self.ctx.transport_id,
                        inbound_team_id = team_id,
                        expected_team_id = expected,
                        "dropping slack event from unexpected team_id"
                    );
                    return None;
                }
            }
        }
        let user = msg.user.as_ref()?.clone();

        // Allowlist enforcement.
        if !self.config.dm_only {
            if !self.config.channels.iter().any(|c| c == &msg.channel) {
                return None;
            }
        } else {
            // dm_only: drop channel events (channel ids start with C).
            if msg.channel_type.as_deref() != Some("im")
                && !msg.channel.starts_with('D')
            {
                return None;
            }
        }

        // Dedup: 5-minute sliding window keyed on (channel, ts).
        let key = (msg.channel.clone(), msg.ts.clone());
        {
            let mut dedup = self.dedup.lock().await;
            // Sweep stale entries while we're here.
            let cutoff = Instant::now() - DEDUP_WINDOW;
            dedup.retain(|_, t| *t > cutoff);
            if dedup.contains_key(&key) {
                tracing::debug!(
                    target: "makakoo_core::transport::slack",
                    transport_id = self.ctx.transport_id,
                    channel = msg.channel,
                    ts = msg.ts,
                    "dropping slack duplicate within 5-minute window"
                );
                return None;
            }
            dedup.insert(key, Instant::now());
        }

        let (thread_id, thread_kind) = match &msg.thread_ts {
            Some(ts) if self.config.support_thread => {
                (Some(ts.clone()), Some(ThreadKind::SlackThread))
            }
            _ => (None, None),
        };
        let account_id = identity
            .as_ref()
            .map(|i| i.account_id.clone())
            .unwrap_or_default();
        let text = msg.text.clone().unwrap_or_default();
        Some(MakakooInboundFrame {
            agent_slot_id: self.ctx.slot_id.clone(),
            transport_id: self.ctx.transport_id.clone(),
            transport_kind: "slack".into(),
            account_id,
            conversation_id: msg.channel.clone(),
            sender_id: user,
            thread_id,
            thread_kind,
            message_id: msg.ts.clone(),
            text,
            transport_timestamp: Some(msg.ts.clone()),
            received_at: chrono::Utc::now(),
            raw_metadata: Default::default(),
        })
    }
}

/// Wrap an adapter in `Arc<dyn Transport>` for router registration.
pub fn boxed(adapter: SlackAdapter) -> Arc<dyn Transport> {
    Arc::new(adapter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "slack-main".into(),
        }
    }

    fn config() -> SlackConfig {
        SlackConfig {
            team_id: "T0123ABCD".into(),
            mode: "socket".into(),
            dm_only: true,
            channels: vec![],
            support_thread: false,
        }
    }

    fn envelope(
        team_id: &str,
        channel: &str,
        user: Option<&str>,
        ts: &str,
        thread_ts: Option<&str>,
        bot_id: Option<&str>,
        subtype: Option<&str>,
        channel_type: Option<&str>,
    ) -> SlackEnvelope {
        SlackEnvelope {
            envelope_id: Some("env-1".into()),
            payload: SlackEventPayload {
                team_id: team_id.into(),
                event: SlackEvent::Message(SlackMessageEvent {
                    channel: channel.into(),
                    user: user.map(Into::into),
                    text: Some("hi".into()),
                    ts: ts.into(),
                    thread_ts: thread_ts.map(Into::into),
                    channel_type: channel_type.map(Into::into),
                    bot_id: bot_id.map(Into::into),
                    subtype: subtype.map(Into::into),
                }),
            },
        }
    }

    async fn primed_adapter(
        cfg: SlackConfig,
        team_id: &str,
        bot_id: &str,
    ) -> SlackAdapter {
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            cfg,
            "xoxb".into(),
            "xapp".into(),
            "http://unused.invalid".into(),
        );
        *adapter.identity.lock().await = Some(VerifiedIdentity {
            account_id: bot_id.into(),
            tenant_id: Some(team_id.into()),
            display_name: None,
        });
        adapter
    }

    #[tokio::test]
    async fn verify_credentials_succeeds_with_matching_team() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "team_id": "T0123ABCD",
                "bot_id": "B0123BOT", "team": "Acme"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apps.connections.open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "url": "wss://wss.slack.com/socket/123"
            })))
            .mount(&server)
            .await;
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            config(),
            "xoxb".into(),
            "xapp".into(),
            server.uri(),
        );
        let id = adapter.verify_credentials().await.unwrap();
        assert_eq!(id.account_id, "B0123BOT");
        assert_eq!(id.tenant_id.as_deref(), Some("T0123ABCD"));
        // Identity cached for inbound use.
        assert!(adapter.identity.lock().await.is_some());
    }

    #[tokio::test]
    async fn verify_credentials_rejects_team_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "team_id": "T9999OTHER", "bot_id": "B0123BOT"
            })))
            .mount(&server)
            .await;
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            config(),
            "xoxb".into(),
            "xapp".into(),
            server.uri(),
        );
        let err = adapter.verify_credentials().await.unwrap_err();
        assert!(format!("{err}").contains("team_id mismatch"));
    }

    #[tokio::test]
    async fn verify_credentials_rejects_bad_bot_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false, "error": "invalid_auth"
            })))
            .mount(&server)
            .await;
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            config(),
            "xoxb".into(),
            "xapp".into(),
            server.uri(),
        );
        let err = adapter.verify_credentials().await.unwrap_err();
        assert!(format!("{err}").contains("invalid_auth"));
    }

    #[tokio::test]
    async fn verify_credentials_rejects_bad_app_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "team_id": "T0123ABCD", "bot_id": "B0123BOT"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apps.connections.open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false, "error": "not_allowed_token_type"
            })))
            .mount(&server)
            .await;
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            config(),
            "xoxb".into(),
            "xapp".into(),
            server.uri(),
        );
        let err = adapter.verify_credentials().await.unwrap_err();
        assert!(format!("{err}").contains("Socket Mode"));
    }

    #[tokio::test]
    async fn send_drops_telegram_thread_kind_on_slack() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true
            })))
            .mount(&server)
            .await;
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            SlackConfig {
                support_thread: true,
                ..config()
            },
            "xoxb".into(),
            "xapp".into(),
            server.uri(),
        );
        let frame = MakakooOutboundFrame {
            transport_id: "slack-main".into(),
            transport_kind: "slack".into(),
            conversation_id: "D0123ABCD".into(),
            thread_id: Some("99".into()),
            thread_kind: Some(ThreadKind::TelegramForum),
            text: "hi".into(),
            reply_to_message_id: None,
        };
        adapter.send(&frame).await.unwrap();
    }

    #[tokio::test]
    async fn send_drops_non_thread_ts_reply_to() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true
            })))
            .mount(&server)
            .await;
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            config(),
            "xoxb".into(),
            "xapp".into(),
            server.uri(),
        );
        let frame = MakakooOutboundFrame {
            transport_id: "slack-main".into(),
            transport_kind: "slack".into(),
            conversation_id: "D0123ABCD".into(),
            thread_id: None,
            thread_kind: None,
            text: "hi".into(),
            reply_to_message_id: Some("42".into()),
        };
        adapter.send(&frame).await.unwrap();
    }

    #[tokio::test]
    async fn build_inbound_frame_dm() {
        let adapter = primed_adapter(config(), "T0123ABCD", "B0123BOT").await;
        let env = envelope(
            "T0123ABCD",
            "D0123ABCD",
            Some("U0123USER"),
            "1714123456.000100",
            None,
            None,
            None,
            Some("im"),
        );
        let frame = adapter.build_inbound_frame(env).await.unwrap();
        assert_eq!(frame.transport_kind, "slack");
        assert_eq!(frame.conversation_id, "D0123ABCD");
        assert_eq!(frame.sender_id, "U0123USER");
        assert_eq!(frame.message_id, "1714123456.000100");
        assert_eq!(frame.account_id, "B0123BOT");
    }

    #[tokio::test]
    async fn build_inbound_frame_channel() {
        let mut cfg = config();
        cfg.dm_only = false;
        cfg.channels = vec!["C0123DEFG".into()];
        let adapter = primed_adapter(cfg, "T0123ABCD", "B0123BOT").await;
        let env = envelope(
            "T0123ABCD",
            "C0123DEFG",
            Some("U0123USER"),
            "1714123456.000100",
            None,
            None,
            None,
            Some("channel"),
        );
        let frame = adapter.build_inbound_frame(env).await.unwrap();
        assert_eq!(frame.conversation_id, "C0123DEFG");
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_bot_echoes() {
        let adapter = primed_adapter(config(), "T0123ABCD", "B0123BOT").await;
        let env = envelope(
            "T0123ABCD",
            "D0123ABCD",
            Some("U0123BOTUSER"),
            "1714123456.000100",
            None,
            Some("B0123BOT"),
            None,
            Some("im"),
        );
        assert!(adapter.build_inbound_frame(env).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_self_loop() {
        let adapter = primed_adapter(config(), "T0123ABCD", "U0123BOT").await;
        let env = envelope(
            "T0123ABCD",
            "D0123ABCD",
            Some("U0123BOT"), // SAME as cached account_id
            "1714123456.000100",
            None,
            None,
            None,
            Some("im"),
        );
        assert!(adapter.build_inbound_frame(env).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_subtype() {
        let adapter = primed_adapter(config(), "T0123ABCD", "B0123BOT").await;
        let env = envelope(
            "T0123ABCD",
            "D0123ABCD",
            Some("U0123USER"),
            "1714123456.000100",
            None,
            None,
            Some("message_changed"),
            Some("im"),
        );
        assert!(adapter.build_inbound_frame(env).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_unexpected_team() {
        let adapter = primed_adapter(config(), "T0123ABCD", "B0123BOT").await;
        let env = envelope(
            "T9999OTHER",
            "D0123ABCD",
            Some("U0123USER"),
            "1714123456.000100",
            None,
            None,
            None,
            Some("im"),
        );
        assert!(adapter.build_inbound_frame(env).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_channel_event_in_dm_only() {
        // dm_only=true; a channel event (channel id starts with C)
        // must be dropped.
        let adapter = primed_adapter(config(), "T0123ABCD", "B0123BOT").await;
        let env = envelope(
            "T0123ABCD",
            "C0123DEFG",
            Some("U0123USER"),
            "1714123456.000100",
            None,
            None,
            None,
            Some("channel"),
        );
        assert!(adapter.build_inbound_frame(env).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_dedups_within_window() {
        let adapter = primed_adapter(config(), "T0123ABCD", "B0123BOT").await;
        let env1 = envelope(
            "T0123ABCD",
            "D0123ABCD",
            Some("U0123USER"),
            "1714123456.000100",
            None,
            None,
            None,
            Some("im"),
        );
        let env2 = envelope(
            "T0123ABCD",
            "D0123ABCD",
            Some("U0123USER"),
            "1714123456.000100", // SAME ts, SAME channel
            None,
            None,
            None,
            Some("im"),
        );
        assert!(adapter.build_inbound_frame(env1).await.is_some());
        assert!(adapter.build_inbound_frame(env2).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_drops_channel_not_in_allowlist() {
        let mut cfg = config();
        cfg.dm_only = false;
        cfg.channels = vec!["C_ALLOWED".into()];
        let adapter = primed_adapter(cfg, "T0123ABCD", "B0123BOT").await;
        let env = envelope(
            "T0123ABCD",
            "C_OTHER",
            Some("U0123USER"),
            "1714123456.000100",
            None,
            None,
            None,
            Some("channel"),
        );
        assert!(adapter.build_inbound_frame(env).await.is_none());
    }

    #[tokio::test]
    async fn build_inbound_frame_includes_thread_when_supported() {
        let mut cfg = config();
        cfg.support_thread = true;
        cfg.dm_only = false;
        cfg.channels = vec!["C0123DEFG".into()];
        let adapter = primed_adapter(cfg, "T0123ABCD", "B0123BOT").await;
        let env = envelope(
            "T0123ABCD",
            "C0123DEFG",
            Some("U0123USER"),
            "1714123456.000200",
            Some("1714123456.000100"),
            None,
            None,
            Some("channel"),
        );
        let frame = adapter.build_inbound_frame(env).await.unwrap();
        assert_eq!(frame.thread_id.as_deref(), Some("1714123456.000100"));
        assert_eq!(frame.thread_kind, Some(ThreadKind::SlackThread));
    }
}
