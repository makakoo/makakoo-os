//! Slack transport adapter (Socket Mode).
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS Q11:
//!   - bot-token verification: `auth.test` HTTP call
//!   - app-token Socket Mode probe: `apps.connections.open` HTTP
//!     call (returns a `wss://` URL — actually opening that URL to
//!     receive Events API envelopes lives in Phase 2 alongside the
//!     `tokio-tungstenite` dep introduction; v1 ships the credential
//!     verifier + outbound + inbound frame mapping).
//!   - outbound: `chat.postMessage` with `thread_ts` and
//!     reply-target coercion.
//!
//! Inbound frame construction is exercised by unit tests against
//! sample Events API envelopes (`message.im`, `message.channel`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::transport::config::SlackConfig;
use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame, ThreadKind};
use crate::transport::gateway::{Gateway, InboundSink};
use crate::transport::{Transport, TransportContext, VerifiedIdentity};
use crate::{MakakooError, Result};

const SLACK_API_BASE: &str = "https://slack.com/api";

/// Slack adapter (Socket Mode).
pub struct SlackAdapter {
    pub ctx: TransportContext,
    pub config: SlackConfig,
    pub bot_token: String,
    pub app_token: String,
    pub api_base: String,
    pub http: reqwest::Client,
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
        }
    }
}

// ── Slack API DTOs (only the fields we use) ────────────────────────

#[derive(Debug, Deserialize)]
struct SlackResponse {
    ok: bool,
    #[serde(default)]
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

#[derive(Debug, Serialize)]
struct ChatPostMessage<'a> {
    channel: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<&'a str>,
}

