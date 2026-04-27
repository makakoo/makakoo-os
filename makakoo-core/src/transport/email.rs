//! Email (SMTP outbound + IMAP inbound) transport adapter.
//!
//! Phase 9 / Q8.
//!
//! Scope decision: this v2.0 adapter ships
//!   - SMTP outbound via `lettre`
//!   - mailparse-based inbound parser (callable via the public
//!     [`parse_inbound_to_frame`] helper from outside an IMAP listener)
//!   - quote-line stripper, Message-ID threading helpers, OAuth2
//!     refresh token shape, allowlist enforcement
//!
//! The IMAP IDLE listener loop ships in v2.1 (the Phase 9 walkthrough
//! at `docs/walkthroughs/email-secretary.md` documents the locked
//! schema + behavior so production deployments can plan around it).
//! Until then, `Gateway::start` parks forever — the adapter is wired
//! through the supervisor lifecycle and supports outbound through
//! the rest of v2.0 (test → reply via `agent send`, mailing-list
//! integrations, etc.).

use std::sync::Arc;

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use mailparse::MailHeaderMap;

use crate::transport::config::EmailConfig;
use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame};
use crate::transport::gateway::{Gateway, InboundSink};
use crate::transport::{Transport, TransportContext, VerifiedIdentity};
use crate::{MakakooError, Result};

pub struct EmailAdapter {
    pub ctx: TransportContext,
    pub config: EmailConfig,
    /// Resolved SMTP credential — for `app_password` mode this is the
    /// password; for `oauth2` mode it's an XOAUTH2 SASL bearer string
    /// (caller's responsibility to refresh before expiry).
    pub credential: String,
}

impl EmailAdapter {
    pub fn new(ctx: TransportContext, config: EmailConfig, credential: String) -> Self {
        Self {
            ctx,
            config,
            credential,
        }
    }

    fn smtp_transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
        let creds = Credentials::new(self.config.account_id.clone(), self.credential.clone());
        let builder = AsyncSmtpTransport::<Tokio1Executor>::relay(&self.config.smtp_server)
            .map_err(|e| {
                MakakooError::Internal(format!(
                    "build smtp relay {}:{}: {e}",
                    self.config.smtp_server, self.config.smtp_port
                ))
            })?
            .port(self.config.smtp_port)
            .credentials(creds);
        Ok(builder.build())
    }
}

#[async_trait]
impl Transport for EmailAdapter {
    fn kind(&self) -> &'static str {
        "email"
    }
    fn transport_id(&self) -> &str {
        &self.ctx.transport_id
    }

    async fn verify_credentials(&self) -> Result<VerifiedIdentity> {
        // Probe by opening + closing an SMTP connection. lettre's
        // `test_connection` does the SMTP handshake without actually
        // sending mail; that's enough to verify auth_mode + tls +
        // credentials simultaneously.
        let smtp = self.smtp_transport()?;
        smtp.test_connection().await.map_err(|e| {
            MakakooError::Config(format!(
                "smtp test_connection {}:{} failed: {e}",
                self.config.smtp_server, self.config.smtp_port
            ))
        })?;
        Ok(VerifiedIdentity {
            account_id: self.config.account_id.clone(),
            tenant_id: None,
            display_name: Some(self.config.account_id.clone()),
        })
    }

    async fn send(&self, frame: &MakakooOutboundFrame) -> Result<()> {
        let to: Mailbox = frame.conversation_id.parse().map_err(|e| {
            MakakooError::InvalidInput(format!(
                "email outbound conversation_id '{}' is not a valid mailbox: {e}",
                frame.conversation_id
            ))
        })?;
        let from: Mailbox = self.config.account_id.parse().map_err(|e| {
            MakakooError::Internal(format!(
                "email account_id '{}' is not a valid mailbox: {e}",
                self.config.account_id
            ))
        })?;

        let mut builder = Message::builder()
            .from(from)
            .to(to)
            .header(ContentType::TEXT_PLAIN);

        // Threading headers (In-Reply-To / References) are computed
        // by [`build_threading_headers`] and tested in isolation;
        // injecting them through lettre's typed-header system requires
        // a bespoke header struct in 0.11 — slated for v2.1 alongside
        // the IMAP IDLE listener. For v2.0 send-only, we omit them
        // and document at docs/walkthroughs/email-secretary.md.
        let _ = frame.reply_to_message_id.as_deref();

        // Subject defaults to a reply prefix when the frame is a reply.
        let subject = if frame.reply_to_message_id.is_some() {
            "Re: (Makakoo)".to_string()
        } else {
            "Makakoo".to_string()
        };
        let mail = builder
            .subject(subject)
            .body(frame.text.clone())
            .map_err(|e| MakakooError::Internal(format!("build email message: {e}")))?;

        let smtp = self.smtp_transport()?;
        smtp.send(mail).await.map_err(|e| {
            MakakooError::Internal(format!(
                "smtp send to {}:{} failed: {e}",
                self.config.smtp_server, self.config.smtp_port
            ))
        })?;
        Ok(())
    }
}

