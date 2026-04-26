//! Phase 11 — Web chat WS upgrade handler.
//!
//! Mounts at `/transport/<slot_uuid>/<transport_uuid>/ws` via the
//! WebhookRouter. Verifies the request Origin header against the
//! configured allowlist (with a loopback-dev exception when
//! `production_mode = false`), accepts or issues a HMAC-SHA256
//! visitor cookie, then performs the WS upgrade and pumps frames
//! between the client and the per-visitor outbound queue on the
//! [`WebChatAdapter`].
//!
//! Inbound wire shape (client → server):
//!   `{"type":"msg","text":"..."}`
//!   `{"type":"typing"}`         (ignored by the gateway)
//!
//! Outbound wire shape (server → client):
//!   `{"type":"msg","text":"...","ts":"<rfc3339>"}`

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        FromRequestParts, Request,
    },
    http::{header, HeaderValue, Response as AxumResponse, StatusCode},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use makakoo_core::transport::frame::MakakooInboundFrame;
use makakoo_core::transport::gateway::InboundSink;
use makakoo_core::transport::web::{
    check_origin, sign_cookie, verify_cookie, OriginCheck, VisitorCookie, WebChatAdapter,
    COOKIE_NAME,
};

use crate::webhook_router::{VerifyError, WsUpgradeHandler};

pub struct WebChatWsHandler {
    pub adapter: Arc<WebChatAdapter>,
    pub sink: InboundSink,
}

impl WebChatWsHandler {
    pub fn new(adapter: Arc<WebChatAdapter>, sink: InboundSink) -> Self {
        Self { adapter, sink }
    }

    fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Read the visitor cookie from the request, or generate a new
    /// one. Returns `(VisitorCookie, set_cookie_value_if_new)`.
    fn cookie_from_or_new(
        &self,
        req: &Request,
        accept_dev: bool,
    ) -> (VisitorCookie, Option<String>) {
        let now = self.now_unix();
        let ttl = self.adapter.config.cookie_ttl_seconds;
        if let Some(raw) = read_cookie(req, COOKIE_NAME) {
            if let Ok(cookie) = verify_cookie(&self.adapter.cookie_key, &raw, now) {
                return (cookie, None);
            }
        }
        // Mint a fresh cookie.
        let id = new_visitor_id();
        let cookie = VisitorCookie {
            visitor_id: id,
            expires_at: now + ttl,
        };
        let value = sign_cookie(&self.adapter.cookie_key, &cookie);
        let secure_flag = if accept_dev { "" } else { "; Secure" };
        let set_cookie = format!(
            "{COOKIE_NAME}={value}; Path=/; HttpOnly; SameSite=Lax{secure_flag}; Max-Age={ttl}"
        );
        (cookie, Some(set_cookie))
    }
}

/// Tag a fresh visitor id (16 hex bytes).
fn new_visitor_id() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Pull the named cookie out of the Cookie request header.
fn read_cookie(req: &Request, name: &str) -> Option<String> {
    let header = req.headers().get(header::COOKIE)?.to_str().ok()?;
    for pair in header.split(';') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k.trim() == name {
            return Some(v.trim().to_string());
        }
    }
    None
}

#[async_trait]
impl WsUpgradeHandler for WebChatWsHandler {
    fn verify_upgrade(&self, req: &Request) -> Result<(), VerifyError> {
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok());
        match check_origin(&self.adapter.config, origin) {
            OriginCheck::Reject => Err(VerifyError::BadOrigin),
            OriginCheck::AcceptDev | OriginCheck::AcceptProd => {
                // Cookie verification happens at on_upgrade so we
                // can mint one on first connect (verify_upgrade has
                // no Set-Cookie capability). Stale/invalid cookies
                // are simply replaced.
                Ok(())
            }
        }
    }

    async fn on_upgrade(&self, req: Request) -> Response {
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let accept_dev = matches!(
            check_origin(&self.adapter.config, origin.as_deref()),
            OriginCheck::AcceptDev
        );
        let (cookie, set_cookie) = self.cookie_from_or_new(&req, accept_dev);

        let (mut parts, body) = req.into_parts();
        let upgrade: WebSocketUpgrade =
            match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
                Ok(u) => u,
                Err(_) => {
                    drop(body);
                    return error_response(StatusCode::BAD_REQUEST, "WS upgrade failed");
                }
            };

        let adapter = self.adapter.clone();
        let sink = self.sink.clone();
        let visitor_id = cookie.visitor_id.clone();

        let mut response = upgrade.on_upgrade(move |ws| async move {
            run_ws_loop(adapter, sink, visitor_id, ws).await;
        });

        if let Some(cookie_header) = set_cookie {
            if let Ok(v) = HeaderValue::from_str(&cookie_header) {
                response.headers_mut().insert(header::SET_COOKIE, v);
            }
        }
        response
    }
}

