//! Telegram transport adapter.
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS Q11:
//!   - credential verification: `getMe`
//!   - inbound: long polling via `getUpdates` (`polling_timeout_seconds`)
//!   - outbound: `sendMessage` with optional `reply_to_message_id`
//!     coercion (parse string → i64; on parse failure drop the
//!     reply_to and log WARN — message still sends).
//!
//! The adapter holds a resolved bot token plus per-transport routing
//! config.  All HTTP calls go through `reqwest`; we use the Telegram
//! Bot API HTTP surface directly rather than the higher-level
//! `teloxide` framework to keep the adapter narrow and testable.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::transport::config::TelegramConfig;
use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame, ThreadKind};
use crate::transport::gateway::{Gateway, InboundSink};
use crate::transport::{Transport, TransportContext, VerifiedIdentity};
use crate::{MakakooError, Result};

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Telegram adapter.
pub struct TelegramAdapter {
    pub ctx: TransportContext,
    pub config: TelegramConfig,
    /// Resolved bot token from the secrets layer.
    pub bot_token: String,
    /// Optional override of the API base URL (tests use wiremock).
    pub api_base: String,
    /// Last-seen update offset for `getUpdates` long polling.
    pub offset: Mutex<i64>,
    pub http: reqwest::Client,
}

impl TelegramAdapter {
    pub fn new(ctx: TransportContext, config: TelegramConfig, bot_token: String) -> Self {
        Self::with_api_base(ctx, config, bot_token, TELEGRAM_API_BASE.into())
    }

    pub fn with_api_base(
        ctx: TransportContext,
        config: TelegramConfig,
        bot_token: String,
        api_base: String,
    ) -> Self {
        Self {
            ctx,
            config,
            bot_token,
            api_base,
            offset: Mutex::new(0),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
        }
    }

    fn url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.api_base, self.bot_token, method)
    }

    /// Coerce the outbound frame's `reply_to_message_id` into the
    /// Telegram-native i64 format.  Returns `None` when the frame
    /// has no reply target OR the string fails to parse — in the
    /// latter case logs a structured WARN per the Q11 contract.
    fn coerce_reply_to(&self, frame: &MakakooOutboundFrame) -> Option<i64> {
        let raw = frame.reply_to_message_id.as_deref()?;
        match raw.parse::<i64>() {
            Ok(v) => Some(v),
            Err(_) => {
                tracing::warn!(
                    target: "makakoo_core::transport::telegram",
                    transport_id = self.ctx.transport_id,
                    reply_to_message_id = raw,
                    "non-integer reply_to_message_id — dropping thread anchor, sending without reply target"
                );
                None
            }
        }
    }
}

// ── Telegram API DTOs (only the fields we use) ─────────────────────

#[derive(Debug, Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramUser {
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramUpdate {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramMessage {
    pub message_id: i64,
    pub date: i64,
    pub chat: TelegramChat,
    pub from: Option<TelegramUser>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub message_thread_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    #[serde(default)]
    pub kind: String,
}

// ── Outbound API request payload ──────────────────────────────────

#[derive(Debug, Serialize)]
struct SendMessageReq<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
}

// ── Transport impl ────────────────────────────────────────────────

#[async_trait]
impl Transport for TelegramAdapter {
    fn kind(&self) -> &'static str {
        "telegram"
    }

    fn transport_id(&self) -> &str {
        &self.ctx.transport_id
    }

    async fn verify_credentials(&self) -> Result<VerifiedIdentity> {
        let resp: TelegramApiResponse<TelegramUser> = self
            .http
            .get(self.url("getMe"))
            .send()
            .await?
            .json()
            .await?;
        if !resp.ok {
            return Err(MakakooError::Config(format!(
                "telegram getMe failed: {}",
                resp.description.unwrap_or_else(|| "unknown".into())
            )));
        }
        let user = resp
            .result
            .ok_or_else(|| MakakooError::internal("getMe returned ok=true but no result"))?;
        Ok(VerifiedIdentity {
            account_id: user.id.to_string(),
            tenant_id: None,
            display_name: user.username.or(user.first_name),
        })
    }

    async fn send(&self, frame: &MakakooOutboundFrame) -> Result<()> {
        let chat_id = frame.conversation_id.parse::<i64>().map_err(|_| {
            MakakooError::InvalidInput(format!(
                "telegram outbound conversation_id '{}' is not a numeric chat_id",
                frame.conversation_id
            ))
        })?;
        let reply_to_message_id = self.coerce_reply_to(frame);
        let message_thread_id = if self.config.support_thread {
            match (&frame.thread_id, &frame.thread_kind) {
                (Some(s), Some(ThreadKind::TelegramForum)) => s.parse::<i64>().ok(),
                (Some(_), Some(ThreadKind::SlackThread)) => {
                    tracing::warn!(
                        target: "makakoo_core::transport::telegram",
                        transport_id = self.ctx.transport_id,
                        "outbound thread_kind=slack_thread cannot ride a telegram transport — dropping thread anchor"
                    );
                    None
                }
                _ => None,
            }
        } else {
            None
        };
        let body = SendMessageReq {
            chat_id,
            text: &frame.text,
            reply_to_message_id,
            message_thread_id,
        };
        let resp: TelegramApiResponse<serde_json::Value> = self
            .http
            .post(self.url("sendMessage"))
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if !resp.ok {
            return Err(MakakooError::Internal(format!(
                "telegram sendMessage failed: {}",
                resp.description.unwrap_or_else(|| "unknown".into())
            )));
        }
        Ok(())
    }
}

