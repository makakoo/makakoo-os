//! Web chat transport — embeddable WS-backed widget.
//!
//! Phase 11 / Q10. Visitors identify via an HMAC-SHA256 signed
//! cookie issued at first WS upgrade; a per-conversation outbound
//! queue lets the LLM `send` reach the connected client without
//! holding a direct socket reference.
//!
//! Locked by Q10 (round-2):
//!   - HMAC-SHA256 cookie format: `<visitor_id>.<exp>.<sig_hex>` where
//!     `sig = HMAC(key, "<visitor_id>.<exp>")`.
//!   - Key persisted to `$MAKAKOO_HOME/keys/web-chat-hmac` mode 0600,
//!     auto-generated on first run if missing (32 random bytes).
//!   - Origin allowlist REQUIRED in production. Loopback origins
//!     accepted only when the WebConfig's `production_mode = false`.
//!   - Cookie `Secure` flag dropped only on loopback origins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use tokio::sync::{mpsc, Mutex};

use crate::transport::config::WebConfig;
use crate::transport::frame::MakakooOutboundFrame;
use crate::transport::gateway::{Gateway, InboundSink};
use crate::transport::{Transport, TransportContext, VerifiedIdentity};
use crate::{MakakooError, Result};

type HmacSha256 = Hmac<Sha256>;

/// Locked cookie name.
pub const COOKIE_NAME: &str = "makakoo_web_visitor";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitorCookie {
    pub visitor_id: String,
    pub expires_at: u64,
}

/// Encode `<visitor_id>.<exp>.<hex(HMAC-SHA256(key, "<visitor_id>.<exp>"))>`.
pub fn sign_cookie(key: &[u8], cookie: &VisitorCookie) -> String {
    let payload = format!("{}.{}", cookie.visitor_id, cookie.expires_at);
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts arbitrary key length");
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    format!("{payload}.{}", hex::encode(sig))
}

