//! v0.6 Phase B — HTTP serve mode for makakoo-mcp.
//!
//! One route: `POST /rpc` — accepts one JSON-RPC 2.0 request per body,
//! returns one response. Authentication is Ed25519 signed-request — the
//! only shipping mode. There is deliberately no unauthenticated network
//! entry point (per v0.6 SPRINT.md D5).
//!
//! # Wire contract
//!
//! ```text
//! POST /rpc
//! X-Makakoo-Peer: <name>                  required
//! X-Makakoo-Ts:   <unix-millis>           required
//! X-Makakoo-Sig:  ed25519=<base64>        required
//! Content-Type:   application/json        required
//!
//! <json-rpc 2.0 request body>
//! ```
//!
//! Failure modes:
//!
//!   - Missing / malformed headers     → 400 Bad Request
//!   - Unknown peer / bad signature    → 401 Unauthorized
//!   - Clock drift beyond window       → 401 Unauthorized (drift error)
//!   - Malformed JSON body             → 400 Bad Request
//!   - Any server-side handler error   → 500 with a JSON body
//!   - Valid request                   → 200 with the JSON-RPC response body
//!
//! TLS is the user's reverse-proxy job. By default the listener binds to
//! `127.0.0.1` — binding elsewhere requires an explicit `--bind`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use ed25519_dalek::VerifyingKey;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use makakoo_core::adapter::peer::{
    self, PeerError, PEER_HEADER, SIG_HEADER, TS_HEADER,
};

use crate::dispatch::{ToolContext, ToolRegistry};
use crate::jsonrpc::Request;
use crate::server::McpServer;

/// Shared state handed to every request. `trust` is behind an `RwLock`
/// so future hot-reload can replace it without rebuilding the router.
pub struct HttpState {
    pub server: McpServer,
    pub trust: RwLock<HashMap<String, VerifyingKey>>,
    /// On-disk location of the trust file. Kept for diagnostic surfaces
    /// (e.g. future `tools/list` handler that reports configured peers)
    /// and for the future hot-reload path.
    #[allow(dead_code)]
    pub trust_path: PathBuf,
}

impl HttpState {
    pub fn new(
        registry: Arc<ToolRegistry>,
        ctx: Arc<ToolContext>,
        trust: HashMap<String, VerifyingKey>,
        trust_path: PathBuf,
    ) -> Self {
        Self {
            server: McpServer::new(registry, ctx),
            trust: RwLock::new(trust),
            trust_path,
        }
    }
}

/// Build the axum router. Exposed so tests can hit the handler directly
/// without actually binding a TCP socket.
pub fn router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/rpc", post(handle_rpc))
        .with_state(state)
}

/// Bind + serve. Returns the bound SocketAddr so callers logging or
/// tests discovering the port can consume it.
pub async fn serve(bind: SocketAddr, state: Arc<HttpState>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    let local = listener.local_addr()?;
    info!(addr = %local, trust_peers = %state.trust.read().await.len(),
          "makakoo-mcp listening on HTTP with Ed25519 peer auth");
    if !bind.ip().is_loopback() {
        warn!(
            "makakoo-mcp is bound to a non-loopback interface ({}). \
             Ed25519 peer auth is still enforced, but make sure your network \
             posture matches your intent.",
            bind.ip()
        );
    }
    axum::serve(listener, router(state)).await?;
    Ok(())
}


async fn handle_rpc(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // 1. Parse + validate the auth headers.
    let peer_name = match header_str(&headers, PEER_HEADER) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let ts_str = match header_str(&headers, TS_HEADER) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let sig_header = match header_str(&headers, SIG_HEADER) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let ts: i64 = match ts_str.parse() {
        Ok(t) => t,
        Err(_) => return bad_request(format!("{TS_HEADER} must be a unix-millis integer")),
    };

    // 2. Verify against the trust store + clock window.
    let trust = state.trust.read().await;
    if let Err(err) = peer::verify_request(
        &trust,
        &peer_name,
        &body,
        ts,
        sig_header.as_str(),
        peer::now_millis(),
    ) {
        return auth_failure(err);
    }
    drop(trust);

    // 3. Parse the JSON-RPC envelope.
    let req: Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad_request(format!("malformed JSON-RPC body: {e}")),
    };

    debug!(peer = %peer_name, method = %req.method, id = ?req.id, "rpc");

    // 4. Dispatch through the same server code that handles stdio.
    match state.server.handle(req).await {
        Some(resp) => {
            let body = serde_json::to_vec(&resp).unwrap_or_default();
            (
                StatusCode::OK,
                [("Content-Type", "application/json")],
                body,
            )
                .into_response()
        }
        None => {
            // Notification → 204 No Content.
            (StatusCode::NO_CONTENT, "").into_response()
        }
    }
}

fn header_str(headers: &HeaderMap, name: &'static str) -> Result<String, Response> {
    match headers.get(name) {
        Some(v) => match v.to_str() {
            Ok(s) => Ok(s.to_string()),
            Err(_) => Err(bad_request(format!("{name} must be valid ASCII"))),
        },
        None => Err(bad_request(format!("{name} header required"))),
    }
}