/// WS message-pump: drain client→server text frames into the inbound
/// sink, drain per-visitor outbound queue out to the WS, until either
/// side closes.
async fn run_ws_loop(
    adapter: Arc<WebChatAdapter>,
    sink: InboundSink,
    visitor_id: String,
    socket: WebSocket,
) {
    let mut outbound_rx = adapter.register_sink(&visitor_id).await;
    let (mut ws_tx, mut ws_rx) = socket.split();

    let visitor_for_inbound = visitor_id.clone();
    let adapter_for_drop = adapter.clone();
    let inbound = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            let text = match msg {
                WsMessage::Text(t) => t,
                WsMessage::Close(_) => break,
                WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Binary(_) => continue,
            };
            let parsed: Result<ClientFrame, _> = serde_json::from_str(&text);
            let Ok(frame_in) = parsed else { continue };
            match frame_in.kind.as_str() {
                "msg" => {
                    let body = frame_in.text.unwrap_or_default();
                    let inbound_frame = MakakooInboundFrame {
                        agent_slot_id: adapter_for_drop.ctx.slot_id.clone(),
                        transport_id: adapter_for_drop.ctx.transport_id.clone(),
                        transport_kind: "web".into(),
                        account_id: format!("web-{}", adapter_for_drop.ctx.transport_id),
                        conversation_id: visitor_for_inbound.clone(),
                        sender_id: visitor_for_inbound.clone(),
                        thread_id: None,
                        thread_kind: None,
                        message_id: format!("{}-{}", visitor_for_inbound, ulid_like()),
                        text: body,
                        transport_timestamp: None,
                        received_at: chrono::Utc::now(),
                        raw_metadata: Default::default(),
                    };
                    if sink.send(inbound_frame).await.is_err() {
                        break;
                    }
                }
                "typing" => {
                    // Future: surface typing state to the gateway.
                    // For now, no-op.
                }
                _ => {
                    // Unknown event; drop silently.
                }
            }
        }
        adapter_for_drop.drop_sink(&visitor_for_inbound).await;
    });

    let outbound = tokio::spawn(async move {
        while let Some(out) = outbound_rx.recv().await {
            let payload = ServerFrame {
                kind: "msg",
                text: out.text,
                ts: chrono::Utc::now().to_rfc3339(),
            };
            let body = match serde_json::to_string(&payload) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if ws_tx.send(WsMessage::Text(body)).await.is_err() {
                break;
            }
        }
    });

    let _ = inbound.await;
    let _ = outbound.await;
}

fn ulid_like() -> String {
    let mut bytes = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[derive(Deserialize)]
struct ClientFrame {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Serialize)]
struct ServerFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
    ts: String,
}

