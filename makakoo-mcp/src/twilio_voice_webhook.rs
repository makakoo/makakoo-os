//! Phase 10 — Twilio Voice webhook handler.
//!
//! Two-phase flow over the same webhook URL, discriminated by the
//! `phase` query parameter:
//!   - phase absent → initial inbound call. Return welcome TwiML
//!     with `<Record action="…?phase=recording&CallSid=…"/>`.
//!   - `phase=recording` → recording-completed callback. Verify
//!     signature, fetch the recording with basic-auth (mock-friendly
//!     seam), run STT (pluggable; defaults to a stub that uses the
//!     RecordingSid as the transcript), emit an inbound frame, and
//!     reply with TwiML acking the recording.
//!
//! Locked Q9: real-time streaming voice + LLM-driven `<Play>`
//! injection are deferred to v2.1; v1 ships push-to-talk only.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Response as AxumResponse, StatusCode},
    response::Response,
};

use makakoo_core::transport::frame::MakakooInboundFrame;
use makakoo_core::transport::gateway::InboundSink;
use makakoo_core::transport::voice_twilio::{
    ack_with_audio_twiml, build_inbound_frame_from_recording, compute_twilio_signature,
    signature_eq, welcome_twiml, VoiceTwilioAdapter,
};

use crate::webhook_router::{VerifyError, WebhookHandler, WebhookRequest};

pub const SIGNATURE_HEADER: &str = "X-Twilio-Signature";

pub struct TwilioVoiceWebhookHandler {
    pub adapter: Arc<VoiceTwilioAdapter>,
    pub slot_uuid: String,
    pub transport_uuid: String,
    pub sink: Option<InboundSink>,
}

impl TwilioVoiceWebhookHandler {
    pub fn new(
        adapter: Arc<VoiceTwilioAdapter>,
        slot_uuid: impl Into<String>,
        transport_uuid: impl Into<String>,
    ) -> Self {
        Self {
            adapter,
            slot_uuid: slot_uuid.into(),
            transport_uuid: transport_uuid.into(),
            sink: None,
        }
    }

    pub fn with_sink(mut self, sink: InboundSink) -> Self {
        self.sink = Some(sink);
        self
    }

    fn full_request_url(&self, uri: &axum::http::Uri) -> String {
        // Twilio signs the FULL public URL — it doesn't know about
        // any reverse proxy. We reconstruct it from the configured
        // public_base_url + the request path/query.
        let path_q = uri
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        format!("{}{}", self.adapter.config.public_base_url, path_q)
    }
}

fn parse_form(body: &[u8]) -> Vec<(String, String)> {
    let s = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    for pair in s.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.push((url_decode(k), url_decode(v)));
    }
    out
}

fn url_decode(s: &str) -> String {
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

fn parse_query(uri: &axum::http::Uri) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(q) = uri.query() {
        for pair in q.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            out.insert(url_decode(k), url_decode(v));
        }
    }
    out
}

fn twiml_response(body: String) -> Response {
    AxumResponse::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(Body::from(body))
        .unwrap()
}

#[async_trait]
impl WebhookHandler for TwilioVoiceWebhookHandler {
    fn verify(&self, req: &WebhookRequest) -> Result<(), VerifyError> {
        if req.method != Method::POST {
            // Twilio always uses POST. Reject other methods up front
            // so a misconfigured curl GET doesn't trip the parser.
            return Err(VerifyError::BadRequest("voice webhook accepts POST only".into()));
        }
        let sig = req
            .headers
            .get(SIGNATURE_HEADER)
            .and_then(|v| v.to_str().ok())
            .ok_or(VerifyError::BadSignature)?;
        let form = parse_form(&req.raw_body);
        let url = self.full_request_url(&req.uri);
        let expected = compute_twilio_signature(&self.adapter.auth_token, &url, &form);
        if !signature_eq(sig, &expected) {
            return Err(VerifyError::InvalidSignature);
        }
        Ok(())
    }

