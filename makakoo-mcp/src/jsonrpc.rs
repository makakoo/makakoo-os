//! JSON-RPC 2.0 types for the Makakoo MCP server.
//!
//! Wire format (MCP stdio transport): one JSON object per line, terminated
//! by `\n`. NOT LSP-style Content-Length headers — the Python reference and
//! the T1 acceptance tests (`tests/migration/test_mcp_framing.py`) both pin
//! newline-delimited JSON as the ground truth. See `framing.rs`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Inbound JSON-RPC 2.0 request or notification.
///
/// `id` is absent for notifications (e.g. `notifications/initialized`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Outbound JSON-RPC 2.0 response (success or error).
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC 2.0 error object (`{"code":..,"message":..,"data":..}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// Standard JSON-RPC 2.0 error codes. Kept as a complete set even though
// not every code is used at T12 — Wave 4 handlers will emit all of them.
#[allow(dead_code)]
pub const PARSE_ERROR: i64 = -32700;
#[allow(dead_code)]
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

impl Response {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    #[allow(dead_code)]
    pub fn failure_with_data(
        id: Value,
        code: i64,
        message: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: Some(data),
            }),
        }
    }
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[allow(dead_code)]
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, msg)
    }

    #[allow(dead_code)]
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_deserializes_with_id() {
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#,
        )
        .unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(json!(1)));
        assert_eq!(req.method, "ping");
    }

    #[test]
    fn request_deserializes_notification_without_id() {
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .unwrap();
        assert!(req.id.is_none());
        assert_eq!(req.method, "notifications/initialized");
    }

    #[test]
    fn success_response_omits_error_field() {
        let resp = Response::success(json!(7), json!({"ok": true}));
        let wire = serde_json::to_string(&resp).unwrap();
        assert!(wire.contains(r#""result":{"ok":true}"#));
        assert!(!wire.contains("error"));
        assert!(wire.contains(r#""jsonrpc":"2.0""#));
        assert!(wire.contains(r#""id":7"#));
    }

    #[test]
    fn failure_response_omits_result_field() {
        let resp = Response::failure(json!("abc"), METHOD_NOT_FOUND, "nope");
        let wire = serde_json::to_string(&resp).unwrap();
        assert!(wire.contains(r#""error":{"code":-32601,"message":"nope"}"#));
        assert!(!wire.contains("\"result\""));
        assert!(wire.contains(r#""id":"abc""#));
    }

    #[test]
    fn rpc_error_constructors() {
        let e = RpcError::invalid_params("missing name");
        assert_eq!(e.code, INVALID_PARAMS);
        let e = RpcError::internal("boom");
        assert_eq!(e.code, INTERNAL_ERROR);
    }
}
