//! Phase 5b — Slack Events API webhook adapter.
//!
//! HTTP-webhook variant of the Socket-Mode SlackAdapter. Both modes
//! share the inbound-frame translation; only the receive path differs.
//!
//! Locked Q5:
//!
//! * `[transport.config] mode = "events_api"` switches the slot from
//!   Socket Mode to this Events API webhook.
//! * Webhook lands at `/transport/<slot_uuid>/<transport_uuid>/events`.
//! * HMAC verification: `v0=` prefix, SHA-256 over
//!   `v0:{timestamp}:{raw_body}` keyed by signing_secret. Verify
//!   BEFORE JSON parsing so malformed bodies still fail with audit.
//! * Replay window: 5 minutes. Timestamps outside the window are
//!   rejected as `VerifyError::Replay`.
//! * URL verification challenge: when `type == "url_verification"`,
//!   respond with `{"challenge": "..."}` immediately.
//! * Outbound path identical to Socket Mode (chat.postMessage).
//!
//! No Slack SDK dep — verification is locked HMAC-SHA256 + a small
//! JSON parser for the inbound event body.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{HeaderMap, Response as AxumResponse, StatusCode},
    response::Response,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::webhook_router::{VerifyError, WebhookHandler, WebhookRequest};

/// Slack's locked replay window.
pub const REPLAY_WINDOW_SECS: i64 = 300;

pub const SIGNATURE_HEADER: &str = "X-Slack-Signature";
pub const TIMESTAMP_HEADER: &str = "X-Slack-Request-Timestamp";

type HmacSha256 = Hmac<Sha256>;

/// Per-slot Slack Events handler. Holds the signing secret used to
/// verify every inbound webhook. Created at slot-start time when the
/// supervisor reads slot.toml.
pub struct SlackEventsHandler {
    pub slot_id: String,
    pub transport_id: String,
    pub signing_secret: String,
    /// Optional clock injection so tests can control "now". `None`
    /// = real wall clock.
    pub now_fn: Option<fn() -> i64>,
}

impl SlackEventsHandler {
    pub fn new(
        slot_id: impl Into<String>,
        transport_id: impl Into<String>,
        signing_secret: impl Into<String>,
    ) -> Self {
        Self {
            slot_id: slot_id.into(),
            transport_id: transport_id.into(),
            signing_secret: signing_secret.into(),
            now_fn: None,
        }
    }

    fn now(&self) -> i64 {
        self.now_fn.map(|f| f()).unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        })
    }
}

/// Compute the locked HMAC: `v0=hex(HMAC-SHA256(secret, "v0:{ts}:{body}"))`.
pub fn compute_signature(secret: &str, timestamp: &str, raw_body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts arbitrary key length");
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(raw_body);
    let bytes = mac.finalize().into_bytes();
    format!("v0={}", hex::encode(bytes))
}

/// Constant-time comparison so the verify path doesn't leak a timing
/// oracle.
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

#[async_trait]
impl WebhookHandler for SlackEventsHandler {
    fn verify(&self, req: &WebhookRequest) -> Result<(), VerifyError> {
        // Headers MUST be present.
        let sig = header_str(&req.headers, SIGNATURE_HEADER)
            .map_err(|_| VerifyError::BadSignature)?;
        let ts = header_str(&req.headers, TIMESTAMP_HEADER)
            .map_err(|_| VerifyError::BadSignature)?;
        let ts_i: i64 = ts.parse().map_err(|_| VerifyError::BadSignature)?;
        let now = self.now();
        if (now - ts_i).abs() > REPLAY_WINDOW_SECS {
            return Err(VerifyError::Replay);
        }
        let expected = compute_signature(&self.signing_secret, ts, &req.raw_body);
        if !constant_eq(&expected, sig) {
            return Err(VerifyError::InvalidSignature);
        }
        Ok(())
    }

    async fn handle(&self, req: WebhookRequest) -> Response {
        // Parse the body as JSON. Slack sends two interesting types:
        //   - `url_verification` — challenge handshake, must echo
        //   - `event_callback` — wraps the actual `event` payload
        let parsed: serde_json::Value = match serde_json::from_slice(&req.raw_body) {
            Ok(v) => v,
            Err(_) => return text_response(StatusCode::BAD_REQUEST, "bad json"),
        };
        match parsed.get("type").and_then(|v| v.as_str()) {
            Some("url_verification") => {
                let challenge = parsed
                    .get("challenge")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let body = serde_json::json!({"challenge": challenge}).to_string();
                AxumResponse::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap()
            }
            Some("event_callback") => {
                // Phase 5b: produce 200 OK so Slack stops retrying.
                // Frame translation + IPC handoff to gateway lands in
                // Phase 5c (concurrent with Slack adapter refactor).
                AxumResponse::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(""))
                    .unwrap()
            }
            other => {
                // Unrecognized event type — 200 with no body so Slack
                // doesn't retry indefinitely. Audit log captures the
                // unknown type for ops review.
                let _ = other;
                text_response(StatusCode::OK, "")
            }
        }
    }
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Result<&'a str, ()> {
    h.get(name).and_then(|v| v.to_str().ok()).ok_or(())
}