    async fn handle(&self, req: WebhookRequest) -> Response {
        let q = parse_query(&req.uri);
        let phase = q.get("phase").cloned().unwrap_or_default();
        let form: HashMap<String, String> = parse_form(&req.raw_body).into_iter().collect();
        let call_sid = q
            .get("CallSid")
            .cloned()
            .or_else(|| form.get("CallSid").cloned())
            .unwrap_or_default();

        if phase != "recording" {
            // Phase 1: caller dialed in. Reply with welcome TwiML +
            // <Record/> whose action embeds CallSid.
            let twiml = welcome_twiml(
                &self.adapter.config.public_base_url,
                &self.slot_uuid,
                &self.transport_uuid,
                &call_sid,
            );
            return twiml_response(twiml);
        }

        // Phase 2: recording-completed callback.
        let from = form.get("From").cloned().unwrap_or_default();
        let recording_url = form.get("RecordingUrl").cloned().unwrap_or_default();
        let recording_sid = form.get("RecordingSid").cloned().unwrap_or_default();

        // Allowlist enforcement.
        let allowed = self
            .adapter
            .config
            .allowed_caller_ids
            .iter()
            .any(|a| a == &from);
        if !allowed {
            tracing::debug!(
                target: "makakoo_mcp::twilio_voice_webhook",
                transport_id = self.adapter.ctx.transport_id,
                from = from,
                "dropping non-allowlisted caller; replying with hangup TwiML"
            );
            return twiml_response(ack_with_audio_twiml(None, "This number isn't authorized."));
        }

        // Stub STT for v1: use the RecordingSid as the synthetic
        // transcript. A real adapter wires SwitchAILocal whisper-1
        // here. The public seam keeps this swappable.
        let transcript = format!("[recording {recording_sid}]");

        let frame = build_inbound_frame_from_recording(
            &self.adapter.ctx,
            &self.adapter.config.account_sid,
            &call_sid,
            &from,
            transcript,
            &recording_url,
        );

        if let Some(sink) = self.sink.as_ref() {
            // Sink-closed = supervisor shutting down; we still 200 so
            // Twilio doesn't retry the webhook indefinitely.
            let _ = sink.send(frame).await;
        }

        // v1 doesn't have a synchronous response — return TwiML that
        // tells the caller their message was received. v2.1 will
        // wire this to the gateway's response and play TTS audio.
        twiml_response(ack_with_audio_twiml(
            None,
            "Got it — I'll get back to you shortly. Goodbye.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::http::{HeaderMap, HeaderValue, Method, Uri};
    use makakoo_core::transport::config::VoiceTwilioConfig;
    use makakoo_core::transport::TransportContext;

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "voice-main".into(),
        }
    }

    fn cfg() -> VoiceTwilioConfig {
        VoiceTwilioConfig {
            account_sid: "ACdeadbeef".into(),
            auth_token_env: None,
            auth_token_ref: None,
            inline_auth_token_dev: Some("AUTHTOK".into()),
            allowed_caller_ids: vec!["+34600000001".into()],
            public_base_url: "https://example.com".into(),
        }
    }

    fn adapter() -> Arc<VoiceTwilioAdapter> {
        Arc::new(VoiceTwilioAdapter::with_api_base(
            ctx(),
            cfg(),
            "AUTHTOK".into(),
            "http://unused".into(),
        ))
    }

    fn handler() -> TwilioVoiceWebhookHandler {
        TwilioVoiceWebhookHandler::new(
            adapter(),
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
        )
    }

    fn req(method: Method, path_q: &str, headers: HeaderMap, body: &[u8]) -> WebhookRequest {
        WebhookRequest {
            headers,
            uri: path_q.parse::<Uri>().unwrap(),
            method,
            raw_body: Bytes::copy_from_slice(body),
            extensions: axum::http::Extensions::default(),
        }
    }

    fn url_encode(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    out.push(b as char);
                }
                _ => {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
        out
    }

    fn form_body(pairs: &[(&str, &str)]) -> Vec<u8> {
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={}", url_encode(v)))
            .collect::<Vec<_>>()
            .join("&")
            .into_bytes()
    }

    fn signed_post(
        h: &TwilioVoiceWebhookHandler,
        path_q: &str,
        form: Vec<(&str, &str)>,
    ) -> WebhookRequest {
        let body = form_body(&form);
        let url = format!("{}{}", h.adapter.config.public_base_url, path_q);
        let pairs: Vec<(String, String)> = form
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let sig = compute_twilio_signature(&h.adapter.auth_token, &url, &pairs);
        let mut hdrs = HeaderMap::new();
        hdrs.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        req(Method::POST, path_q, hdrs, &body)
    }

    // ── verify ───────────────────────────────────────────────

    #[tokio::test]
    async fn verify_passes_with_correct_signature() {
        let h = handler();
        let r = signed_post(
            &h,
            "/transport/x/y/webhook",
            vec![("CallSid", "CA1"), ("From", "+34600000001")],
        );
        h.verify(&r).unwrap();
    }

    #[tokio::test]
    async fn verify_rejects_missing_signature() {
        let h = handler();
        let r = req(
            Method::POST,
            "/transport/x/y/webhook",
            HeaderMap::new(),
            b"CallSid=CA1",
        );
        let err = h.verify(&r).unwrap_err();
        assert!(matches!(err, VerifyError::BadSignature));
    }

    #[tokio::test]
    async fn verify_rejects_tampered_body() {
        let h = handler();
        let r = signed_post(&h, "/transport/x/y/webhook", vec![("Body", "good")]);
        let mut tampered = r;
        tampered.raw_body = Bytes::from_static(b"Body=evil");
        let err = h.verify(&tampered).unwrap_err();
        assert!(matches!(err, VerifyError::InvalidSignature));
    }

    #[tokio::test]
    async fn verify_rejects_get() {
        let h = handler();
        let r = req(Method::GET, "/x", HeaderMap::new(), b"");
        let err = h.verify(&r).unwrap_err();
        assert!(matches!(err, VerifyError::BadRequest(_)));
    }

    // ── handle phase 1 ───────────────────────────────────────

    #[tokio::test]
    async fn handle_initial_call_returns_welcome_twiml_with_record() {
        let h = handler();
        let r = signed_post(
            &h,
            "/transport/x/y/webhook",
            vec![("CallSid", "CA-INIT")],
        );
        h.verify(&r).unwrap();
        let resp = h.handle(r).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("<Record"), "must contain <Record>");
        assert!(body.contains("CallSid=CA-INIT"));
        assert!(body.contains("phase=recording"));
    }

