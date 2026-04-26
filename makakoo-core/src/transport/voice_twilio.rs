//! Voice (Twilio + TwiML) transport — Phase 10 / Q9.
//!
//! Push-to-talk model:
//!   1. Caller dials the Twilio number → Twilio POSTs to
//!      `/transport/<slot>/<transport>/webhook` (form-encoded).
//!   2. We respond with TwiML that contains a `<Record>` element
//!      whose `action` URL embeds the CallSid for correlation.
//!   3. Caller speaks → Twilio uploads the recording → POSTs
//!      again with `RecordingUrl` + `CallSid`.
//!   4. We fetch the recording with HTTP basic-auth (account_sid +
//!      auth_token), run STT (mock-friendly seam), emit a
//!      `MakakooInboundFrame`. The TwiML response Plays a
//!      placeholder `<Say>` ack while the gateway processes.
//!   5. Real-time streaming voice + LLM-driven `<Play>` reply
//!      injection are documented as deferred to v2.1 (Q9).
//!
//! Locked Q9 details:
//!   - Recording-callback URL embeds `CallSid` so the second webhook
//!     can be correlated to the first.
//!   - X-Twilio-Signature uses HMAC-SHA1 over the request URL +
//!     sorted POST params concatenated.
//!   - Auth token doubles as the basic-auth password for recording
//!     fetches.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::transport::config::VoiceTwilioConfig;
use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame};
use crate::transport::gateway::{Gateway, InboundSink};
use crate::transport::{Transport, TransportContext, VerifiedIdentity};
use crate::{MakakooError, Result};

const TWILIO_API_BASE: &str = "https://api.twilio.com";

pub struct VoiceTwilioAdapter {
    pub ctx: TransportContext,
    pub config: VoiceTwilioConfig,
    /// Resolved auth_token (used for both signature verify + basic-auth).
    pub auth_token: String,
    /// Twilio API base override for tests.
    pub api_base: String,
    pub http: reqwest::Client,
}

impl VoiceTwilioAdapter {
    pub fn new(
        ctx: TransportContext,
        config: VoiceTwilioConfig,
        auth_token: String,
    ) -> Self {
        Self::with_api_base(ctx, config, auth_token, TWILIO_API_BASE.into())
    }

    pub fn with_api_base(
        ctx: TransportContext,
        config: VoiceTwilioConfig,
        auth_token: String,
        api_base: String,
    ) -> Self {
        Self {
            ctx,
            config,
            auth_token,
            api_base,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn account_url(&self, suffix: &str) -> String {
        format!(
            "{}/2010-04-01/Accounts/{}{}",
            self.api_base, self.config.account_sid, suffix
        )
    }
}

// ── REST verify ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct TwilioAccountInfo {
    sid: String,
    #[serde(default)]
    friendly_name: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[async_trait]
impl Transport for VoiceTwilioAdapter {
    fn kind(&self) -> &'static str {
        "voice_twilio"
    }
    fn transport_id(&self) -> &str {
        &self.ctx.transport_id
    }

    async fn verify_credentials(&self) -> Result<VerifiedIdentity> {
        // GET /Accounts/{sid}.json with basic auth — confirms both
        // that the auth_token is correct and that it has access to
        // the configured account.
        let url = self.account_url(".json");
        let resp = self
            .http
            .get(&url)
            .basic_auth(&self.config.account_sid, Some(&self.auth_token))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MakakooError::Config(format!(
                "twilio account verify failed: HTTP {status}: {body}"
            )));
        }
        let info: TwilioAccountInfo = resp.json().await?;
        if info.sid != self.config.account_sid {
            return Err(MakakooError::Config(format!(
                "twilio account_sid mismatch: TOML='{}' but API returned '{}'",
                self.config.account_sid, info.sid
            )));
        }
        Ok(VerifiedIdentity {
            account_id: info.sid,
            tenant_id: None,
            display_name: info.friendly_name,
        })
    }

    async fn send(&self, _frame: &MakakooOutboundFrame) -> Result<()> {
        // Voice replies are returned via TwiML on the webhook
        // response, not via a push API. Outbound text frames in v1
        // are surfaced as TwiML <Say> on the next caller turn —
        // wired by the webhook handler. Direct send is a no-op so
        // the supervisor's outbound queue doesn't error.
        tracing::debug!(
            target: "makakoo_core::transport::voice_twilio",
            transport_id = self.ctx.transport_id,
            "voice_twilio outbound frames are dispatched via TwiML — direct send is a no-op (Q9 push-to-talk model; real-time streaming deferred to v2.1)"
        );
        Ok(())
    }
}