fn error_response(status: StatusCode, msg: &str) -> Response {
    AxumResponse::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .body(axum::body::Body::from(msg.to_string()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, Method, Request as HttpRequest};
    use makakoo_core::transport::config::WebConfig;
    use makakoo_core::transport::TransportContext;
    use std::sync::Arc;

    fn adapter(cfg: WebConfig) -> Arc<WebChatAdapter> {
        Arc::new(WebChatAdapter::new(
            TransportContext {
                slot_id: "secretary".into(),
                transport_id: "web-main".into(),
            },
            cfg,
            b"a-32-byte-test-key-of-fixed-shape".to_vec(),
        ))
    }

    fn handler(cfg: WebConfig) -> (WebChatWsHandler, mpsc::Sender<MakakooInboundFrame>) {
        let (tx, _rx) = mpsc::channel(4);
        let h = WebChatWsHandler::new(adapter(cfg), tx.clone());
        (h, tx)
    }

    fn req_with_origin(origin: Option<&str>, cookie: Option<&str>) -> Request {
        let mut builder = HttpRequest::builder().method(Method::GET).uri("/x");
        if let Some(o) = origin {
            builder = builder.header(header::ORIGIN, o);
        }
        if let Some(c) = cookie {
            builder = builder.header(header::COOKIE, c);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn verify_upgrade_accepts_loopback_in_dev_mode() {
        let (h, _) = handler(WebConfig {
            allowed_origins: vec![],
            production_mode: false,
            cookie_ttl_seconds: 100,
        });
        let req = req_with_origin(Some("http://localhost:5173"), None);
        h.verify_upgrade(&req).unwrap();
    }

    #[test]
    fn verify_upgrade_rejects_unknown_origin_in_production() {
        let (h, _) = handler(WebConfig {
            allowed_origins: vec!["https://harvey.example".into()],
            production_mode: true,
            cookie_ttl_seconds: 100,
        });
        let req = req_with_origin(Some("https://attacker.example"), None);
        let err = h.verify_upgrade(&req).unwrap_err();
        assert!(matches!(err, VerifyError::BadOrigin));
    }

    #[test]
    fn verify_upgrade_accepts_allowlisted_origin() {
        let (h, _) = handler(WebConfig {
            allowed_origins: vec!["https://harvey.example".into()],
            production_mode: true,
            cookie_ttl_seconds: 100,
        });
        let req = req_with_origin(Some("https://harvey.example"), None);
        h.verify_upgrade(&req).unwrap();
    }

    #[test]
    fn read_cookie_picks_named_cookie() {
        let req = req_with_origin(
            Some("http://localhost"),
            Some(&format!("session=foo; {COOKIE_NAME}=v.1.sig; theme=dark")),
        );
        let v = read_cookie(&req, COOKIE_NAME).unwrap();
        assert_eq!(v, "v.1.sig");
    }

    #[test]
    fn read_cookie_returns_none_when_absent() {
        let req = req_with_origin(Some("http://localhost"), Some("only=other"));
        assert!(read_cookie(&req, COOKIE_NAME).is_none());
    }

    #[test]
    fn cookie_from_or_new_reuses_valid_cookie() {
        let (h, _) = handler(WebConfig {
            allowed_origins: vec![],
            production_mode: false,
            cookie_ttl_seconds: 3600,
        });
        let now = h.now_unix();
        let valid_cookie = VisitorCookie {
            visitor_id: "stable-id".into(),
            expires_at: now + 1000,
        };
        let raw = sign_cookie(&h.adapter.cookie_key, &valid_cookie);
        let cookie_hdr = format!("{COOKIE_NAME}={raw}");
        let req = req_with_origin(Some("http://localhost"), Some(&cookie_hdr));
        let (cookie, set_new) = h.cookie_from_or_new(&req, true);
        assert_eq!(cookie.visitor_id, "stable-id");
        assert!(
            set_new.is_none(),
            "valid cookie must NOT trigger a new Set-Cookie"
        );
    }

    #[test]
    fn cookie_from_or_new_mints_when_missing() {
        let (h, _) = handler(WebConfig {
            allowed_origins: vec![],
            production_mode: false,
            cookie_ttl_seconds: 3600,
        });
        let req = req_with_origin(Some("http://localhost"), None);
        let (_cookie, set_new) = h.cookie_from_or_new(&req, true);
        let header = set_new.expect("must mint a Set-Cookie when missing");
        assert!(header.starts_with(&format!("{COOKIE_NAME}=")));
        // Dev mode → no Secure flag.
        assert!(
            !header.contains("Secure"),
            "loopback-dev must drop Secure flag"
        );
    }

    #[test]
    fn cookie_from_or_new_marks_secure_in_production() {
        let (h, _) = handler(WebConfig {
            allowed_origins: vec!["https://harvey.example".into()],
            production_mode: true,
            cookie_ttl_seconds: 3600,
        });
        let req = req_with_origin(Some("https://harvey.example"), None);
        let (_cookie, set_new) = h.cookie_from_or_new(&req, false);
        let header = set_new.unwrap();
        assert!(header.contains("Secure"), "production must mark Secure");
    }

    #[test]
    fn cookie_from_or_new_replaces_expired_cookie() {
        let (h, _) = handler(WebConfig {
            allowed_origins: vec![],
            production_mode: false,
            cookie_ttl_seconds: 3600,
        });
        let expired_cookie = VisitorCookie {
            visitor_id: "old".into(),
            expires_at: 1, // long-ago
        };
        let raw = sign_cookie(&h.adapter.cookie_key, &expired_cookie);
        let cookie_hdr = format!("{COOKIE_NAME}={raw}");
        let req = req_with_origin(Some("http://localhost"), Some(&cookie_hdr));
        let (cookie, set_new) = h.cookie_from_or_new(&req, true);
        assert_ne!(cookie.visitor_id, "old", "expired cookie must be replaced");
        assert!(set_new.is_some(), "expired cookie must trigger Set-Cookie");
    }

    #[test]
    fn cookie_from_or_new_replaces_bad_signature() {
        let (h, _) = handler(WebConfig::default());
        let cookie_hdr = format!("{COOKIE_NAME}=v.999999999.deadbeef");
        let req = req_with_origin(Some("http://localhost"), Some(&cookie_hdr));
        let (_cookie, set_new) = h.cookie_from_or_new(&req, true);
        assert!(set_new.is_some(), "bad-sig cookie must be replaced");
    }

    #[test]
    fn new_visitor_id_is_unique_each_call() {
        let a = new_visitor_id();
        let b = new_visitor_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 16); // 8 bytes hex
    }

}