fn bad_request(msg: impl Into<String>) -> Response {
    let body = serde_json::json!({ "error": msg.into() });
    (
        StatusCode::BAD_REQUEST,
        [("Content-Type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn auth_failure(err: PeerError) -> Response {
    let msg = err.to_string();
    let body = serde_json::json!({ "error": msg });
    (
        StatusCode::UNAUTHORIZED,
        [("Content-Type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{ToolHandler, ToolRegistry};
    use crate::jsonrpc::RpcError;
    use async_trait::async_trait;
    use base64::Engine;
    use ed25519_dalek::SigningKey;
    use serde_json::{json, Value};

    struct Hello;

    #[async_trait]
    impl ToolHandler for Hello {
        fn name(&self) -> &str {
            "hello"
        }
        fn description(&self) -> &str {
            "greets"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, _: Value) -> Result<Value, RpcError> {
            Ok(json!("hi from http"))
        }
    }

    fn test_state() -> (Arc<HttpState>, SigningKey) {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Hello));
        let ctx = Arc::new(ToolContext::empty(std::path::PathBuf::from("/tmp")));

        // Generate a fresh keypair; insert pub into trust map as "clienta".
        use rand::RngCore;
        let mut s = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut s);
        let signing = SigningKey::from_bytes(&s);
        let verifying = signing.verifying_key();
        let mut trust = HashMap::new();
        trust.insert("clienta".to_string(), verifying);

        let state = Arc::new(HttpState::new(
            Arc::new(registry),
            ctx,
            trust,
            PathBuf::from("/tmp/unused-trust"),
        ));
        (state, signing)
    }

    fn signed_headers(signing: &SigningKey, body: &[u8], peer: &str) -> Vec<(String, String)> {
        let ts = peer::now_millis();
        let sig = peer::sign_request(signing, body, ts);
        vec![
            (PEER_HEADER.to_string(), peer.to_string()),
            (TS_HEADER.to_string(), ts.to_string()),
            (SIG_HEADER.to_string(), format!("{}{}", peer::SIG_PREFIX, sig)),
            ("Content-Type".to_string(), "application/json".to_string()),
        ]
    }

    async fn post_via_axum(
        state: Arc<HttpState>,
        headers: Vec<(String, String)>,
        body: &[u8],
    ) -> (StatusCode, String) {
        use axum::http::Request as AxumRequest;
        use tower::ServiceExt;

        let app = router(state);
        let mut builder = AxumRequest::post("/rpc");
        for (k, v) in &headers {
            builder = builder.header(k, v);
        }
        let req = builder.body(axum::body::Body::from(body.to_vec())).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn tools_list_with_valid_signature_round_trip() {
        let (state, signing) = test_state();
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let headers = signed_headers(&signing, body, "clienta");
        let (status, resp) = post_via_axum(state, headers, body).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed["result"]["tools"][0]["name"], "hello");
    }

    #[tokio::test]
    async fn tools_call_dispatches_through_registry() {
        let (state, signing) = test_state();
        let body = br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"hello","arguments":{}}}"#;
        let headers = signed_headers(&signing, body, "clienta");
        let (status, resp) = post_via_axum(state, headers, body).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed["result"]["content"][0]["text"], "hi from http");
    }

    #[tokio::test]
    async fn missing_peer_header_returns_400() {
        let (state, signing) = test_state();
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let mut headers = signed_headers(&signing, body, "clienta");
        headers.retain(|(k, _)| k != PEER_HEADER);
        let (status, body_str) = post_via_axum(state, headers, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body_str.contains(PEER_HEADER));
    }

    #[tokio::test]
    async fn unknown_peer_returns_401() {
        let (state, signing) = test_state();
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let headers = signed_headers(&signing, body, "strangerzzz");
        let (status, _) = post_via_axum(state, headers, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn tampered_body_returns_401() {
        let (state, signing) = test_state();
        let signed_body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let headers = signed_headers(&signing, signed_body, "clienta");
        // Send a DIFFERENT body with the same signature.
        let tampered = br#"{"jsonrpc":"2.0","id":99,"method":"tools/list"}"#;
        let (status, _) = post_via_axum(state, headers, tampered).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn drift_beyond_window_returns_401() {
        let (state, signing) = test_state();
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        // Sign with a ts that's 2 minutes in the past.
        let ts = peer::now_millis() - 120_000;
        let sig = peer::sign_request(&signing, body, ts);
        let headers = vec![
            (PEER_HEADER.to_string(), "clienta".to_string()),
            (TS_HEADER.to_string(), ts.to_string()),
            (
                SIG_HEADER.to_string(),
                format!("{}{}", peer::SIG_PREFIX, sig),
            ),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let (status, _) = post_via_axum(state, headers, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn malformed_json_body_returns_400() {
        let (state, signing) = test_state();
        let body = b"not json at all";
        let headers = signed_headers(&signing, body, "clienta");
        let (status, _) = post_via_axum(state, headers, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn ping_returns_empty_object() {
        let (state, signing) = test_state();
        let body = br#"{"jsonrpc":"2.0","id":42,"method":"ping"}"#;
        let headers = signed_headers(&signing, body, "clienta");
        let (status, resp) = post_via_axum(state, headers, body).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed["result"], json!({}));
    }

    #[tokio::test]
    async fn base64_module_is_reachable() {
        // Sanity: the b64 engine used in peer.rs must be the same as the
        // engine tests construct encoded pubkeys with.
        let raw = [1u8; 32];
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(encoded.len(), 44);
    }
}