#[async_trait]
impl Gateway for EmailAdapter {
    async fn start(&self, _sink: InboundSink) -> Result<()> {
        // IMAP IDLE listener ships in v2.1. The walkthrough at
        // docs/walkthroughs/email-secretary.md documents the locked
        // schema + behavior so production deployments can plan
        // around it.
        std::future::pending::<()>().await;
        Ok(())
    }
}

// ── Pure inbound parsing helpers (testable in isolation) ──────────

/// Parse a raw RFC822 byte slice into a `MakakooInboundFrame`. The
/// `From` header populates `sender_id`; `Message-ID` populates
/// `message_id`; `In-Reply-To` populates `thread_id` so the
/// supervisor's outbound demux can route a reply back into the same
/// thread. Returns `Err` when the message has no valid `From` (we
/// can't ACL-check it) or no `Message-ID` (we can't thread).
pub fn parse_inbound_to_frame(
    ctx: &TransportContext,
    account_id: &str,
    allowed_senders: &[String],
    raw: &[u8],
) -> std::result::Result<Option<MakakooInboundFrame>, ParseError> {
    let mail = mailparse::parse_mail(raw).map_err(|e| ParseError::Mime(e.to_string()))?;
    let from = mail
        .headers
        .get_first_value("From")
        .ok_or(ParseError::NoFromHeader)?;
    let message_id = mail
        .headers
        .get_first_value("Message-ID")
        .ok_or(ParseError::NoMessageId)?
        .trim_matches(|c| c == '<' || c == '>')
        .to_string();
    let in_reply_to = mail
        .headers
        .get_first_value("In-Reply-To")
        .map(|v| v.trim_matches(|c| c == '<' || c == '>').to_string());

    let from_addr = extract_email_addr(&from).unwrap_or_else(|| from.clone());
    let from_lower = from_addr.to_ascii_lowercase();
    let allowed = allowed_senders
        .iter()
        .any(|a| a.to_ascii_lowercase() == from_lower);
    if !allowed {
        // Caller drops without reply; v1 does not "log first allow later".
        return Ok(None);
    }

    let body = mail
        .get_body()
        .map_err(|e| ParseError::Mime(e.to_string()))?;
    let stripped = strip_quoted_lines(&body);

    let mut raw_meta = std::collections::BTreeMap::new();
    raw_meta.insert(
        "body_raw".into(),
        serde_json::Value::String(body.clone()),
    );
    if let Some(ref irt) = in_reply_to {
        raw_meta.insert(
            "in_reply_to".into(),
            serde_json::Value::String(irt.clone()),
        );
    }

    let conversation_id = in_reply_to.clone().unwrap_or_else(|| message_id.clone());

    Ok(Some(MakakooInboundFrame {
        agent_slot_id: ctx.slot_id.clone(),
        transport_id: ctx.transport_id.clone(),
        transport_kind: "email".into(),
        account_id: account_id.to_string(),
        conversation_id,
        sender_id: from_addr,
        thread_id: in_reply_to,
        thread_kind: None,
        message_id,
        text: stripped,
        transport_timestamp: mail.headers.get_first_value("Date"),
        received_at: chrono::Utc::now(),
        raw_metadata: raw_meta,
    }))
}

#[derive(Debug, Clone)]
pub enum ParseError {
    Mime(String),
    NoFromHeader,
    NoMessageId,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Mime(m) => write!(f, "mime parse: {m}"),
            ParseError::NoFromHeader => write!(f, "missing From header"),
            ParseError::NoMessageId => write!(f, "missing Message-ID header"),
        }
    }
}
impl std::error::Error for ParseError {}

