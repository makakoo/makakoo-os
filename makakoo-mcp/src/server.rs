//! The MCP stdio event loop.
//!
//! # Protocol surface (matches Python reference)
//!
//! - `initialize` → `{protocolVersion, capabilities:{tools:{listChanged:false}},
//!    serverInfo:{name:"harvey", version}}`
//! - `notifications/initialized` → no response (JSON-RPC notification)
//! - `tools/list` → `{tools: [...]}`
//! - `tools/call` → `{content:[{type:"text", text:"..."}]}` or same shape
//!   with `isError: true` on handler errors
//! - `ping` → `{}`
//! - `shutdown` → `null`  (courtesy; Python doesn't implement this but
//!   MCP-2024-11-05 clients sometimes send it before closing stdin)
//! - any other method → `-32601 Method not found`
//!
//! Tool names and `serverInfo.name = "harvey"` are pinned by T1's
//! `tests/migration/test_mcp_framing.py` — do not rename to `makakoo`
//! without first updating the acceptance test.

use crate::dispatch::{ToolContext, ToolRegistry};
use crate::framing::{FrameReader, FrameWriter};
use crate::jsonrpc::{Request, Response, METHOD_NOT_FOUND};
use serde_json::{json, Value};
use std::io;
use std::sync::Arc;
use tracing::{debug, info};

/// MCP stdio server. Owns a shared registry + context and drives the
/// `read → dispatch → write` loop on whatever async reader/writer pair
/// you hand it (`tokio::io::stdin()` / `stdout()` in production, duplex
/// streams in tests).
pub struct McpServer {
    registry: Arc<ToolRegistry>,
    #[allow(dead_code)]
    ctx: Arc<ToolContext>,
}

impl McpServer {
    pub fn new(registry: Arc<ToolRegistry>, ctx: Arc<ToolContext>) -> Self {
        Self { registry, ctx }
    }