/// Verify shape + signature + expiry. Returns the parsed cookie on
/// success.
pub fn verify_cookie(
    key: &[u8],
    raw: &str,
    now_unix: u64,
) -> std::result::Result<VisitorCookie, CookieError> {
    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() != 3 {
        return Err(CookieError::Malformed);
    }
    let visitor_id = parts[0];
    let exp: u64 = parts[1]
        .parse()
        .map_err(|_| CookieError::Malformed)?;
    let sig = parts[2];
    let recomputed = sign_cookie(
        key,
        &VisitorCookie {
            visitor_id: visitor_id.into(),
            expires_at: exp,
        },
    );
    let recomputed_sig = recomputed.rsplit('.').next().unwrap_or("");
    if !constant_eq(sig, recomputed_sig) {
        return Err(CookieError::BadSig);
    }
    if exp < now_unix {
        return Err(CookieError::Expired);
    }
    Ok(VisitorCookie {
        visitor_id: visitor_id.into(),
        expires_at: exp,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CookieError {
    Malformed,
    BadSig,
    Expired,
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

/// Generate a 32-byte cookie-signing key (256 bits of entropy).
pub fn generate_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    k
}

/// Locked path inside `$MAKAKOO_HOME`.
pub fn cookie_key_path(makakoo_home: &Path) -> PathBuf {
    makakoo_home.join("keys").join("web-chat-hmac")
}

/// Read the key from disk. If missing, generate + persist with mode
/// 0600. Idempotent across restarts — the same key is reused.
pub fn load_or_generate_key(makakoo_home: &Path) -> Result<Vec<u8>> {
    let path = cookie_key_path(makakoo_home);
    if path.exists() {
        let bytes = std::fs::read(&path)
            .map_err(|e| MakakooError::internal(format!("read web cookie key: {e}")))?;
        return Ok(bytes);
    }
    let dir = path
        .parent()
        .ok_or_else(|| MakakooError::internal("cookie key path has no parent"))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| MakakooError::internal(format!("create keys dir: {e}")))?;
    let key = generate_key();
    write_key_with_mode(&path, &key)?;
    Ok(key.to_vec())
}

#[cfg(unix)]
fn write_key_with_mode(path: &Path, key: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| MakakooError::internal(format!("create web cookie key (0600): {e}")))?;
    f.write_all(key)
        .map_err(|e| MakakooError::internal(format!("write web cookie key: {e}")))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_key_with_mode(path: &Path, key: &[u8]) -> Result<()> {
    std::fs::write(path, key)
        .map_err(|e| MakakooError::internal(format!("write web cookie key: {e}")))
}

/// Whether a request Origin is a loopback dev origin (localhost or
/// 127.0.0.1, with or without port). Used to decide:
///   - origin allowlist bypass (when production_mode = false)
///   - whether to drop the `Secure` cookie attribute
pub fn is_loopback_origin(origin: &str) -> bool {
    // Strip scheme.
    let without_scheme = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// Decision returned by `check_origin`. The handler maps these onto
/// HTTP responses + Set-Cookie shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginCheck {
    /// Accept; emit cookies WITHOUT `Secure` (loopback dev).
    AcceptDev,
    /// Accept; emit cookies WITH `Secure`.
    AcceptProd,
    /// Reject — origin not allowlisted in production mode.
    Reject,
}

pub fn check_origin(cfg: &WebConfig, origin: Option<&str>) -> OriginCheck {
    let origin = origin.unwrap_or("");
    if is_loopback_origin(origin) && !cfg.production_mode {
        return OriginCheck::AcceptDev;
    }
    if cfg
        .allowed_origins
        .iter()
        .any(|allowed| allowed == origin)
    {
        return OriginCheck::AcceptProd;
    }
    OriginCheck::Reject
}

// ── Adapter ──────────────────────────────────────────────────────

type ConvSinks = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<MakakooOutboundFrame>>>>;

pub struct WebChatAdapter {
    pub ctx: TransportContext,
    pub config: WebConfig,
    pub cookie_key: Vec<u8>,
    /// Per-visitor outbound channels. The Transport::send call writes
    /// to the queue; the WS upgrade handler drains it for the
    /// connected visitor.
    sinks: ConvSinks,
}

impl WebChatAdapter {
    pub fn new(ctx: TransportContext, config: WebConfig, cookie_key: Vec<u8>) -> Self {
        Self {
            ctx,
            config,
            cookie_key,
            sinks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a new connection sink for `visitor_id`. Returns the
    /// receiver the WS upgrade handler reads from. Replaces any
    /// prior sink (a reconnect closes the old socket cleanly).
    pub async fn register_sink(
        &self,
        visitor_id: &str,
    ) -> mpsc::UnboundedReceiver<MakakooOutboundFrame> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.sinks
            .lock()
            .await
            .insert(visitor_id.to_string(), tx);
        rx
    }

    /// Drop the per-visitor sink (called on WS close).
    pub async fn drop_sink(&self, visitor_id: &str) {
        self.sinks.lock().await.remove(visitor_id);
    }

    pub async fn connected_visitors(&self) -> Vec<String> {
        self.sinks.lock().await.keys().cloned().collect()
    }
}

#[async_trait]
impl Transport for WebChatAdapter {
    fn kind(&self) -> &'static str {
        "web"
    }
    fn transport_id(&self) -> &str {
        &self.ctx.transport_id
    }

    async fn verify_credentials(&self) -> Result<VerifiedIdentity> {
        // Web chat has no upstream provider to authenticate against —
        // the cookie key IS the verification credential. Synthesize a
        // local identity so the lifecycle code can stamp inbound
        // frames with a stable account_id.
        Ok(VerifiedIdentity {
            account_id: format!("web-{}", self.ctx.transport_id),
            tenant_id: None,
            display_name: None,
        })
    }

    async fn send(&self, frame: &MakakooOutboundFrame) -> Result<()> {
        let sinks = self.sinks.lock().await;
        let sink = sinks.get(&frame.conversation_id).ok_or_else(|| {
            MakakooError::InvalidInput(format!(
                "web chat visitor '{}' is not connected",
                frame.conversation_id
            ))
        })?;
        sink.send(frame.clone()).map_err(|_| {
            MakakooError::Internal(format!(
                "web chat visitor '{}' channel closed mid-send",
                frame.conversation_id
            ))
        })?;
        Ok(())
    }
}

/// Web chat is WS-driven — the inbound path is the WS upgrade
/// handler. Gateway::start parks forever to satisfy the trait.
#[async_trait]
impl Gateway for WebChatAdapter {
    async fn start(&self, _sink: InboundSink) -> Result<()> {
        std::future::pending::<()>().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn key() -> Vec<u8> {
        b"a-32-byte-test-key-of-fixed-shape".to_vec()
    }

    // ── cookie sign/verify ────────────────────────────────────

    #[test]
    fn cookie_round_trip_succeeds() {
        let c = VisitorCookie {
            visitor_id: "v-1".into(),
            expires_at: 1_700_000_000,
        };
        let raw = sign_cookie(&key(), &c);
        let parsed = verify_cookie(&key(), &raw, 1_699_999_000).unwrap();
        assert_eq!(parsed.visitor_id, "v-1");
        assert_eq!(parsed.expires_at, 1_700_000_000);
    }

    #[test]
    fn cookie_signed_with_other_key_fails() {
        let c = VisitorCookie {
            visitor_id: "v-1".into(),
            expires_at: 1_700_000_000,
        };
        let raw = sign_cookie(&key(), &c);
        let other = b"different-key-of-different-shape";
        let err = verify_cookie(other, &raw, 1_699_999_000).unwrap_err();
        assert_eq!(err, CookieError::BadSig);
    }

    #[test]
    fn cookie_expired_is_rejected() {
        let c = VisitorCookie {
            visitor_id: "v-1".into(),
            expires_at: 1_000,
        };
        let raw = sign_cookie(&key(), &c);
        let err = verify_cookie(&key(), &raw, 9_999).unwrap_err();
        assert_eq!(err, CookieError::Expired);
    }

    #[test]
    fn cookie_malformed_is_rejected() {
        let err = verify_cookie(&key(), "not-a-cookie", 1).unwrap_err();
        assert_eq!(err, CookieError::Malformed);
        let err = verify_cookie(&key(), "x.notnum.sig", 1).unwrap_err();
        assert_eq!(err, CookieError::Malformed);
    }

    #[test]
    fn cookie_tampered_visitor_id_fails() {
        let c = VisitorCookie {
            visitor_id: "v-1".into(),
            expires_at: 1_700_000_000,
        };
        let raw = sign_cookie(&key(), &c);
        // Swap the visitor_id; the signature is over the original
        // payload so the recomputed signature won't match.
        let parts: Vec<&str> = raw.split('.').collect();
        let tampered = format!("v-evil.{}.{}", parts[1], parts[2]);
        let err = verify_cookie(&key(), &tampered, 1_699_999_000).unwrap_err();
        assert_eq!(err, CookieError::BadSig);
    }

    // ── key persistence ───────────────────────────────────────

    #[test]
    fn key_generates_on_first_call_and_reuses_after() {
        let dir = TempDir::new().unwrap();
        let first = load_or_generate_key(dir.path()).unwrap();
        let second = load_or_generate_key(dir.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 32);
    }

    #[cfg(unix)]
    #[test]
    fn key_file_lands_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let _ = load_or_generate_key(dir.path()).unwrap();
        let path = cookie_key_path(dir.path());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key file must be 0600");
    }

    // ── origin checks ─────────────────────────────────────────

    #[test]
    fn origin_loopback_accepted_in_dev_mode() {
        let cfg = WebConfig {
            allowed_origins: vec![],
            production_mode: false,
            cookie_ttl_seconds: 30,
        };
        assert_eq!(
            check_origin(&cfg, Some("http://localhost:5173")),
            OriginCheck::AcceptDev
        );
    }

    #[test]
    fn origin_loopback_rejected_in_production_mode_when_not_allowlisted() {
        let cfg = WebConfig {
            allowed_origins: vec![],
            production_mode: true,
            cookie_ttl_seconds: 30,
        };
        assert_eq!(
            check_origin(&cfg, Some("http://localhost:5173")),
            OriginCheck::Reject
        );
    }

    #[test]
    fn origin_explicit_allowlist_match_accepted_with_secure() {
        let cfg = WebConfig {
            allowed_origins: vec!["https://harvey.example".into()],
            production_mode: true,
            cookie_ttl_seconds: 30,
        };
        assert_eq!(
            check_origin(&cfg, Some("https://harvey.example")),
            OriginCheck::AcceptProd
        );
    }

    #[test]
    fn origin_unknown_rejected() {
        let cfg = WebConfig {
            allowed_origins: vec!["https://harvey.example".into()],
            production_mode: true,
            cookie_ttl_seconds: 30,
        };
        assert_eq!(
            check_origin(&cfg, Some("https://attacker.example")),
            OriginCheck::Reject
        );
    }

    #[test]
    fn is_loopback_origin_recognizes_common_dev_shapes() {
        assert!(is_loopback_origin("http://localhost"));
        assert!(is_loopback_origin("http://localhost:5173"));
        assert!(is_loopback_origin("http://127.0.0.1:8080"));
        assert!(is_loopback_origin("https://localhost:3000"));
        assert!(!is_loopback_origin("https://harvey.example"));
        assert!(!is_loopback_origin("http://attacker.localhost.evil"));
    }

    // ── adapter routing ───────────────────────────────────────

    #[tokio::test]
    async fn send_to_unconnected_visitor_returns_invalid_input() {
        let adapter = WebChatAdapter::new(
            TransportContext {
                slot_id: "secretary".into(),
                transport_id: "web-main".into(),
            },
            WebConfig::default(),
            key(),
        );
        let frame = MakakooOutboundFrame {
            transport_id: "web-main".into(),
            transport_kind: "web".into(),
            conversation_id: "v-99".into(),
            thread_id: None,
            thread_kind: None,
            text: "hi".into(),
            reply_to_message_id: None,
        };
        let err = adapter.send(&frame).await.unwrap_err();
        assert!(format!("{err}").contains("not connected"));
    }

    #[tokio::test]
    async fn send_to_registered_visitor_delivers_via_channel() {
        let adapter = WebChatAdapter::new(
            TransportContext {
                slot_id: "secretary".into(),
                transport_id: "web-main".into(),
            },
            WebConfig::default(),
            key(),
        );
        let mut rx = adapter.register_sink("v-1").await;
        let frame = MakakooOutboundFrame {
            transport_id: "web-main".into(),
            transport_kind: "web".into(),
            conversation_id: "v-1".into(),
            thread_id: None,
            thread_kind: None,
            text: "hello visitor".into(),
            reply_to_message_id: None,
        };
        adapter.send(&frame).await.unwrap();
        let recv = rx.recv().await.unwrap();
        assert_eq!(recv.text, "hello visitor");
    }

    #[tokio::test]
    async fn drop_sink_removes_visitor() {
        let adapter = WebChatAdapter::new(
            TransportContext {
                slot_id: "secretary".into(),
                transport_id: "web-main".into(),
            },
            WebConfig::default(),
            key(),
        );
        let _rx = adapter.register_sink("v-1").await;
        assert_eq!(adapter.connected_visitors().await, vec!["v-1".to_string()]);
        adapter.drop_sink("v-1").await;
        assert!(adapter.connected_visitors().await.is_empty());
    }

    // ── verify_credentials shape ──────────────────────────────

    #[tokio::test]
    async fn verify_credentials_synthesizes_local_identity() {
        let adapter = WebChatAdapter::new(
            TransportContext {
                slot_id: "secretary".into(),
                transport_id: "web-main".into(),
            },
            WebConfig::default(),
            key(),
        );
        let id = adapter.verify_credentials().await.unwrap();
        assert_eq!(id.account_id, "web-web-main");
    }
}