#[async_trait]
impl Gateway for VoiceTwilioAdapter {
    async fn start(&self, _sink: InboundSink) -> Result<()> {
        std::future::pending::<()>().await;
        Ok(())
    }
}

// ── TwiML generation (pure helpers) ────────────────────────────

/// Build the recording-callback URL Twilio POSTs after the recording
/// finishes uploading. Locked Q9: CallSid embedded in the path so
/// the second webhook is correlated to the first call.
pub fn recording_callback_url(public_base: &str, slot_uuid: &str, transport_uuid: &str, call_sid: &str) -> String {
    format!(
        "{public_base}/transport/{slot_uuid}/{transport_uuid}/webhook?CallSid={call_sid}&phase=recording"
    )
}

/// XML-escape a string for safe interpolation into TwiML bodies.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Generate the welcome TwiML returned on the FIRST inbound webhook.
/// Includes a `<Record>` whose action embeds CallSid.
pub fn welcome_twiml(public_base: &str, slot_uuid: &str, transport_uuid: &str, call_sid: &str) -> String {
    let action = xml_escape(&recording_callback_url(public_base, slot_uuid, transport_uuid, call_sid));
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Say>Hello, please leave your message after the tone.</Say>
  <Record action="{action}" maxLength="60" finishOnKey="#" playBeep="true"/>
  <Say>I didn't catch that. Goodbye.</Say>
</Response>"##
    )
}

/// Generate the TwiML returned after a recording is processed.
/// `audio_url` is the TTS-rendered response audio URL (or `None` to
/// fall back to a `<Say>` of `say_fallback_text`).
pub fn ack_with_audio_twiml(audio_url: Option<&str>, say_fallback_text: &str) -> String {
    let body = match audio_url {
        Some(u) => format!("<Play>{}</Play>", xml_escape(u)),
        None => format!("<Say>{}</Say>", xml_escape(say_fallback_text)),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  {body}
  <Hangup/>
</Response>"#
    )
}

// ── X-Twilio-Signature verification ────────────────────────────

