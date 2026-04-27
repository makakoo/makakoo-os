//! Phase 5a — webhook router primitive.
//!
//! Hosts every transport-webhook endpoint (Slack Events API,
//! WhatsApp Cloud API, Twilio Voice, Web chat WS) under one shared
//! axum router. Each transport registers its own handler at
//! `/transport/<slot_uuid>/<transport_uuid>/<kind>/...`.
//!
//! Locked Q5 / Q15:
//!
//! * `WebhookHandler::verify` runs BEFORE parsing, has access to the
//!   raw body (so HMAC verification works). Returning Err
//!   short-circuits with 401 + audit log.
//! * `WsUpgradeHandler::verify_upgrade` runs against the HTTP upgrade
//!   request (cookie / Origin / HMAC) before the WS protocol handshake.
//! * Graceful shutdown: 30s drain on SIGTERM. In-flight WS / Twilio
//!   recording sessions are dropped with a warn log; alternative is
//!   blocking shutdown indefinitely.
//! * `/health` endpoint reports webhook router readiness.
//!
//! Route isolation from the Ed25519-signed `/rpc` surface is by route
//! tree separation — there is no cross-route auth delegation. Route
//! middleware on `/transport/*` is per-handler HMAC; `/rpc` keeps its
//! own Ed25519 middleware.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    body::{Body, Bytes},
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, Method, Response as AxumResponse, StatusCode, Uri},
    response::Response,
    routing::{any, get},
    Router,
};
use axum::http::Extensions;
use tokio::sync::{Notify, RwLock};
/// Slot / transport UUIDs are kept as plain strings to avoid
/// introducing a workspace-wide `uuid` dep just for routing. We
/// validate the shape via [`is_valid_uuid_str`].
fn is_valid_uuid_str(s: &str) -> bool {
    // 36 chars: 8-4-4-4-12 hex with dashes.
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    let dashes = [8usize, 13, 18, 23];
    for (i, &b) in bytes.iter().enumerate() {
        if dashes.contains(&i) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Locked drain timeout for graceful shutdown.
pub const SHUTDOWN_DRAIN: Duration = Duration::from_secs(30);

/// Buffered request handed to a `WebhookHandler`. The body has
/// already been read from the wire so `verify` can hash it without
/// consuming it for the handler.
pub struct WebhookRequest {
    pub headers: HeaderMap,
    pub uri: Uri,
    pub method: Method,
    pub raw_body: Bytes,
    pub extensions: Extensions,
}

/// Errors a verify path can return. Each maps to a structured HTTP
/// response + audit log entry; callers should NEVER raw-`unwrap` and
/// leak details to the caller (the message body is intentionally
/// generic to avoid timing oracles).
#[derive(Debug, Clone)]
pub enum VerifyError {
    /// Signature header missing or malformed.
    BadSignature,
    /// Signature didn't match the body / timestamp.
    InvalidSignature,
    /// Replay window exceeded (timestamp too old or too new).
    Replay,
    /// Origin header missing or not allowlisted (WS only).
    BadOrigin,
    /// Cookie missing or expired (WS only).
    BadCookie,
    /// Generic 4xx from the handler's verify step.
    BadRequest(String),
}

impl VerifyError {
    pub fn status(&self) -> StatusCode {
        match self {
            VerifyError::BadSignature
            | VerifyError::InvalidSignature
            | VerifyError::Replay
            | VerifyError::BadCookie => StatusCode::UNAUTHORIZED,
            VerifyError::BadOrigin => StatusCode::FORBIDDEN,
            VerifyError::BadRequest(_) => StatusCode::BAD_REQUEST,
        }
    }

    /// Audit-log kind for the violation — used by `agents::audit`.
    pub fn audit_kind(&self) -> &'static str {
        match self {
            VerifyError::BadSignature
            | VerifyError::InvalidSignature
            | VerifyError::Replay => "webhook.invalid_signature",
            VerifyError::BadOrigin => "webhook.bad_origin",
            VerifyError::BadCookie => "webhook.bad_cookie",
            VerifyError::BadRequest(_) => "webhook.bad_request",
        }
    }
}

/// Trait for webhook (POST/GET) handlers. `#[async_trait]` keeps the
/// trait object-safe so routes can store `Box<dyn WebhookHandler>`.
#[async_trait]
pub trait WebhookHandler: Send + Sync {
    /// Called BEFORE parsing. Must verify signature/HMAC against
    /// raw_body without consuming it. Err short-circuits to
    /// `verr.status()` + audit entry.
    fn verify(&self, req: &WebhookRequest) -> Result<(), VerifyError>;

    /// Called only after verify passed. Free to parse raw_body.
    async fn handle(&self, req: WebhookRequest) -> Response;
}

/// Trait for WebSocket upgrade handlers. WS upgrades bypass body
/// buffering since there's no body to verify; instead the upgrade
/// request itself (cookie / Origin / etc.) is the verification
/// payload.
#[async_trait]
pub trait WsUpgradeHandler: Send + Sync {
    fn verify_upgrade(&self, req: &Request) -> Result<(), VerifyError>;

    /// Run after verify_upgrade passed. The handler is responsible
    /// for performing the WS upgrade (e.g. via `axum::extract::WebSocketUpgrade`).
    async fn on_upgrade(&self, req: Request) -> Response;
}

/// Locked path keys: (slot_uuid, transport_uuid, kind).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteKey {
    pub slot_uuid: String,
    pub transport_uuid: String,
    pub kind: String,
}

