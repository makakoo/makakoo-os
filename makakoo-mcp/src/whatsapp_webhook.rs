//! Phase 8 — WhatsApp Cloud API webhook adapter.
//!
//! HTTP webhook handler that fronts the [`WhatsAppAdapter`] inbound
//! path. Locked Q7:
//!
//! * GET (subscription handshake): `hub.mode=subscribe`,
//!   `hub.verify_token=<expected>`, `hub.challenge=<echo>`. Respond
//!   200 with the challenge body verbatim if the token matches; 401
//!   otherwise. No HMAC.
//! * POST (event delivery): `X-Hub-Signature-256: sha256=<hex>` over
//!   the raw body, keyed by app_secret. Verify BEFORE JSON parse.
//! * Webhook lands at `/transport/<slot_uuid>/<transport_uuid>/webhook`.
//! * On verified POST: parse the envelope, hand to
//!   `WhatsAppAdapter::map_inbound`, push frames to the inbound sink
//!   if any. The handler always replies 200 OK so Meta doesn't retry
//!   on internal hiccups.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Response as AxumResponse, StatusCode},
    response::Response,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use makakoo_core::transport::gateway::InboundSink;
use makakoo_core::transport::whatsapp::{
    WaInboundOutcome, WaWebhookEnvelope, WhatsAppAdapter,
};

use crate::webhook_router::{VerifyError, WebhookHandler, WebhookRequest};

pub const SIGNATURE_HEADER: &str = "X-Hub-Signature-256";

type HmacSha256 = Hmac<Sha256>;

pub struct WhatsAppWebhookHandler {
    pub adapter: Arc<WhatsAppAdapter>,
    pub verify_token: String,
    pub app_secret: String,
    /// Optional inbound sink — when wired, decoded frames flow into
    /// the supervisor's IPC channel. None = drop frames silently
    /// (used by tests that only assert the response shape).
    pub sink: Option<InboundSink>,
}

impl WhatsAppWebhookHandler {
    pub fn new(
        adapter: Arc<WhatsAppAdapter>,
        verify_token: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        Self {
            adapter,
            verify_token: verify_token.into(),
            app_secret: app_secret.into(),
            sink: None,
        }
    }

    pub fn with_sink(mut self, sink: InboundSink) -> Self {
        self.sink = Some(sink);
        self
    }
}

/// Compute the locked signature: `sha256=hex(HMAC-SHA256(secret, body))`.
pub fn compute_signature(secret: &str, raw_body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts arbitrary key length");
    mac.update(raw_body);
    let bytes = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(bytes))
}