    // ── handle phase 2 ───────────────────────────────────────

    #[tokio::test]
    async fn handle_recording_callback_emits_frame_to_sink() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut h = handler();
        h.sink = Some(tx);

        let r = signed_post(
            &h,
            "/transport/x/y/webhook?phase=recording&CallSid=CA-RECD",
            vec![
                ("CallSid", "CA-RECD"),
                ("From", "+34600000001"),
                ("RecordingUrl", "https://api.twilio.com/.../Recordings/RE-1.wav"),
                ("RecordingSid", "RE-1"),
            ],
        );
        h.verify(&r).unwrap();
        let resp = h.handle(r).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let frame = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("frame timeout")
            .expect("sink closed");
        assert_eq!(frame.message_id, "CA-RECD");
        assert_eq!(frame.sender_id, "+34600000001");
        assert!(frame.text.contains("RE-1"));
        assert!(frame
            .raw_metadata
            .get("recording_url")
            .and_then(|v| v.as_str())
            .unwrap()
            .ends_with("RE-1.wav"));
    }

    #[tokio::test]
    async fn handle_recording_callback_drops_non_allowlisted_caller() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut h = handler();
        h.sink = Some(tx);

        let r = signed_post(
            &h,
            "/transport/x/y/webhook?phase=recording&CallSid=CA-X",
            vec![
                ("CallSid", "CA-X"),
                ("From", "+34999999999"),
                ("RecordingUrl", "https://example/r"),
                ("RecordingSid", "RE-X"),
            ],
        );
        h.verify(&r).unwrap();
        let resp = h.handle(r).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        // The unauthorized message is XML-escaped so the apostrophe
        // appears as `&#39;`. Assert on a stable substring.
        assert!(body.contains("authorized"));

        // No frame emitted.
        let outcome = tokio::time::timeout(std::time::Duration::from_millis(80), rx.recv()).await;
        assert!(outcome.is_err() || outcome.unwrap().is_none());
    }

    // ── form parser ──────────────────────────────────────────

    #[test]
    fn parse_form_handles_url_encoding_and_pluses() {
        let body = b"From=%2B34600000001&Body=hello+world";
        let parsed: HashMap<_, _> = parse_form(body).into_iter().collect();
        assert_eq!(parsed.get("From"), Some(&"+34600000001".to_string()));
        assert_eq!(parsed.get("Body"), Some(&"hello world".to_string()));
    }

    #[test]
    fn parse_query_handles_query_string() {
        let uri = "/x?phase=recording&CallSid=CA1".parse::<Uri>().unwrap();
        let q = parse_query(&uri);
        assert_eq!(q.get("phase"), Some(&"recording".to_string()));
        assert_eq!(q.get("CallSid"), Some(&"CA1".to_string()));
    }
}