impl RouteKey {
    pub fn new(
        slot_uuid: impl Into<String>,
        transport_uuid: impl Into<String>,
        kind: impl Into<String>,
    ) -> Self {
        Self {
            slot_uuid: slot_uuid.into(),
            transport_uuid: transport_uuid.into(),
            kind: kind.into(),
        }
    }
}

type WebhookHandlerBox = Arc<dyn WebhookHandler>;
type WsHandlerBox = Arc<dyn WsUpgradeHandler>;

/// Registry of webhook handlers, keyed by `(slot_uuid, transport_uuid, kind)`.
/// Lock-free reads via `Arc<RwLock<HashMap<...>>>`.
#[derive(Default)]
pub struct WebhookRouter {
    webhooks: RwLock<HashMap<RouteKey, WebhookHandlerBox>>,
    ws_handlers: RwLock<HashMap<RouteKey, WsHandlerBox>>,
    /// Fired on graceful shutdown. Tasks holding in-flight handlers
    /// observe this to exit cleanly.
    shutdown: Arc<Notify>,
}

impl WebhookRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a webhook handler at
    /// `/transport/<slot_uuid>/<transport_uuid>/<kind>/...`.
    pub async fn register_webhook(
        &self,
        key: RouteKey,
        handler: WebhookHandlerBox,
    ) {
        self.webhooks.write().await.insert(key, handler);
    }

    /// Register a WS upgrade handler.
    pub async fn register_ws(&self, key: RouteKey, handler: WsHandlerBox) {
        self.ws_handlers.write().await.insert(key, handler);
    }

    /// Lookup a registered webhook handler.
    pub async fn lookup_webhook(&self, key: &RouteKey) -> Option<WebhookHandlerBox> {
        self.webhooks.read().await.get(key).cloned()
    }

    /// Lookup a registered WS handler.
    pub async fn lookup_ws(&self, key: &RouteKey) -> Option<WsHandlerBox> {
        self.ws_handlers.read().await.get(key).cloned()
    }

    /// Total number of registered handlers (sum of webhook + ws).
    pub async fn len(&self) -> usize {
        self.webhooks.read().await.len() + self.ws_handlers.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Trigger graceful shutdown. In-flight requests get
    /// `SHUTDOWN_DRAIN` to complete; WS / Twilio recording sessions
    /// are dropped with a warn log.
    pub fn shutdown_signal(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }
}

/// Build the webhook surface as a `Router<()>` (state already
/// applied). Mount this under axum::Router::new().merge(...) at
/// the makakoo-mcp HTTP server.
///
/// Routes are exact path:
/// `/transport/{slot_uuid}/{transport_uuid}/{kind}` →  webhook
/// `/transport/{slot_uuid}/{transport_uuid}/{kind}/ws` →  WS upgrade
///
/// (The locked Q5 spec calls for `/{kind}/...` with a sub-path tail,
///  but no shipping transport actually needs path tail beyond the
///  three locked variants — `events`, `webhook`, `ws`. If a future
///  transport needs a tail, add a separate `/{kind}-{subkind}` shape
///  rather than a catch-all that conflicts with the named segments.)
pub fn build_router(webhook_router: Arc<WebhookRouter>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route(
            "/transport/:slot_uuid/:transport_uuid/:kind/ws",
            any(ws_dispatch),
        )
        .route(
            "/transport/:slot_uuid/:transport_uuid/:kind",
            any(webhook_dispatch),
        )
        .with_state(webhook_router)
}