// ── Gateway (long-poll loop) ──────────────────────────────────────

#[async_trait]
impl Gateway for TelegramAdapter {
    async fn start(&self, sink: InboundSink) -> Result<()> {
        loop {
            let timeout = self.config.polling_timeout_seconds.max(1);
            let offset = { *self.offset.lock().await };
            let url = format!(
                "{}/bot{}/getUpdates?timeout={}&offset={}",
                self.api_base, self.bot_token, timeout, offset
            );
            let resp = match self
                .http
                .get(&url)
                .timeout(Duration::from_secs(timeout + 5))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        target: "makakoo_core::transport::telegram",
                        transport_id = self.ctx.transport_id,
                        error = %e,
                        "telegram getUpdates transport error — backing off 2s"
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
            let parsed: TelegramApiResponse<Vec<TelegramUpdate>> = match resp.json().await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        target: "makakoo_core::transport::telegram",
                        transport_id = self.ctx.transport_id,
                        error = %e,
                        "telegram getUpdates JSON parse failure — backing off 2s"
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
            if !parsed.ok {
                tracing::warn!(
                    target: "makakoo_core::transport::telegram",
                    transport_id = self.ctx.transport_id,
                    description = ?parsed.description,
                    "telegram getUpdates returned ok=false — backing off 5s"
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
            let updates = parsed.result.unwrap_or_default();
            for update in updates {
                {
                    let mut o = self.offset.lock().await;
                    if update.update_id + 1 > *o {
                        *o = update.update_id + 1;
                    }
                }
                let Some(msg) = update.message else { continue };
                let Some(text) = msg.text.clone() else { continue };
                let frame = self.build_inbound_frame(msg, text);
                if sink.send(frame).await.is_err() {
                    tracing::error!(
                        target: "makakoo_core::transport::telegram",
                        transport_id = self.ctx.transport_id,
                        "inbound sink closed — telegram listener exiting"
                    );
                    return Ok(());
                }
            }
        }
    }
}

impl TelegramAdapter {
    /// Build an inbound frame from a Telegram update.  Pure (no I/O)
    /// so unit tests can exercise the field mapping with a fixture.
    pub(crate) fn build_inbound_frame(
        &self,
        msg: TelegramMessage,
        text: String,
    ) -> MakakooInboundFrame {
        let conversation_id = msg.chat.id.to_string();
        let sender_id = msg
            .from
            .as_ref()
            .map(|u| u.id.to_string())
            .unwrap_or_else(|| conversation_id.clone());
        let (thread_id, thread_kind) = match msg.message_thread_id {
            Some(t) if self.config.support_thread => {
                (Some(t.to_string()), Some(ThreadKind::TelegramForum))
            }
            _ => (None, None),
        };
        MakakooInboundFrame {
            agent_slot_id: self.ctx.slot_id.clone(),
            transport_id: self.ctx.transport_id.clone(),
            transport_kind: "telegram".into(),
            account_id: String::new(), // filled in by verify_credentials at startup
            conversation_id,
            sender_id,
            thread_id,
            thread_kind,
            message_id: msg.message_id.to_string(),
            text,
            transport_timestamp: Some(msg.date.to_string()),
            received_at: chrono::Utc::now(),
            raw_metadata: Default::default(),
        }
    }
}

/// Wrap an adapter in `Arc<dyn Transport>` for router registration.
pub fn boxed(adapter: TelegramAdapter) -> Arc<dyn Transport> {
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
            transport_id: "telegram-main".into(),
        }
    }

    fn config() -> TelegramConfig {
        TelegramConfig {
            polling_timeout_seconds: 30,
            allowed_chat_ids: vec!["746496145".into()],
            allowed_group_ids: vec![],
            support_thread: false,
        }
    }