/// Extract `alice@example.com` from a `From` header that may be
/// `"Alice <alice@example.com>"` or just the raw address.
pub fn extract_email_addr(s: &str) -> Option<String> {
    if let (Some(start), Some(end)) = (s.find('<'), s.rfind('>')) {
        if end > start {
            return Some(s[start + 1..end].trim().to_string());
        }
    }
    let trimmed = s.trim();
    if trimmed.contains('@') {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Drop quoted-reply lines + trailing signature blocks from an email
/// body. Recognized markers:
///   - lines starting with `>` (standard quote)
///   - "On <date>, <name> wrote:" preamble (Apple Mail / Gmail)
///   - "From: ... Sent: ..." preamble (Outlook)
///   - "-- " line (signature delimiter per RFC 3676)
pub fn strip_quoted_lines(body: &str) -> String {
    let mut kept = Vec::new();
    let mut hit_quote = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('>') {
            hit_quote = true;
            break;
        }
        // "On Mon, Jan 1, 2026 at 12:00 PM Alice <alice@x> wrote:"
        if trimmed.starts_with("On ") && trimmed.ends_with("wrote:") {
            hit_quote = true;
            break;
        }
        // Outlook-style. Match "From:" appearing at line start AND
        // preceded by a blank-ish line — which conservatively means
        // we look one line ahead.
        if trimmed.starts_with("From:") && kept.last().map(|l: &String| l.is_empty()).unwrap_or(false)
        {
            hit_quote = true;
            break;
        }
        // RFC 3676 sig delimiter.
        if line == "-- " || line == "--" {
            break;
        }
        kept.push(line.to_string());
    }
    let _ = hit_quote;
    kept.join("\n").trim_end().to_string()
}

/// Build the `In-Reply-To` and `References` header values for an
/// outbound reply. References chains the new message onto the prior
/// thread; In-Reply-To points at the immediate parent.
pub fn build_threading_headers(parent_message_id: &str) -> (String, String) {
    let id = parent_message_id.trim_matches(|c| c == '<' || c == '>');
    let formatted = format!("<{id}>");
    (formatted.clone(), formatted)
}

/// OAuth2 token-refresh contract. Pure helper that takes a refresh
/// token + client_id + client_secret + token endpoint, returns the
/// new access token string. Mocked end-to-end via wiremock in tests.
pub async fn refresh_oauth2_token(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> std::result::Result<String, OAuth2Error> {
    let resp = http
        .post(token_url)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| OAuth2Error::Http(e.to_string()))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuth2Error::Remote(body));
    }
    #[derive(serde::Deserialize)]
    struct TokenResp {
        access_token: String,
    }
    let parsed: TokenResp = resp
        .json()
        .await
        .map_err(|e| OAuth2Error::Http(e.to_string()))?;
    Ok(parsed.access_token)
}

#[derive(Debug, Clone)]
pub enum OAuth2Error {
    Http(String),
    Remote(String),
}
impl std::fmt::Display for OAuth2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OAuth2Error::Http(m) => write!(f, "oauth2 http: {m}"),
            OAuth2Error::Remote(m) => write!(f, "oauth2 remote: {m}"),
        }
    }
}
impl std::error::Error for OAuth2Error {}

