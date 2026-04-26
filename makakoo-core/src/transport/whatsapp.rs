//! WhatsApp Cloud API transport adapter.
//!
//! Phase 8 / Q7. WhatsApp uses Meta's Cloud API webhook delivery —
//! NO per-task gateway listener. The receive path is the webhook
//! handler in [`makakoo_mcp::whatsapp_webhook`]. This file ships:
//!
//! - [`WhatsAppAdapter`] implementing [`Transport`] for outbound +
//!   credential verification.
//! - Locked frame-mapping helpers used by the webhook handler.
//! - Locked outbound DTOs.
//!
//! Behavior locked by Q7:
//!   - bot-token = Cloud API access token (Bearer)
//!   - `verify_token_ref` for the GET `hub.challenge` handshake
//!   - `app_secret_ref` for POST X-Hub-Signature-256 HMAC
//!   - Inbound media → polite drop reply (no transcription in v1)
//!   - account_id = phone_number_id (Meta-issued)

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::transport::config::WhatsAppConfig;
use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame};
use crate::transport::gateway::{Gateway, InboundSink};
use crate::transport::{Transport, TransportContext, VerifiedIdentity};
use crate::{MakakooError, Result};

const DEFAULT_GRAPH_API: &str = "https://graph.facebook.com";

/// Locked drop-reply text the adapter sends back when an inbound
/// non-text message arrives. Surfaces the limitation politely without
/// triggering a "blocked the bot" reaction in the UI.
pub const MEDIA_DROP_REPLY: &str =
    "Thanks — I can only read text messages right now. Please re-send as text.";

pub struct WhatsAppAdapter {
    pub ctx: TransportContext,
    pub config: WhatsAppConfig,
    /// Resolved Cloud API access token (Bearer).
    pub access_token: String,
    /// Override of the Graph API base URL (tests use wiremock).
    pub api_base: String,
    pub http: reqwest::Client,
    /// Per-transport allowlist (Q7 simplified): inbound from `from`
    /// not in this list is dropped at the frame-mapping step. Empty
    /// = least-privilege deny-all.
    pub allowed_wa_ids: Vec<String>,
}

impl WhatsAppAdapter {
    pub fn new(
        ctx: TransportContext,
        config: WhatsAppConfig,
        access_token: String,
        allowed_wa_ids: Vec<String>,
    ) -> Self {
        Self::with_api_base(
            ctx,
            config,
            access_token,
            allowed_wa_ids,
            DEFAULT_GRAPH_API.into(),
        )
    }

    pub fn with_api_base(
        ctx: TransportContext,
        config: WhatsAppConfig,
        access_token: String,
        allowed_wa_ids: Vec<String>,
        api_base: String,
    ) -> Self {
        Self {
            ctx,
            config,
            access_token,
            api_base,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            allowed_wa_ids,
        }
    }

    fn graph_url(&self, path: &str) -> String {
        format!(
            "{}/{}{}",
            self.api_base, self.config.graph_version, path
        )
    }
}

#[derive(Serialize)]
struct WaSendText<'a> {
    messaging_product: &'a str,
    to: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    text: WaTextBody<'a>,
}

#[derive(Serialize)]
struct WaTextBody<'a> {
    body: &'a str,
}

#[derive(Deserialize)]
struct WaPhoneNumberInfo {
    id: String,
    #[serde(default)]
    display_phone_number: Option<String>,
    #[serde(default)]
    verified_name: Option<String>,
}

#[async_trait]
impl Transport for WhatsAppAdapter {
    fn kind(&self) -> &'static str {
        "whatsapp"
    }
    fn transport_id(&self) -> &str {
        &self.ctx.transport_id
    }

    async fn verify_credentials(&self) -> Result<VerifiedIdentity> {
        // GET /v18.0/{phone_number_id} with Bearer token. Returns the
        // phone number's metadata. Confirms both that the token is
        // valid and that it has access to the configured number.
        let url = self.graph_url(&format!("/{}", self.config.phone_number_id));
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MakakooError::Config(format!(
                "whatsapp graph GET {url} failed: HTTP {status}: {body}"
            )));
        }
        let info: WaPhoneNumberInfo = resp.json().await?;
        if info.id != self.config.phone_number_id {
            return Err(MakakooError::Config(format!(
                "whatsapp phone_number_id mismatch: TOML='{}' but Graph returned '{}'",
                self.config.phone_number_id, info.id
            )));
        }
        Ok(VerifiedIdentity {
            account_id: info.id,
            tenant_id: None,
            display_name: info.verified_name.or(info.display_phone_number),
        })
    }

    async fn send(&self, frame: &MakakooOutboundFrame) -> Result<()> {
        let body = WaSendText {
            messaging_product: "whatsapp",
            to: &frame.conversation_id,
            kind: "text",
            text: WaTextBody { body: &frame.text },
        };
        let url = self.graph_url(&format!("/{}/messages", self.config.phone_number_id));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MakakooError::Internal(format!(
                "whatsapp send to {} failed: HTTP {status}: {body}",
                frame.conversation_id
            )));
        }
        Ok(())
    }
}