/// Compute the locked HMAC-SHA1 signature.
///
/// Twilio's signature is base64(HMAC-SHA1(authToken,
/// `<full_url>` + concat(sorted_form_params: "<key><value>"))).
pub fn compute_twilio_signature(auth_token: &str, full_url: &str, form: &[(String, String)]) -> String {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;

    let mut sorted = form.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut data = String::from(full_url);
    for (k, v) in sorted {
        data.push_str(&k);
        data.push_str(&v);
    }

    type HmacSha1 = Hmac<Sha1>;
    let mut mac =
        HmacSha1::new_from_slice(auth_token.as_bytes()).expect("HMAC accepts arbitrary key length");
    mac.update(data.as_bytes());
    let bytes = mac.finalize().into_bytes();
    base64_encode(&bytes)
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Constant-time string equality.
pub fn signature_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ── inbound frame mapping ──────────────────────────────────────

/// Build a Makakoo inbound frame from a recording-completed Twilio
/// webhook + (caller-supplied) STT text.
pub fn build_inbound_frame_from_recording(
    ctx: &TransportContext,
    account_sid: &str,
    call_sid: &str,
    from: &str,
    transcript: String,
    recording_url: &str,
) -> MakakooInboundFrame {
    let mut raw = std::collections::BTreeMap::new();
    raw.insert(
        "recording_url".into(),
        serde_json::Value::String(recording_url.to_string()),
    );
    raw.insert(
        "twilio_call_sid".into(),
        serde_json::Value::String(call_sid.to_string()),
    );
    MakakooInboundFrame {
        agent_slot_id: ctx.slot_id.clone(),
        transport_id: ctx.transport_id.clone(),
        transport_kind: "voice_twilio".into(),
        account_id: account_sid.to_string(),
        conversation_id: from.to_string(),
        sender_id: from.to_string(),
        thread_id: None,
        thread_kind: None,
        message_id: call_sid.to_string(),
        text: transcript,
        transport_timestamp: None,
        received_at: chrono::Utc::now(),
        raw_metadata: raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    fn adapter(api_base: String) -> VoiceTwilioAdapter {
        VoiceTwilioAdapter::with_api_base(ctx(), cfg(), "AUTHTOK".into(), api_base)
    }

    // ── verify_credentials ───────────────────────────────────

    #[tokio::test]
    async fn verify_credentials_returns_account_sid() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/2010-04-01/Accounts/ACdeadbeef.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sid": "ACdeadbeef",
                "friendly_name": "Makakoo Voice",
                "status": "active"
            })))
            .mount(&server)
            .await;
        let a = adapter(server.uri());
        let id = a.verify_credentials().await.unwrap();
        assert_eq!(id.account_id, "ACdeadbeef");
        assert_eq!(id.display_name.as_deref(), Some("Makakoo Voice"));
    }

    #[tokio::test]
    async fn verify_credentials_rejects_bad_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/2010-04-01/Accounts/ACdeadbeef.json"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .mount(&server)
            .await;
        let a = adapter(server.uri());
        let err = a.verify_credentials().await.unwrap_err();
        assert!(format!("{err}").contains("HTTP 401"));
    }

    // ── TwiML helpers ────────────────────────────────────────

    #[test]
    fn welcome_twiml_embeds_call_sid_in_action() {
        let twiml = welcome_twiml(
            "https://example.com",
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
            "CA123",
        );
        assert!(twiml.contains("CallSid=CA123"));
        assert!(twiml.contains("<Record"));
        assert!(twiml.contains("playBeep=\"true\""));
    }

    #[test]
    fn ack_with_audio_uses_play_when_url_present() {
        let twiml = ack_with_audio_twiml(Some("https://cdn/x.mp3"), "fallback");
        assert!(twiml.contains("<Play>https://cdn/x.mp3</Play>"));
        assert!(!twiml.contains("<Say>fallback</Say>"));
    }

    #[test]
    fn ack_with_audio_falls_back_to_say_when_no_url() {
        let twiml = ack_with_audio_twiml(None, "TTS unavailable");
        assert!(twiml.contains("<Say>TTS unavailable</Say>"));
    }

    #[test]
    fn xml_escape_handles_special_chars() {
        let out = xml_escape("<b>'hi & \"bye\"'</b>");
        assert_eq!(out, "&lt;b&gt;&#39;hi &amp; &quot;bye&quot;&#39;&lt;/b&gt;");
    }

    #[test]
    fn recording_callback_url_includes_phase_marker() {
        let u = recording_callback_url("https://e.com", "S1", "T1", "CA-X");
        assert!(u.contains("phase=recording"));
        assert!(u.contains("CallSid=CA-X"));
    }

    // ── signature verification ───────────────────────────────

    #[test]
    fn compute_twilio_signature_is_deterministic() {
        let form = vec![
            ("Body".into(), "hello world".into()),
            ("From".into(), "+34600000001".into()),
        ];
        let a = compute_twilio_signature("token", "https://e.com/x", &form);
        let b = compute_twilio_signature("token", "https://e.com/x", &form);
        assert_eq!(a, b);
    }

    #[test]
    fn compute_twilio_signature_sorts_form_keys() {
        // Reordering inputs must produce the same signature.
        let a = compute_twilio_signature(
            "tok",
            "https://e.com/x",
            &[
                ("Body".into(), "x".into()),
                ("From".into(), "+1".into()),
            ],
        );
        let b = compute_twilio_signature(
            "tok",
            "https://e.com/x",
            &[
                ("From".into(), "+1".into()),
                ("Body".into(), "x".into()),
            ],
        );
        assert_eq!(a, b);
    }

    #[test]
    fn compute_twilio_signature_changes_with_body() {
        let a = compute_twilio_signature(
            "tok",
            "https://e.com/x",
            &[("Body".into(), "one".into())],
        );
        let b = compute_twilio_signature(
            "tok",
            "https://e.com/x",
            &[("Body".into(), "two".into())],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn signature_eq_constant_time() {
        assert!(signature_eq("abc", "abc"));
        assert!(!signature_eq("abc", "abd"));
        assert!(!signature_eq("ab", "abc"));
    }

    // ── frame mapping ────────────────────────────────────────

    #[test]
    fn build_inbound_frame_populates_call_sid_and_recording_url() {
        let f = build_inbound_frame_from_recording(
            &ctx(),
            "ACdeadbeef",
            "CA-1",
            "+34600000001",
            "hello bot".into(),
            "https://api.twilio.com/.../Recordings/RE-1.wav",
        );
        assert_eq!(f.account_id, "ACdeadbeef");
        assert_eq!(f.message_id, "CA-1");
        assert_eq!(f.sender_id, "+34600000001");
        assert_eq!(f.text, "hello bot");
        assert!(f
            .raw_metadata
            .get("recording_url")
            .and_then(|v| v.as_str())
            .unwrap()
            .ends_with("RE-1.wav"));
        assert_eq!(
            f.raw_metadata
                .get("twilio_call_sid")
                .and_then(|v| v.as_str()),
            Some("CA-1")
        );
    }
}