fn constant_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn parse_query(uri: &axum::http::Uri) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Some(q) = uri.query() {
        for pair in q.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let k = url_decode(k);
            let v = url_decode(v);
            out.insert(k, v);
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    // Minimal percent-decoding — Meta's hub.* params are well-formed
    // alnum + `+`. We replace `+` with space and decode `%XX`.
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
        } else if b == b'%' && i + 2 < bytes.len() {
            let hex = &bytes[i + 1..=i + 2];
            if let Ok(s) = std::str::from_utf8(hex) {
                if let Ok(byte) = u8::from_str_radix(s, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
            out.push(b);
            i += 1;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[async_trait]
impl WebhookHandler for WhatsAppWebhookHandler {
    fn verify(&self, req: &WebhookRequest) -> Result<(), VerifyError> {
        if req.method == Method::GET {
            // GET handshake: verify_token equality only — the
            // hub.challenge content itself is the response, not part
            // of an HMAC. Token mismatch → 401.
            let q = parse_query(&req.uri);
            let expected = self.verify_token.as_str();
            let mode = q.get("hub.mode").map(String::as_str).unwrap_or("");
            let token = q.get("hub.verify_token").map(String::as_str).unwrap_or("");
            if mode != "subscribe" {
                return Err(VerifyError::BadRequest("hub.mode != subscribe".into()));
            }
            if !constant_eq(token, expected) {
                return Err(VerifyError::BadCookie);
            }
            if !q.contains_key("hub.challenge") {
                return Err(VerifyError::BadRequest("missing hub.challenge".into()));
            }
            return Ok(());
        }
        // POST — require X-Hub-Signature-256.
        let sig = req
            .headers
            .get(SIGNATURE_HEADER)
            .and_then(|v| v.to_str().ok())
            .ok_or(VerifyError::BadSignature)?;
        let expected = compute_signature(&self.app_secret, &req.raw_body);
        if !constant_eq(sig, &expected) {
            return Err(VerifyError::InvalidSignature);
        }
        Ok(())
    }

    async fn handle(&self, req: WebhookRequest) -> Response {
        if req.method == Method::GET {
            let q = parse_query(&req.uri);
            // verify already confirmed challenge presence.
            let challenge = q.get("hub.challenge").cloned().unwrap_or_default();
            return AxumResponse::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/plain")
                .body(Body::from(challenge))
                .unwrap();
        }

        // POST: parse envelope, dispatch.
        let env: WaWebhookEnvelope = match serde_json::from_slice(&req.raw_body) {
            Ok(v) => v,
            Err(_) => {
                // Meta retries indefinitely on non-200 — we accept the
                // envelope (200) and log at WARN. Internal-malformed
                // bodies are not the sender's problem.
                tracing::warn!(
                    target: "makakoo_mcp::whatsapp_webhook",
                    "malformed whatsapp webhook body — dropping with 200"
                );
                return AxumResponse::builder()
                    .status(StatusCode::OK)
                    .body(Body::empty())
                    .unwrap();
            }
        };

        match self.adapter.map_inbound(env) {
            WaInboundOutcome::Frames(frames) => {
                if let Some(sink) = self.sink.as_ref() {
                    for f in frames {
                        // Sink-closed = supervisor shutting down; drop
                        // and exit the loop. Reply 200 either way so
                        // Meta doesn't retry.
                        if sink.send(f).await.is_err() {
                            break;
                        }
                    }
                }
            }
            WaInboundOutcome::MediaDrop { wa_ids } => {
                // Spec: surface limitation politely. Best-effort send
                // on a background task so we can return 200 quickly.
                let adapter = self.adapter.clone();
                tokio::spawn(async move {
                    use makakoo_core::transport::frame::MakakooOutboundFrame;
                    use makakoo_core::transport::Transport;
                    use makakoo_core::transport::whatsapp::MEDIA_DROP_REPLY;
                    for wa_id in wa_ids {
                        let frame = MakakooOutboundFrame {
                            transport_id: adapter.ctx.transport_id.clone(),
                            transport_kind: "whatsapp".into(),
                            conversation_id: wa_id,
                            thread_id: None,
                            thread_kind: None,
                            text: MEDIA_DROP_REPLY.into(),
                            reply_to_message_id: None,
                        };
                        if let Err(e) = adapter.send(&frame).await {
                            tracing::warn!(
                                target: "makakoo_mcp::whatsapp_webhook",
                                error = %e,
                                "media-drop reply send failed"
                            );
                        }
                    }
                });
            }
            WaInboundOutcome::Status | WaInboundOutcome::Empty => {}
        }
        AxumResponse::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Bytes};
    use axum::http::{HeaderMap, HeaderValue, Method, Uri};
    use makakoo_core::transport::config::WhatsAppConfig;
    use makakoo_core::transport::whatsapp::{WhatsAppAdapter, WhatsAppAdapter as _};
    use makakoo_core::transport::TransportContext;
    use std::sync::Arc;

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "wa-main".into(),
        }
    }

    fn adapter(api_base: String, allowed: Vec<String>) -> Arc<WhatsAppAdapter> {
        let cfg = WhatsAppConfig {
            phone_number_id: "12345".into(),
            graph_version: "v18.0".into(),
            verify_token_env: None,
            verify_token_ref: None,
            inline_verify_token_dev: Some("HUBVERIFY".into()),
            app_secret_env: None,
            app_secret_ref: None,
            inline_app_secret_dev: Some("APPSECRET".into()),
            allowed_wa_ids: allowed.clone(),
        };
        Arc::new(WhatsAppAdapter::with_api_base(
            ctx(),
            cfg,
            "ACCESS".into(),
            allowed,
            api_base,
        ))
    }

    fn handler(allowed: Vec<String>) -> WhatsAppWebhookHandler {
        let a = adapter("http://unused".into(), allowed);
        WhatsAppWebhookHandler::new(a, "HUBVERIFY", "APPSECRET")
    }

    fn req(method: Method, uri: &str, headers: HeaderMap, body: &[u8]) -> WebhookRequest {
        WebhookRequest {
            headers,
            uri: uri.parse::<Uri>().unwrap(),
            method,
            raw_body: Bytes::copy_from_slice(body),
            extensions: axum::http::Extensions::default(),
        }
    }

    // ── GET handshake ─────────────────────────────────────────

    #[tokio::test]
    async fn get_handshake_with_correct_token_responds_with_challenge() {
        let h = handler(vec![]);
        let r = req(
            Method::GET,
            "/x?hub.mode=subscribe&hub.verify_token=HUBVERIFY&hub.challenge=ECHO123",
            HeaderMap::new(),
            b"",
        );
        h.verify(&r).unwrap();
        let resp = h.handle(r).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ECHO123");
    }

    #[tokio::test]
    async fn get_handshake_with_wrong_token_returns_bad_cookie() {
        let h = handler(vec![]);
        let r = req(
            Method::GET,
            "/x?hub.mode=subscribe&hub.verify_token=WRONG&hub.challenge=ECHO",
            HeaderMap::new(),
            b"",
        );
        let err = h.verify(&r).unwrap_err();
        assert!(matches!(err, VerifyError::BadCookie));
    }

    #[tokio::test]
    async fn get_handshake_without_subscribe_mode_is_bad_request() {
        let h = handler(vec![]);
        let r = req(
            Method::GET,
            "/x?hub.mode=other&hub.verify_token=HUBVERIFY&hub.challenge=Y",
            HeaderMap::new(),
            b"",
        );
        let err = h.verify(&r).unwrap_err();
        assert!(matches!(err, VerifyError::BadRequest(_)));
    }

    // ── POST signature ────────────────────────────────────────

    #[tokio::test]
    async fn post_with_valid_signature_passes_verify() {
        let h = handler(vec![]);
        let body = br#"{"object":"whatsapp_business_account","entry":[]}"#;
        let sig = compute_signature("APPSECRET", body);
        let mut hdrs = HeaderMap::new();
        hdrs.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        let r = req(Method::POST, "/x", hdrs, body);
        h.verify(&r).unwrap();
    }

    #[tokio::test]
    async fn post_without_signature_header_fails() {
        let h = handler(vec![]);
        let r = req(Method::POST, "/x", HeaderMap::new(), b"{}");
        let err = h.verify(&r).unwrap_err();
        assert!(matches!(err, VerifyError::BadSignature));
    }

    #[tokio::test]
    async fn post_with_tampered_body_fails() {
        let h = handler(vec![]);
        let body = b"{}";
        let sig = compute_signature("APPSECRET", body);
        let mut hdrs = HeaderMap::new();
        hdrs.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        // Send a different body than the one we signed.
        let r = req(Method::POST, "/x", hdrs, b"{tampered}");
        let err = h.verify(&r).unwrap_err();
        assert!(matches!(err, VerifyError::InvalidSignature));
    }

    // ── POST handle dispatches frames ─────────────────────────

    #[tokio::test]
    async fn post_text_message_pushes_frame_to_sink() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut h = handler(vec!["34000000001".into()]);
        h.sink = Some(tx);

        let body = serde_json::to_vec(&serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "E1",
                "changes": [{
                    "field": "messages",
                    "value": {
                        "messaging_product": "whatsapp",
                        "metadata": {"phone_number_id": "12345"},
                        "messages": [{
                            "id": "wamid.A",
                            "from": "34000000001",
                            "timestamp": "1700000000",
                            "type": "text",
                            "text": {"body": "hi"}
                        }]
                    }
                }]
            }]
        }))
        .unwrap();
        let sig = compute_signature("APPSECRET", &body);
        let mut hdrs = HeaderMap::new();
        hdrs.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        let r = req(Method::POST, "/x", hdrs, &body);
        h.verify(&r).unwrap();
        let resp = h.handle(r).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let frame = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("sink delivery timeout")
            .expect("sink closed");
        assert_eq!(frame.text, "hi");
        assert_eq!(frame.sender_id, "34000000001");
    }

    #[tokio::test]
    async fn post_malformed_body_responds_200_and_does_not_panic() {
        let h = handler(vec![]);
        let body = b"not-json";
        let sig = compute_signature("APPSECRET", body);
        let mut hdrs = HeaderMap::new();
        hdrs.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        let r = req(Method::POST, "/x", hdrs, body);
        h.verify(&r).unwrap();
        let resp = h.handle(r).await;
        // Locked: we still 200 so Meta doesn't retry indefinitely.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── status + filtering ────────────────────────────────────

    #[tokio::test]
    async fn post_status_only_envelope_does_not_emit_frame() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut h = handler(vec!["34000000001".into()]);
        h.sink = Some(tx);

        let body = serde_json::to_vec(&serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "E",
                "changes": [{
                    "field": "messages",
                    "value": {
                        "messaging_product": "whatsapp",
                        "messages": [],
                        "statuses": [{"id":"x","status":"delivered"}]
                    }
                }]
            }]
        }))
        .unwrap();
        let sig = compute_signature("APPSECRET", &body);
        let mut hdrs = HeaderMap::new();
        hdrs.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        let r = req(Method::POST, "/x", hdrs, &body);
        h.verify(&r).unwrap();
        let _ = h.handle(r).await;

        // No frames should have been delivered.
        let outcome = tokio::time::timeout(std::time::Duration::from_millis(80), rx.recv()).await;
        assert!(
            outcome.is_err() || outcome.unwrap().is_none(),
            "status-only must not produce frames"
        );
    }

    // ── compute_signature determinism ─────────────────────────

    #[test]
    fn compute_signature_deterministic_and_prefixed() {
        let s = compute_signature("KEY", b"body");
        assert!(s.starts_with("sha256="));
        let s2 = compute_signature("KEY", b"body");
        assert_eq!(s, s2);
    }

    // suppress unused-import warning
    #[allow(dead_code)]
    fn _unused(_x: ()) -> WhatsAppAdapter {
        unimplemented!()
    }
}