/// WhatsApp delivery is webhook-based — no per-task listener needed.
/// We implement Gateway as a no-op so the adapter slots into the
/// existing Transport+Gateway pair lifecycle without special-casing.
#[async_trait]
impl Gateway for WhatsAppAdapter {
    async fn start(&self, _sink: InboundSink) -> Result<()> {
        // Park forever. The webhook router delivers inbound frames
        // through a separate code path (whatsapp_webhook); the per-
        // adapter Gateway exists only to satisfy the trait shape.
        std::future::pending::<()>().await;
        Ok(())
    }
}

// ── Inbound webhook payload DTOs (subset we use) ───────────────────

#[derive(Deserialize, Debug)]
pub struct WaWebhookEnvelope {
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub entry: Vec<WaEntry>,
}

#[derive(Deserialize, Debug)]
pub struct WaEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub changes: Vec<WaChange>,
}

#[derive(Deserialize, Debug)]
pub struct WaChange {
    #[serde(default)]
    pub field: String,
    pub value: WaChangeValue,
}

#[derive(Deserialize, Debug, Default)]
pub struct WaChangeValue {
    #[serde(default)]
    pub messaging_product: String,
    #[serde(default)]
    pub messages: Vec<WaInboundMessage>,
    #[serde(default)]
    pub statuses: Vec<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<WaMetadata>,
}

#[derive(Deserialize, Debug, Default)]
pub struct WaMetadata {
    #[serde(default)]
    pub display_phone_number: Option<String>,
    #[serde(default)]
    pub phone_number_id: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct WaInboundMessage {
    pub id: String,
    pub from: String,
    /// Unix epoch (string).
    pub timestamp: String,
    /// `"text" | "image" | "audio" | "video" | "document" | ...`
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<WaTextField>,
}

#[derive(Deserialize, Debug)]
pub struct WaTextField {
    pub body: String,
}

/// Outcome of mapping one inbound webhook envelope.
pub enum WaInboundOutcome {
    /// One or more text frames produced.
    Frames(Vec<MakakooInboundFrame>),
    /// Status callback (delivery / read receipt) — silently
    /// swallowed.
    Status,
    /// Media-only message — caller should send `MEDIA_DROP_REPLY` to
    /// the listed senders.
    MediaDrop { wa_ids: Vec<String> },
    /// Empty / unknown payload shape — drop silently.
    Empty,
}

impl WhatsAppAdapter {
    /// Map a verified webhook envelope into Makakoo frames or
    /// status/media-drop outcomes. Pure — no I/O. The caller (the
    /// webhook handler) decides whether to send a media-drop reply.
    pub fn map_inbound(&self, env: WaWebhookEnvelope) -> WaInboundOutcome {
        if env.object != "whatsapp_business_account" {
            return WaInboundOutcome::Empty;
        }

        let mut frames: Vec<MakakooInboundFrame> = Vec::new();
        let mut media_senders: Vec<String> = Vec::new();
        let mut had_status = false;

        for entry in env.entry {
            for change in entry.changes {
                let value = change.value;
                if !value.statuses.is_empty() && value.messages.is_empty() {
                    had_status = true;
                    continue;
                }
                for msg in value.messages {
                    // Allowlist enforcement — drop silently.
                    if !self.allowed_wa_ids.contains(&msg.from) {
                        tracing::debug!(
                            target: "makakoo_core::transport::whatsapp",
                            transport_id = self.ctx.transport_id,
                            from = msg.from,
                            "dropping non-allowlisted wa_id"
                        );
                        continue;
                    }
                    if msg.kind != "text" {
                        media_senders.push(msg.from.clone());
                        continue;
                    }
                    let body = msg.text.map(|t| t.body).unwrap_or_default();
                    let mut raw = BTreeMap::new();
                    if let Some(meta) = value.metadata.as_ref() {
                        if let Some(p) = &meta.phone_number_id {
                            raw.insert(
                                "phone_number_id".into(),
                                serde_json::Value::String(p.clone()),
                            );
                        }
                    }
                    raw.insert(
                        "wa_message_kind".into(),
                        serde_json::Value::String(msg.kind.clone()),
                    );
                    let frame = MakakooInboundFrame {
                        agent_slot_id: self.ctx.slot_id.clone(),
                        transport_id: self.ctx.transport_id.clone(),
                        transport_kind: "whatsapp".into(),
                        account_id: self.config.phone_number_id.clone(),
                        conversation_id: msg.from.clone(),
                        sender_id: msg.from,
                        thread_id: None,
                        thread_kind: None,
                        message_id: msg.id,
                        text: body,
                        transport_timestamp: Some(msg.timestamp),
                        received_at: chrono::Utc::now(),
                        raw_metadata: raw,
                    };
                    frames.push(frame);
                }
            }
        }

        if !frames.is_empty() {
            return WaInboundOutcome::Frames(frames);
        }
        if !media_senders.is_empty() {
            return WaInboundOutcome::MediaDrop {
                wa_ids: media_senders,
            };
        }
        if had_status {
            return WaInboundOutcome::Status;
        }
        WaInboundOutcome::Empty
    }
}

/// Convenience constructor.
pub fn boxed(adapter: WhatsAppAdapter) -> Arc<dyn Transport> {
    Arc::new(adapter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportContext;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "wa-main".into(),
        }
    }