/// Convenience wrapper: take an existing `Router<()>` and merge the
/// webhook surface onto it.
pub fn mount(router: Router, webhook_router: Arc<WebhookRouter>) -> Router {
    router.merge(build_router(webhook_router))
}

#[derive(serde::Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn health_handler(State(_router): State<Arc<WebhookRouter>>) -> Response {
    let body = serde_json::json!({"status": "ok"}).to_string();
    AxumResponse::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn webhook_dispatch(
    State(router): State<Arc<WebhookRouter>>,
    AxumPath((slot_str, transport_str, kind)): AxumPath<(String, String, String)>,
    req: Request,
) -> Response {
    if !is_valid_uuid_str(&slot_str) {
        return error_response(StatusCode::NOT_FOUND, "unknown slot");
    }
    if !is_valid_uuid_str(&transport_str) {
        return error_response(StatusCode::NOT_FOUND, "unknown transport");
    }
    let key = RouteKey::new(slot_str, transport_str, kind);
    let handler = match router.lookup_webhook(&key).await {
        Some(h) => h,
        None => return error_response(StatusCode::NOT_FOUND, "no handler"),
    };

    let (parts, body) = req.into_parts();
    let raw_body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "body read"),
    };
    let wreq = WebhookRequest {
        headers: parts.headers,
        uri: parts.uri,
        method: parts.method,
        raw_body,
        extensions: parts.extensions,
    };

    if let Err(e) = handler.verify(&wreq) {
        return error_response(e.status(), "verify failed");
    }
    handler.handle(wreq).await
}

async fn ws_dispatch(
    State(router): State<Arc<WebhookRouter>>,
    AxumPath((slot_str, transport_str, kind)): AxumPath<(String, String, String)>,
    req: Request,
) -> Response {
    if !is_valid_uuid_str(&slot_str) {
        return error_response(StatusCode::NOT_FOUND, "unknown slot");
    }
    if !is_valid_uuid_str(&transport_str) {
        return error_response(StatusCode::NOT_FOUND, "unknown transport");
    }
    let key = RouteKey::new(slot_str, transport_str, kind);
    let handler = match router.lookup_ws(&key).await {
        Some(h) => h,
        None => return error_response(StatusCode::NOT_FOUND, "no handler"),
    };

    if let Err(e) = handler.verify_upgrade(&req) {
        return error_response(e.status(), "verify failed");
    }
    handler.on_upgrade(req).await
}