    /// Run the server against real stdio. Exits cleanly on EOF or the
    /// first malformed JSON line (see `framing.rs` for the pinned
    /// behavior and the path to flip it).
    pub async fn run_stdio(&self) -> io::Result<()> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        self.run(stdin, stdout).await
    }

    /// Run the event loop against any async reader/writer pair.
    pub async fn run<R, W>(&self, reader: R, writer: W) -> io::Result<()>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        info!("makakoo-mcp server running");
        let mut reader = FrameReader::new(reader);
        let mut writer = FrameWriter::new(writer);

        // Phase 3: stdio MCP path reads MAKAKOO_AGENT_SLOT once at
        // startup. The HTTP path reads X-Makakoo-Agent-Id per
        // request — different transport, same task-local sink.
        let agent_id = std::env::var(makakoo_core::agents::AGENT_SLOT_ENV_VAR).ok();
        if let Some(ref id) = agent_id {
            info!(agent_id = %id, "stdio mcp call attributed to subagent");
        }

        while let Some(req) = reader.read_message().await? {
            debug!(method = %req.method, id = ?req.id, "recv");

            // JSON-RPC notifications (no id) get no response.
            let is_notification = req.id.is_none();
            if is_notification && req.method.starts_with("notifications/") {
                // Acknowledged silently, matches the Python reference.
                continue;
            }

            let resp = crate::dispatch::AGENT_ID
                .scope(agent_id.clone(), self.handle(req))
                .await;
            if let Some(resp) = resp {
                writer.write_response(&resp).await?;
            }
        }
        info!("makakoo-mcp server stopped");
        Ok(())
    }

    /// Handle one request. Returns `None` for notifications that should
    /// not generate a response, `Some(Response)` otherwise.
    pub async fn handle(&self, req: Request) -> Option<Response> {
        let id = req.id.clone().unwrap_or(Value::Null);

        match req.method.as_str() {
            "initialize" => Some(Response::success(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": { "listChanged": false }
                    },
                    "serverInfo": {
                        "name": "harvey",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )),

            "notifications/initialized" => None,

            "tools/list" => Some(Response::success(
                id,
                json!({ "tools": self.registry.list() }),
            )),

            "tools/call" => {
                let tool_name = req
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let tool_args = req
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));

                match tool_name {
                    Some(name) => match self.registry.call(&name, tool_args).await {
                        Ok(result) => {
                            // Python reference wraps results in a content[]
                            // block with a single text element. The text is
                            // the stringified JSON payload so MCP clients
                            // that only show text get something readable.
                            let text = match &result {
                                Value::String(s) => s.clone(),
                                other => serde_json::to_string(other).unwrap_or_default(),
                            };
                            Some(Response::success(
                                id,
                                json!({
                                    "content": [{ "type": "text", "text": text }]
                                }),
                            ))
                        }
                        Err(e) => {
                            // Matches Python: tool errors come back in the
                            // result envelope with isError=true, NOT as
                            // JSON-RPC error objects. This is the MCP
                            // protocol convention for tool-level failures.
                            Some(Response::success(
                                id,
                                json!({
                                    "content": [{
                                        "type": "text",
                                        "text": format!("Error: {}", e.message)
                                    }],
                                    "isError": true
                                }),
                            ))
                        }
                    },
                    None => Some(Response::failure(
                        id,
                        crate::jsonrpc::INVALID_PARAMS,
                        "tools/call missing 'name' parameter",
                    )),
                }
            }

            "ping" => Some(Response::success(id, json!({}))),

            "shutdown" => Some(Response::success(id, Value::Null)),

            other => {
                if req.id.is_some() {
                    Some(Response::failure(
                        id,
                        METHOD_NOT_FOUND,
                        format!("Method not found: {}", other),
                    ))
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{ToolContext, ToolHandler, ToolRegistry};
    use crate::jsonrpc::RpcError;
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::PathBuf;

    fn empty_server() -> McpServer {
        let registry = Arc::new(ToolRegistry::new());
        let ctx = Arc::new(ToolContext::empty(PathBuf::from("/tmp")));
        McpServer::new(registry, ctx)
    }

    #[tokio::test]
    async fn initialize_handshake_matches_python() {
        let s = empty_server();
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .unwrap();
        let resp = s.handle(req).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "harvey");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn notifications_initialized_returns_none() {
        let s = empty_server();
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .unwrap();
        assert!(s.handle(req).await.is_none());
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let s = empty_server();
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":99,"method":"nonsense/doesNotExist","params":{}}"#,
        )
        .unwrap();
        let resp = s.handle(req).await.unwrap();
        assert_eq!(resp.id, json!(99));
        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn ping_returns_empty_object() {
        let s = empty_server();
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":77,"method":"ping","params":{}}"#)
                .unwrap();
        let resp = s.handle(req).await.unwrap();
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    #[tokio::test]
    async fn tools_list_is_empty_without_handlers() {
        let s = empty_server();
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let resp = s.handle(req).await.unwrap();
        assert_eq!(resp.result.unwrap(), json!({"tools": []}));
    }

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
            Ok(json!("hi there"))
        }
    }

    #[tokio::test]
    async fn tools_call_wraps_string_result_in_text_content() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Hello));
        let s = McpServer::new(
            Arc::new(registry),
            Arc::new(ToolContext::empty(PathBuf::from("/tmp"))),
        );
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"hello","arguments":{}}}"#,
        )
        .unwrap();
        let resp = s.handle(req).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "hi there");
        assert!(result.get("isError").is_none());
    }

    struct Boom;

    #[async_trait]
    impl ToolHandler for Boom {
        fn name(&self) -> &str {
            "boom"
        }
        fn description(&self) -> &str {
            ""
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, _: Value) -> Result<Value, RpcError> {
            Err(RpcError::internal("kaboom"))
        }
    }

    #[tokio::test]
    async fn tools_call_handler_error_returns_is_error_in_result() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Boom));
        let s = McpServer::new(
            Arc::new(registry),
            Arc::new(ToolContext::empty(PathBuf::from("/tmp"))),
        );
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"boom","arguments":{}}}"#,
        )
        .unwrap();
        let resp = s.handle(req).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("kaboom"));
    }

    #[tokio::test]
    async fn tools_call_missing_name_returns_invalid_params() {
        let s = empty_server();
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"arguments":{}}}"#,
        )
        .unwrap();
        let resp = s.handle(req).await.unwrap();
        assert_eq!(resp.error.unwrap().code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn full_stream_round_trip_via_duplex() {
        // End-to-end: pipe an initialize request through `run` and parse
        // the response off the other side. This is the same shape the
        // T1 acceptance tests use, just with an in-memory duplex instead
        // of a real subprocess.
        let registry = Arc::new(ToolRegistry::new());
        let ctx = Arc::new(ToolContext::empty(PathBuf::from("/tmp")));
        let server = McpServer::new(registry, ctx);

        let (mut client_side, server_side) = tokio::io::duplex(4096);
        let (server_out_read, server_out_write) = tokio::io::duplex(4096);

        let server_handle = tokio::spawn(async move {
            server.run(server_side, server_out_write).await.unwrap();
        });

        use tokio::io::AsyncWriteExt;
        client_side
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
            .await
            .unwrap();
        client_side.flush().await.unwrap();

        // Read one line of response
        let mut reader = tokio::io::BufReader::new(server_out_read);
        let mut line = String::new();
        use tokio::io::AsyncBufReadExt;
        reader.read_line(&mut line).await.unwrap();

        // Close client stdin → server exits cleanly
        drop(client_side);
        server_handle.await.unwrap();

        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["result"]["serverInfo"]["name"], "harvey");
        assert_eq!(parsed["result"]["protocolVersion"], "2024-11-05");
    }
}