    fn cfg() -> WhatsAppConfig {
        WhatsAppConfig {
            phone_number_id: "12345".into(),
            graph_version: "v18.0".into(),
            verify_token_env: None,
            verify_token_ref: None,
            inline_verify_token_dev: Some("HUBVERIFY".into()),
            app_secret_env: None,
            app_secret_ref: None,
            inline_app_secret_dev: Some("APPSECRET".into()),
            allowed_wa_ids: vec!["34000000001".into()],
        }
    }

    fn adapter(api_base: String, allowed: Vec<String>) -> WhatsAppAdapter {
        WhatsAppAdapter::with_api_base(ctx(), cfg(), "ACCESS".into(), allowed, api_base)
    }

    // ── verify_credentials ───────────────────────────────────

    #[tokio::test]
    async fn verify_credentials_returns_phone_number_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v18.0/12345"))
            .and(header("authorization", "Bearer ACCESS"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "12345",
                "display_phone_number": "+34 600 000 001",
                "verified_name": "Makakoo Secretary"
            })))
            .mount(&server)
            .await;
        let a = adapter(server.uri(), vec!["34000000001".into()]);
        let id = a.verify_credentials().await.unwrap();
        assert_eq!(id.account_id, "12345");
        assert_eq!(id.display_name.as_deref(), Some("Makakoo Secretary"));
    }

    #[tokio::test]
    async fn verify_credentials_rejects_token_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v18.0/12345"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Bad token"))
            .mount(&server)
            .await;
        let a = adapter(server.uri(), vec![]);
        let err = a.verify_credentials().await.unwrap_err();
        assert!(format!("{err}").contains("HTTP 401"));
    }

    #[tokio::test]
    async fn verify_credentials_rejects_phone_number_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v18.0/12345"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "99999",
                "display_phone_number": "+1 555 000 0000"
            })))
            .mount(&server)
            .await;
        let a = adapter(server.uri(), vec![]);
        let err = a.verify_credentials().await.unwrap_err();
        assert!(format!("{err}").contains("phone_number_id mismatch"));
    }

    // ── send ─────────────────────────────────────────────────

    #[tokio::test]
    async fn send_posts_to_messages_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v18.0/12345/messages"))
            .and(header("authorization", "Bearer ACCESS"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "messaging_product": "whatsapp",
                "messages": [{ "id": "wamid.x" }]
            })))
            .mount(&server)
            .await;
        let a = adapter(server.uri(), vec![]);
        let frame = MakakooOutboundFrame {
            transport_id: "wa-main".into(),
            transport_kind: "whatsapp".into(),
            conversation_id: "34000000001".into(),
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
            .and(path("/v18.0/12345/messages"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Recipient not in allowlist"))
            .mount(&server)
            .await;
        let a = adapter(server.uri(), vec![]);
        let frame = MakakooOutboundFrame {
            transport_id: "wa-main".into(),
            transport_kind: "whatsapp".into(),
            conversation_id: "34000000999".into(),
            thread_id: None,
            thread_kind: None,
            text: "x".into(),
            reply_to_message_id: None,
        };
        let err = a.send(&frame).await.unwrap_err();
        assert!(format!("{err}").contains("HTTP 400"));
    }

    // ── map_inbound ──────────────────────────────────────────

    #[tokio::test]
    async fn map_inbound_text_message_produces_frame() {
        let env = WaWebhookEnvelope {
            object: "whatsapp_business_account".into(),
            entry: vec![WaEntry {
                id: "ENTRY".into(),
                changes: vec![WaChange {
                    field: "messages".into(),
                    value: WaChangeValue {
                        messaging_product: "whatsapp".into(),
                        messages: vec![WaInboundMessage {
                            id: "wamid.A".into(),
                            from: "34000000001".into(),
                            timestamp: "1700000000".into(),
                            kind: "text".into(),
                            text: Some(WaTextField {
                                body: "hello".into(),
                            }),
                        }],
                        statuses: vec![],
                        metadata: Some(WaMetadata {
                            phone_number_id: Some("12345".into()),
                            display_phone_number: None,
                        }),
                    },
                }],
            }],
        };
        let a = adapter("http://unused".into(), vec!["34000000001".into()]);
        match a.map_inbound(env) {
            WaInboundOutcome::Frames(fs) => {
                assert_eq!(fs.len(), 1);
                assert_eq!(fs[0].text, "hello");
                assert_eq!(fs[0].sender_id, "34000000001");
                assert_eq!(fs[0].account_id, "12345");
            }
            o => panic!("expected Frames, got {:?}", std::mem::discriminant(&o)),
        }
    }

    #[tokio::test]
    async fn map_inbound_image_triggers_media_drop() {
        let env = WaWebhookEnvelope {
            object: "whatsapp_business_account".into(),
            entry: vec![WaEntry {
                id: "E".into(),
                changes: vec![WaChange {
                    field: "messages".into(),
                    value: WaChangeValue {
                        messaging_product: "whatsapp".into(),
                        messages: vec![WaInboundMessage {
                            id: "wamid.B".into(),
                            from: "34000000001".into(),
                            timestamp: "1".into(),
                            kind: "image".into(),
                            text: None,
                        }],
                        statuses: vec![],
                        metadata: None,
                    },
                }],
            }],
        };
        let a = adapter("http://unused".into(), vec!["34000000001".into()]);
        match a.map_inbound(env) {
            WaInboundOutcome::MediaDrop { wa_ids } => {
                assert_eq!(wa_ids, vec!["34000000001".to_string()]);
            }
            _ => panic!("expected MediaDrop"),
        }
    }

    #[tokio::test]
    async fn map_inbound_status_only_returns_status_outcome() {
        let env = WaWebhookEnvelope {
            object: "whatsapp_business_account".into(),
            entry: vec![WaEntry {
                id: "E".into(),
                changes: vec![WaChange {
                    field: "messages".into(),
                    value: WaChangeValue {
                        messaging_product: "whatsapp".into(),
                        messages: vec![],
                        statuses: vec![serde_json::json!({"id":"x", "status":"delivered"})],
                        metadata: None,
                    },
                }],
            }],
        };
        let a = adapter("http://unused".into(), vec![]);
        assert!(matches!(a.map_inbound(env), WaInboundOutcome::Status));
    }

    #[tokio::test]
    async fn map_inbound_drops_non_allowlisted_sender() {
        let env = WaWebhookEnvelope {
            object: "whatsapp_business_account".into(),
            entry: vec![WaEntry {
                id: "E".into(),
                changes: vec![WaChange {
                    field: "messages".into(),
                    value: WaChangeValue {
                        messaging_product: "whatsapp".into(),
                        messages: vec![WaInboundMessage {
                            id: "wamid.C".into(),
                            from: "34000000999".into(),
                            timestamp: "1".into(),
                            kind: "text".into(),
                            text: Some(WaTextField { body: "x".into() }),
                        }],
                        statuses: vec![],
                        metadata: None,
                    },
                }],
            }],
        };
        let a = adapter("http://unused".into(), vec!["34000000001".into()]);
        assert!(matches!(a.map_inbound(env), WaInboundOutcome::Empty));
    }

    #[tokio::test]
    async fn map_inbound_object_mismatch_returns_empty() {
        let env = WaWebhookEnvelope {
            object: "page".into(),
            entry: vec![],
        };
        let a = adapter("http://unused".into(), vec![]);
        assert!(matches!(a.map_inbound(env), WaInboundOutcome::Empty));
    }
}