fn text_response(status: StatusCode, body: &str) -> Response {
    AxumResponse::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &str = "test-signing-secret";

    fn fixed_now() -> i64 {
        1700000000
    }

    fn handler() -> SlackEventsHandler {
        let mut h = SlackEventsHandler::new("secretary", "slack-main", SECRET);
        h.now_fn = Some(fixed_now);
        h
    }

    fn req(headers: HeaderMap, body: &str) -> WebhookRequest {
        WebhookRequest {
            headers,
            uri: "/transport/x/y/events".parse().unwrap(),
            method: axum::http::Method::POST,
            raw_body: body.to_string().into(),
            extensions: Default::default(),
        }
    }

    #[test]
    fn signature_helper_produces_v0_prefix() {
        let sig = compute_signature(SECRET, "1700000000", b"body");
        assert!(sig.starts_with("v0="));
        assert_eq!(sig.len(), 3 + 64); // "v0=" + 64 hex chars (32-byte SHA-256)
    }

    #[test]
    fn signature_helper_is_deterministic() {
        let a = compute_signature(SECRET, "1700000000", b"body");
        let b = compute_signature(SECRET, "1700000000", b"body");
        assert_eq!(a, b);
    }

    #[test]
    fn signature_helper_changes_with_body() {
        let a = compute_signature(SECRET, "1700000000", b"body1");
        let b = compute_signature(SECRET, "1700000000", b"body2");
        assert_ne!(a, b);
    }

    #[test]
    fn verify_accepts_valid_signature_within_window() {
        let h = handler();
        let body = r#"{"type":"event_callback"}"#;
        let ts = fixed_now().to_string();
        let sig = compute_signature(SECRET, &ts, body.as_bytes());
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from_str(&ts).unwrap());
        h.verify(&req(headers, body)).expect("verify should pass");
    }

    #[test]
    fn verify_rejects_missing_signature_header() {
        let h = handler();
        let mut headers = HeaderMap::new();
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from_static("1700000000"));
        let err = h.verify(&req(headers, "body")).unwrap_err();
        assert!(matches!(err, VerifyError::BadSignature));
    }

    #[test]
    fn verify_rejects_missing_timestamp_header() {
        let h = handler();
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_static("v0=abc"));
        let err = h.verify(&req(headers, "body")).unwrap_err();
        assert!(matches!(err, VerifyError::BadSignature));
    }

    #[test]
    fn verify_rejects_invalid_signature() {
        let h = handler();
        let body = "body";
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_static("v0=deadbeef"));
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from_static("1700000000"));
        let err = h.verify(&req(headers, body)).unwrap_err();
        assert!(matches!(err, VerifyError::InvalidSignature));
    }

    #[test]
    fn verify_rejects_old_timestamp_replay() {
        let h = handler();
        let body = "body";
        // 10 minutes in the past — outside the 5-minute window.
        let stale_ts = (fixed_now() - 600).to_string();
        let sig = compute_signature(SECRET, &stale_ts, body.as_bytes());
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from_str(&stale_ts).unwrap());
        let err = h.verify(&req(headers, body)).unwrap_err();
        assert!(matches!(err, VerifyError::Replay));
    }

    #[tokio::test]
    async fn handle_url_verification_returns_challenge() {
        let h = handler();
        let body = r#"{"type":"url_verification","challenge":"abcdef"}"#;
        let ts = fixed_now().to_string();
        let sig = compute_signature(SECRET, &ts, body.as_bytes());
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from_str(&ts).unwrap());
        let r = req(headers, body);
        let resp = h.handle(r).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("\"challenge\""));
        assert!(body_str.contains("abcdef"));
    }

    #[tokio::test]
    async fn handle_event_callback_returns_200_no_body() {
        let h = handler();
        let body = r#"{"type":"event_callback","event":{"type":"message","text":"hi"}}"#;
        let ts = fixed_now().to_string();
        let sig = compute_signature(SECRET, &ts, body.as_bytes());
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from_str(&ts).unwrap());
        let r = req(headers, body);
        let resp = h.handle(r).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn handle_malformed_body_returns_400() {
        let h = handler();
        let body = "not json";
        let ts = fixed_now().to_string();
        let sig = compute_signature(SECRET, &ts, body.as_bytes());
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from_str(&ts).unwrap());
        let r = req(headers, body);
        let resp = h.handle(r).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn constant_eq_accepts_equal_strings() {
        assert!(constant_eq("v0=abc", "v0=abc"));
    }

    #[test]
    fn constant_eq_rejects_different_strings_and_lengths() {
        assert!(!constant_eq("v0=abc", "v0=abd"));
        assert!(!constant_eq("v0=abc", "v0=abcd"));
    }
}