/// One inbound Events API envelope as delivered over Socket Mode.
/// Phase 1 deserializes it to validate the field mapping; Phase 2
/// adds the WebSocket loop that pulls envelopes from `wss://…` and
/// drains them into `build_inbound_frame`.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct SlackEnvelope {
    #[serde(default)]
    pub envelope_id: Option<String>,
    pub payload: SlackEventPayload,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct SlackEventPayload {
    pub team_id: String,
    pub event: SlackEvent,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SlackEvent {
    Message(SlackMessageEvent),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct SlackMessageEvent {
    pub channel: String,
    pub user: String,
    pub text: String,
    pub ts: String,
    #[serde(default)]
    pub thread_ts: Option<String>,
    #[serde(default)]
    pub channel_type: Option<String>,
    #[serde(default)]
    pub bot_id: Option<String>,
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
        // The `url` field on success is the wss:// endpoint Phase 2
        // will dial; we don't connect here.

        let account_id = auth
            .bot_id
            .or(auth.user_id)
            .unwrap_or_else(|| "unknown".into());
        Ok(VerifiedIdentity {
            account_id,
            tenant_id: Some(auth.team_id),
            display_name: auth.team,
        })
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

        // Q11 reply_to_message_id coercion for Slack: pass the
        // string through if it looks like a Slack thread_ts (matches
        // /^\d+\.\d+$/). Otherwise drop with WARN. We use this only
        // when explicit thread_ts isn't already set (Slack treats
        // thread_ts as the reply anchor).
        let reply_thread_ts = if thread_ts.is_none() {
            frame.reply_to_message_id.as_deref().and_then(|raw| {
                let looks_like_ts = raw
                    .split_once('.')
                    .map(|(a, b)| {
                        !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit())
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
    async fn start(&self, _sink: InboundSink) -> Result<()> {
        // Phase 1 ships the credential verifier + outbound + inbound
        // frame mapping; the WebSocket loop body lands in Phase 2
        // alongside the tokio-tungstenite dep introduction.  For
        // Phase 1 we no-op the listener so adapters compile and
        // can be wired into the router for unit tests.
        tracing::warn!(
            target: "makakoo_core::transport::slack",
            transport_id = self.ctx.transport_id,
            "slack Socket Mode WebSocket loop is a Phase 2 deliverable; Phase 1 ships verifier + outbound + envelope parser only"
        );
        Ok(())
    }
}

impl SlackAdapter {
    /// Build an inbound frame from a Socket Mode Events API envelope.
    /// Pure (no I/O) so unit tests can exercise the field mapping
    /// without a live WebSocket.  Returns `None` for envelopes we
    /// don't translate in v1 (non-`message` events, bot echoes).
    pub(crate) fn build_inbound_frame(
        &self,
        envelope: SlackEnvelope,
    ) -> Option<MakakooInboundFrame> {
        let team_id = envelope.payload.team_id.clone();
        let SlackEvent::Message(msg) = envelope.payload.event else {
            return None;
        };
        // Suppress bot echoes — we don't want the agent replying to
        // its own messages.
        if msg.bot_id.is_some() {
            return None;
        }
        let (thread_id, thread_kind) = match &msg.thread_ts {
            Some(ts) if self.config.support_thread => {
                (Some(ts.clone()), Some(ThreadKind::SlackThread))
            }
            _ => (None, None),
        };
        Some(MakakooInboundFrame {
            agent_slot_id: self.ctx.slot_id.clone(),
            transport_id: self.ctx.transport_id.clone(),
            transport_kind: "slack".into(),
            account_id: format!("{}:{}", team_id, msg.channel_type.clone().unwrap_or_default()),
            conversation_id: msg.channel.clone(),
            sender_id: msg.user.clone(),
            thread_id,
            thread_kind,
            message_id: msg.ts.clone(),
            text: msg.text.clone(),
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

    #[tokio::test]
    async fn verify_credentials_succeeds_with_matching_team() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "team_id": "T0123ABCD",
                "bot_id": "B0123BOT",
                "team": "Acme"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apps.connections.open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "url": "wss://wss.slack.com/socket/123"
            })))
            .mount(&server)
            .await;
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            config(),
            "xoxb-bot".into(),
            "xapp-app".into(),
            server.uri(),
        );
        let id = adapter.verify_credentials().await.unwrap();
        assert_eq!(id.account_id, "B0123BOT");
        assert_eq!(id.tenant_id.as_deref(), Some("T0123ABCD"));
    }

    #[tokio::test]
    async fn verify_credentials_rejects_team_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "team_id": "T9999OTHER",
                "bot_id": "B0123BOT"
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
                "ok": false,
                "error": "invalid_auth"
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
            // Telegram-shaped reply_to (numeric int, not float-string)
            reply_to_message_id: Some("42".into()),
        };
        adapter.send(&frame).await.unwrap();
    }

    #[test]
    fn build_inbound_frame_dm() {
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            config(),
            "xoxb".into(),
            "xapp".into(),
            "http://unused.invalid".into(),
        );
        let env = SlackEnvelope {
            envelope_id: Some("env-1".into()),
            payload: SlackEventPayload {
                team_id: "T0123ABCD".into(),
                event: SlackEvent::Message(SlackMessageEvent {
                    channel: "D0123ABCD".into(),
                    user: "U0123USER".into(),
                    text: "hi".into(),
                    ts: "1714123456.000100".into(),
                    thread_ts: None,
                    channel_type: Some("im".into()),
                    bot_id: None,
                }),
            },
        };
        let frame = adapter.build_inbound_frame(env).unwrap();
        assert_eq!(frame.transport_kind, "slack");
        assert_eq!(frame.conversation_id, "D0123ABCD");
        assert_eq!(frame.sender_id, "U0123USER");
        assert_eq!(frame.message_id, "1714123456.000100");
    }

    #[test]
    fn build_inbound_frame_channel() {
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            SlackConfig {
                dm_only: false,
                channels: vec!["C0123DEFG".into()],
                ..config()
            },
            "xoxb".into(),
            "xapp".into(),
            "http://unused.invalid".into(),
        );
        let env = SlackEnvelope {
            envelope_id: None,
            payload: SlackEventPayload {
                team_id: "T0123ABCD".into(),
                event: SlackEvent::Message(SlackMessageEvent {
                    channel: "C0123DEFG".into(),
                    user: "U0123USER".into(),
                    text: "hi".into(),
                    ts: "1714123456.000100".into(),
                    thread_ts: None,
                    channel_type: Some("channel".into()),
                    bot_id: None,
                }),
            },
        };
        let frame = adapter.build_inbound_frame(env).unwrap();
        assert_eq!(frame.conversation_id, "C0123DEFG");
    }

    #[test]
    fn build_inbound_frame_drops_bot_echoes() {
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            config(),
            "xoxb".into(),
            "xapp".into(),
            "http://unused.invalid".into(),
        );
        let env = SlackEnvelope {
            envelope_id: None,
            payload: SlackEventPayload {
                team_id: "T0123ABCD".into(),
                event: SlackEvent::Message(SlackMessageEvent {
                    channel: "D0123ABCD".into(),
                    user: "U0123BOTUSER".into(),
                    text: "hi".into(),
                    ts: "1714123456.000100".into(),
                    thread_ts: None,
                    channel_type: Some("im".into()),
                    bot_id: Some("B0123BOT".into()),
                }),
            },
        };
        assert!(adapter.build_inbound_frame(env).is_none());
    }

    #[test]
    fn build_inbound_frame_includes_thread_when_supported() {
        let adapter = SlackAdapter::with_api_base(
            ctx(),
            SlackConfig {
                support_thread: true,
                ..config()
            },
            "xoxb".into(),
            "xapp".into(),
            "http://unused.invalid".into(),
        );
        let env = SlackEnvelope {
            envelope_id: None,
            payload: SlackEventPayload {
                team_id: "T0123ABCD".into(),
                event: SlackEvent::Message(SlackMessageEvent {
                    channel: "C0123DEFG".into(),
                    user: "U0123USER".into(),
                    text: "in thread".into(),
                    ts: "1714123456.000200".into(),
                    thread_ts: Some("1714123456.000100".into()),
                    channel_type: Some("channel".into()),
                    bot_id: None,
                }),
            },
        };
        let frame = adapter.build_inbound_frame(env).unwrap();
        assert_eq!(frame.thread_id.as_deref(), Some("1714123456.000100"));
        assert_eq!(frame.thread_kind, Some(ThreadKind::SlackThread));
    }
}