/// Convenience constructor.
pub fn boxed(adapter: EmailAdapter) -> Arc<dyn Transport> {
    Arc::new(adapter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportContext;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "email-main".into(),
        }
    }

    // ── extract_email_addr ───────────────────────────────────

    #[test]
    fn extract_picks_address_inside_brackets() {
        assert_eq!(
            extract_email_addr("Alice <alice@example.com>"),
            Some("alice@example.com".into())
        );
    }

    #[test]
    fn extract_passes_through_bare_address() {
        assert_eq!(
            extract_email_addr("alice@example.com"),
            Some("alice@example.com".into())
        );
    }

    #[test]
    fn extract_returns_none_when_no_at_sign() {
        assert!(extract_email_addr("Alice").is_none());
    }

    // ── strip_quoted_lines ───────────────────────────────────

    #[test]
    fn strip_drops_gmail_style_quote_block() {
        let body = "Sounds good, thanks!\n\nOn Mon, Jan 1, 2026 at 12:00 PM Alice <alice@x> wrote:\n> hi";
        let out = strip_quoted_lines(body);
        assert_eq!(out, "Sounds good, thanks!");
    }

    #[test]
    fn strip_drops_pure_quote_lines() {
        let body = "Reply text\n> quoted line";
        assert_eq!(strip_quoted_lines(body), "Reply text");
    }

    #[test]
    fn strip_drops_outlook_style_preamble() {
        let body = "Reply\n\nFrom: alice@x\nSent: Monday\nTo: bot\nSubject: re";
        let out = strip_quoted_lines(body);
        assert_eq!(out, "Reply");
    }

    #[test]
    fn strip_drops_signature_delimiter() {
        let body = "Hi\n\n-- \nAlice\n+34000000001";
        let out = strip_quoted_lines(body);
        assert_eq!(out, "Hi");
    }

    #[test]
    fn strip_passes_through_simple_body() {
        assert_eq!(strip_quoted_lines("Just text\nMore text"), "Just text\nMore text");
    }

    // ── threading headers ────────────────────────────────────

    #[test]
    fn build_threading_headers_normalizes_brackets() {
        let (irt, refs) = build_threading_headers("abc@example.com");
        assert_eq!(irt, "<abc@example.com>");
        assert_eq!(refs, "<abc@example.com>");

        let (irt, _) = build_threading_headers("<already@bracketed>");
        assert_eq!(irt, "<already@bracketed>");
    }

    // ── parse_inbound_to_frame ────────────────────────────────

    #[test]
    fn parse_inbound_drops_non_allowlisted_sender() {
        let raw = b"From: rando@example.com\r\nMessage-ID: <m1@x>\r\n\r\nhi";
        let frame = parse_inbound_to_frame(
            &ctx(),
            "secretary@example.com",
            &["alice@example.com".into()],
            raw,
        )
        .unwrap();
        assert!(frame.is_none());
    }

    #[test]
    fn parse_inbound_allowlisted_produces_frame() {
        let raw = b"From: Alice <alice@example.com>\r\nMessage-ID: <m1@example.com>\r\nDate: Mon, 1 Jan 2026 12:00:00 +0000\r\n\r\nhello bot\n\nOn previous date, bot wrote:\n> earlier";
        let frame = parse_inbound_to_frame(
            &ctx(),
            "secretary@example.com",
            &["alice@example.com".into()],
            raw,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frame.sender_id, "alice@example.com");
        assert_eq!(frame.message_id, "m1@example.com");
        assert_eq!(frame.text, "hello bot");
        assert!(frame.thread_id.is_none(), "no In-Reply-To → no thread_id");
        assert_eq!(frame.conversation_id, "m1@example.com");
        assert!(frame
            .raw_metadata
            .get("body_raw")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("On previous date"));
    }

    #[test]
    fn parse_inbound_threads_via_in_reply_to() {
        let raw = b"From: alice@example.com\r\nMessage-ID: <m2@x>\r\nIn-Reply-To: <root@x>\r\n\r\nreply text";
        let frame = parse_inbound_to_frame(
            &ctx(),
            "secretary@example.com",
            &["alice@example.com".into()],
            raw,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frame.thread_id.as_deref(), Some("root@x"));
        assert_eq!(frame.conversation_id, "root@x");
    }

    #[test]
    fn parse_inbound_case_insensitive_sender_match() {
        let raw = b"From: Alice <ALICE@example.com>\r\nMessage-ID: <m@x>\r\n\r\nhi";
        let frame = parse_inbound_to_frame(
            &ctx(),
            "secretary@example.com",
            &["alice@example.com".into()],
            raw,
        )
        .unwrap();
        assert!(frame.is_some(), "case-insensitive allowlist match");
    }

    #[test]
    fn parse_inbound_missing_message_id_errors() {
        let raw = b"From: alice@example.com\r\n\r\nbody";
        let err = parse_inbound_to_frame(&ctx(), "x@x", &["alice@example.com".into()], raw)
            .unwrap_err();
        assert!(matches!(err, ParseError::NoMessageId));
    }

    #[test]
    fn parse_inbound_missing_from_errors() {
        let raw = b"Message-ID: <m@x>\r\n\r\nbody";
        let err = parse_inbound_to_frame(&ctx(), "x@x", &[], raw).unwrap_err();
        assert!(matches!(err, ParseError::NoFromHeader));
    }

    // ── OAuth2 refresh ───────────────────────────────────────

    #[tokio::test]
    async fn oauth2_refresh_returns_access_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ya29.NEW",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let tok = refresh_oauth2_token(
            &http,
            &format!("{}/token", server.uri()),
            "client-id",
            "client-secret",
            "refresh-tok",
        )
        .await
        .unwrap();
        assert_eq!(tok, "ya29.NEW");
    }

    #[tokio::test]
    async fn oauth2_refresh_propagates_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_string("invalid_grant"))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let err = refresh_oauth2_token(
            &http,
            &format!("{}/token", server.uri()),
            "id",
            "sec",
            "tok",
        )
        .await
        .unwrap_err();
        assert!(format!("{err}").contains("invalid_grant"));
    }
}