    #[tokio::test]
    async fn verify_credentials_returns_bot_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bot123:abc/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "id": 8675309, "username": "SecretaryBot", "first_name": "Sec" }
            })))
            .mount(&server)
            .await;
        let adapter = TelegramAdapter::with_api_base(
            ctx(),
            config(),
            "123:abc".into(),
            server.uri(),
        );
        let id = adapter.verify_credentials().await.unwrap();
        assert_eq!(id.account_id, "8675309");
        assert!(id.tenant_id.is_none());
        assert_eq!(id.display_name.as_deref(), Some("SecretaryBot"));
    }

    #[tokio::test]
    async fn verify_credentials_rejects_invalid_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botbad/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "description": "Unauthorized"
            })))
            .mount(&server)
            .await;
        let adapter =
            TelegramAdapter::with_api_base(ctx(), config(), "bad".into(), server.uri());
        let err = adapter.verify_credentials().await.unwrap_err();
        assert!(format!("{err}").contains("Unauthorized"));
    }

    #[tokio::test]
    async fn send_invalid_chat_id_rejected_before_http() {
        let adapter = TelegramAdapter::with_api_base(
            ctx(),
            config(),
            "123:abc".into(),
            "http://unused.invalid".into(),
        );
        let frame = MakakooOutboundFrame {
            transport_id: "telegram-main".into(),
            transport_kind: "telegram".into(),
            conversation_id: "not-a-number".into(),
            thread_id: None,
            thread_kind: None,
            text: "hi".into(),
            reply_to_message_id: None,
        };
        let err = adapter.send(&frame).await.unwrap_err();
        assert!(format!("{err}").contains("not a numeric chat_id"));
    }

    #[tokio::test]
    async fn send_drops_invalid_reply_to_but_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/bot123:abc/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {}
            })))
            .mount(&server)
            .await;
        let adapter = TelegramAdapter::with_api_base(
            ctx(),
            config(),
            "123:abc".into(),
            server.uri(),
        );
        let frame = MakakooOutboundFrame {
            transport_id: "telegram-main".into(),
            transport_kind: "telegram".into(),
            conversation_id: "746496145".into(),
            thread_id: None,
            thread_kind: None,
            text: "hi".into(),
            // Slack-shaped thread_ts string sent on a Telegram outbound
            reply_to_message_id: Some("1714123456.000200".into()),
        };
        adapter.send(&frame).await.unwrap();
    }

    #[test]
    fn build_inbound_frame_maps_fields() {
        let adapter = TelegramAdapter::with_api_base(
            ctx(),
            config(),
            "123:abc".into(),
            "http://unused.invalid".into(),
        );
        let msg = TelegramMessage {
            message_id: 42,
            date: 1714123456,
            chat: TelegramChat {
                id: 746496145,
                kind: "private".into(),
            },
            from: Some(TelegramUser {
                id: 746496145,
                username: Some("schkudlara".into()),
                first_name: None,
            }),
            text: Some("hi".into()),
            message_thread_id: None,
        };
        let frame = adapter.build_inbound_frame(msg, "hi".into());
        assert_eq!(frame.transport_id, "telegram-main");
        assert_eq!(frame.transport_kind, "telegram");
        assert_eq!(frame.conversation_id, "746496145");
        assert_eq!(frame.sender_id, "746496145");
        assert_eq!(frame.message_id, "42");
        assert_eq!(frame.text, "hi");
        assert_eq!(frame.transport_timestamp.as_deref(), Some("1714123456"));
    }

    #[test]
    fn build_inbound_frame_drops_thread_when_unsupported() {
        let mut cfg = config();
        cfg.support_thread = false;
        let adapter = TelegramAdapter::with_api_base(
            ctx(),
            cfg,
            "123:abc".into(),
            "http://unused.invalid".into(),
        );
        let msg = TelegramMessage {
            message_id: 1,
            date: 0,
            chat: TelegramChat { id: 1, kind: "supergroup".into() },
            from: None,
            text: Some("x".into()),
            message_thread_id: Some(99),
        };
        let frame = adapter.build_inbound_frame(msg, "x".into());
        assert!(frame.thread_id.is_none());
        assert!(frame.thread_kind.is_none());
    }

    #[test]
    fn build_inbound_frame_includes_thread_when_supported() {
        let mut cfg = config();
        cfg.support_thread = true;
        let adapter = TelegramAdapter::with_api_base(
            ctx(),
            cfg,
            "123:abc".into(),
            "http://unused.invalid".into(),
        );
        let msg = TelegramMessage {
            message_id: 1,
            date: 0,
            chat: TelegramChat { id: 1, kind: "supergroup".into() },
            from: None,
            text: Some("x".into()),
            message_thread_id: Some(99),
        };
        let frame = adapter.build_inbound_frame(msg, "x".into());
        assert_eq!(frame.thread_id.as_deref(), Some("99"));
        assert_eq!(frame.thread_kind, Some(ThreadKind::TelegramForum));
    }
}