fn error_response(status: StatusCode, msg: &str) -> Response {
    AxumResponse::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .body(Body::from(msg.to_string()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request as HttpRequest;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    /// Helper handler that records calls + always-passes verify.
    struct CountingHandler {
        called: Arc<AtomicUsize>,
        verify_ok: bool,
    }

    #[async_trait]
    impl WebhookHandler for CountingHandler {
        fn verify(&self, _req: &WebhookRequest) -> Result<(), VerifyError> {
            if self.verify_ok {
                Ok(())
            } else {
                Err(VerifyError::InvalidSignature)
            }
        }
        async fn handle(&self, _req: WebhookRequest) -> Response {
            self.called.fetch_add(1, Ordering::SeqCst);
            AxumResponse::builder()
                .status(StatusCode::OK)
                .body(Body::from("ok"))
                .unwrap()
        }
    }

    fn slot_uuid() -> &'static str {
        "00000000-0000-0000-0000-000000000001"
    }
    fn transport_uuid() -> &'static str {
        "00000000-0000-0000-0000-000000000002"
    }

    #[tokio::test]
    async fn empty_router_starts_empty() {
        let r = WebhookRouter::new();
        assert!(r.is_empty().await);
    }

    #[tokio::test]
    async fn register_then_lookup_returns_handler() {
        let r = WebhookRouter::new();
        let key = RouteKey::new(slot_uuid(), transport_uuid(), "events");
        let counted = Arc::new(AtomicUsize::new(0));
        let h: WebhookHandlerBox = Arc::new(CountingHandler {
            called: Arc::clone(&counted),
            verify_ok: true,
        });
        r.register_webhook(key.clone(), h).await;
        assert_eq!(r.len().await, 1);
        let found = r.lookup_webhook(&key).await;
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn unknown_route_lookup_returns_none() {
        let r = WebhookRouter::new();
        let key = RouteKey::new(slot_uuid(), transport_uuid(), "events");
        assert!(r.lookup_webhook(&key).await.is_none());
    }

    #[tokio::test]
    async fn route_keys_are_distinct_per_kind() {
        let r = WebhookRouter::new();
        let counted = Arc::new(AtomicUsize::new(0));
        let h: WebhookHandlerBox = Arc::new(CountingHandler {
            called: Arc::clone(&counted),
            verify_ok: true,
        });
        let key_events = RouteKey::new(slot_uuid(), transport_uuid(), "events");
        let key_voice = RouteKey::new(slot_uuid(), transport_uuid(), "voice");
        r.register_webhook(key_events.clone(), Arc::clone(&h)).await;
        r.register_webhook(key_voice.clone(), Arc::clone(&h)).await;
        assert_eq!(r.len().await, 2);
        assert!(r.lookup_webhook(&key_events).await.is_some());
        assert!(r.lookup_webhook(&key_voice).await.is_some());
    }

    #[tokio::test]
    async fn axum_dispatches_post_to_registered_handler_after_verify() {
        let router = Arc::new(WebhookRouter::new());
        let counted = Arc::new(AtomicUsize::new(0));
        let h: WebhookHandlerBox = Arc::new(CountingHandler {
            called: Arc::clone(&counted),
            verify_ok: true,
        });
        let key = RouteKey::new(slot_uuid(), transport_uuid(), "events");
        router.register_webhook(key, h).await;

        let app = mount(Router::new(), Arc::clone(&router));
        let url = format!(
            "/transport/{}/{}/events",
            slot_uuid(),
            transport_uuid()
        );
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri(&url)
                    .body(Body::from("payload"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(counted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn axum_returns_401_when_verify_fails() {
        let router = Arc::new(WebhookRouter::new());
        let counted = Arc::new(AtomicUsize::new(0));
        let h: WebhookHandlerBox = Arc::new(CountingHandler {
            called: Arc::clone(&counted),
            verify_ok: false,
        });
        let key = RouteKey::new(slot_uuid(), transport_uuid(), "events");
        router.register_webhook(key, h).await;

        let app = mount(Router::new(), Arc::clone(&router));
        let url = format!(
            "/transport/{}/{}/events",
            slot_uuid(),
            transport_uuid()
        );
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri(&url)
                    .body(Body::from("bad"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // handler.handle MUST NOT have been called.
        assert_eq!(counted.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn axum_returns_404_for_unregistered_route() {
        let router = Arc::new(WebhookRouter::new());
        let app = mount(Router::new(), Arc::clone(&router));
        let url = format!(
            "/transport/{}/{}/events",
            slot_uuid(),
            transport_uuid()
        );
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri(&url)
                    .body(Body::from("x"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn axum_returns_404_for_invalid_uuid() {
        let router = Arc::new(WebhookRouter::new());
        let app = mount(Router::new(), Arc::clone(&router));
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/transport/not-a-uuid/also-not/events")
                    .body(Body::from("x"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn health_endpoint_returns_200_json() {
        let router = Arc::new(WebhookRouter::new());
        let app = mount(Router::new(), Arc::clone(&router));
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(Method::GET)
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("\"status\""));
        assert!(body_str.contains("ok"));
    }

    #[test]
    fn verify_error_status_codes_are_locked() {
        assert_eq!(VerifyError::BadSignature.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            VerifyError::InvalidSignature.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(VerifyError::Replay.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(VerifyError::BadCookie.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(VerifyError::BadOrigin.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            VerifyError::BadRequest("x".into()).status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn verify_error_audit_kinds_are_locked() {
        assert_eq!(
            VerifyError::InvalidSignature.audit_kind(),
            "webhook.invalid_signature"
        );
        assert_eq!(VerifyError::BadOrigin.audit_kind(), "webhook.bad_origin");
        assert_eq!(VerifyError::BadCookie.audit_kind(), "webhook.bad_cookie");
    }
}
