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
use crate::jsonrpc::{Request, Response, INTERNAL_ERROR, METHOD_NOT_FOUND};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info};

/// MCP stdio server. Owns a shared registry + context and drives the
/// `read → dispatch → write` loop on whatever async reader/writer pair
/// you hand it (`tokio::io::stdin()` / `stdout()` in production, duplex
/// streams in tests).
pub struct McpServer {
    registry: Arc<ToolRegistry>,
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

            "tools/list" => match self.scoped_tools() {
                Ok(tools) => Some(Response::success(id, json!({ "tools": tools }))),
                Err(message) => Some(Response::failure(id, INTERNAL_ERROR, message)),
            },

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
                    Some(name) => match self.authorize_tool(&name, &tool_args) {
                        Err(message) => Some(Self::tool_error(id, message)),
                        Ok(()) => match self.registry.call(&name, tool_args).await {
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
                                Some(Self::tool_error(id, e.message))
                            }
                        },
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

    fn scoped_tools(&self) -> Result<Vec<crate::dispatch::ToolDescriptor>, String> {
        let Some(slot) = self.current_slot()? else {
            return Ok(self.registry.list());
        };
        Ok(self
            .registry
            .list()
            .into_iter()
            .filter(|tool| makakoo_core::agents::check_tool(&slot, &tool.name).is_ok())
            .collect())
    }

    fn authorize_tool(&self, name: &str, arguments: &Value) -> Result<(), String> {
        let Some(slot) = self.current_slot()? else {
            return Ok(());
        };
        makakoo_core::agents::check_tool(&slot, name).map_err(|e| e.to_string())?;
        for candidate in filesystem_paths(arguments) {
            let candidate = root_candidate(&self.ctx.home, candidate);
            makakoo_core::agents::check_path(&slot, &candidate).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn current_slot(&self) -> Result<Option<makakoo_core::agents::AgentSlot>, String> {
        let Some(slot_id) = crate::dispatch::current_agent_id() else {
            return Ok(None);
        };
        let path = makakoo_core::agents::checked_slot_path(&self.ctx.home, &slot_id)
            .map_err(|e| format!("invalid agent slot id '{}': {}", slot_id, e))?;
        makakoo_core::agents::AgentSlot::load_from_file(&path)
            .map(Some)
            .map_err(|e| format!("agent slot '{}' scope load failed: {}", slot_id, e))
    }

    fn tool_error(id: Value, message: impl Into<String>) -> Response {
        Response::success(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": format!("Error: {}", message.into())
                }],
                "isError": true
            }),
        )
    }
}

/// Extract filesystem-bearing MCP arguments before handler dispatch. Tool
/// schemas use several names (`path`, `file_path`, `source_path`, ...), so the
/// boundary walks nested objects and treats `*_path`/`*_paths` uniformly.
/// `source` is included only when it is a local path, not a URL/data URI.
fn filesystem_paths(arguments: &Value) -> Vec<PathBuf> {
    fn walk(key: Option<&str>, value: &Value, out: &mut Vec<PathBuf>) {
        match value {
            Value::Object(map) => {
                for (child_key, child) in map {
                    walk(Some(child_key), child, out);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(key, item, out);
                }
            }
            Value::String(candidate) if key.is_some_and(is_filesystem_key) => {
                if key.is_some_and(is_remote_capable_source_key)
                    && looks_like_remote_source(candidate)
                {
                    return;
                }
                out.push(Path::new(candidate).to_path_buf());
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    walk(None, arguments, &mut out);
    out
}

fn is_filesystem_key(key: &str) -> bool {
    let key = normalize_argument_key(key);
    if matches!(
        key.as_str(),
        "path"
            | "paths"
            | "filepath"
            | "filepaths"
            | "file"
            | "files"
            | "dir"
            | "dirs"
            | "directory"
            | "directories"
            | "dirname"
            | "folder"
            | "folders"
            | "cwd"
            | "workdir"
            | "working_dir"
            | "working_directory"
            | "target"
            | "targets"
            | "output"
            | "outputs"
            | "destination"
            | "destinations"
            | "dest"
            | "source"
            | "sources"
            | "src"
            | "dst"
            | "filename"
            | "filenames"
            | "fullpath"
            | "fullpaths"
    ) || [
        "_path",
        "_paths",
        "_file",
        "_files",
        "_dir",
        "_dirs",
        "_directory",
        "_directories",
        "_folder",
        "_folders",
        "_cwd",
        "_target",
        "_targets",
        "_output",
        "_outputs",
        "_destination",
        "_destinations",
        "_dest",
        "_source",
        "_sources",
        "_src",
        "_dst",
        "_filename",
        "_filenames",
        "_fullpath",
        "_fullpaths",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
    {
        return true;
    }
    let squashed = key.replace('_', "");
    [
        "filepath",
        "filepaths",
        "dirname",
        "workdir",
        "workingdirectory",
        "filename",
        "filenames",
        "fullpath",
        "fullpaths",
    ]
    .iter()
    .any(|suffix| squashed.ends_with(suffix))
}

fn normalize_argument_key(key: &str) -> String {
    let mut normalized = String::with_capacity(key.len());
    let mut previous_was_lower_or_digit = false;
    let mut previous_was_separator = false;
    for character in key.chars() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase()
                && previous_was_lower_or_digit
                && !previous_was_separator
            {
                normalized.push('_');
            }
            normalized.push(character.to_ascii_lowercase());
            previous_was_lower_or_digit =
                character.is_ascii_lowercase() || character.is_ascii_digit();
            previous_was_separator = false;
        } else if !normalized.is_empty() && !previous_was_separator {
            normalized.push('_');
            previous_was_lower_or_digit = false;
            previous_was_separator = true;
        }
    }
    normalized.trim_matches('_').to_string()
}

fn is_remote_capable_source_key(key: &str) -> bool {
    matches!(normalize_argument_key(key).as_str(), "source" | "sources")
}

/// Mirrors the downstream remote-source predicates exactly
/// (`LlmClient::encode_source` in makakoo-core/src/llm.rs and
/// `resolve_source` in plugins-core/agent-multimodal-knowledge/src/ingest.py):
/// only `http://`, `https://`, and `data:` are remote. Anything else —
/// including `file://` URIs and Windows drive-letter paths such as
/// `C://Users/...` whose "scheme" would parse as `C` — is a local path
/// and must pass `check_path`.
fn looks_like_remote_source(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://") || value.starts_with("data:")
}

fn root_candidate(home: &Path, candidate: PathBuf) -> PathBuf {
    if candidate.is_absolute() || candidate.to_string_lossy().starts_with("~/") {
        candidate
    } else {
        home.join(candidate)
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
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
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
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
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

    struct PathEcho;

    #[async_trait]
    impl ToolHandler for PathEcho {
        fn name(&self) -> &str {
            "path_echo"
        }
        fn description(&self) -> &str {
            "returns a path"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object", "properties": {"path": {"type": "string"}}})
        }
        async fn call(&self, params: Value) -> Result<Value, RpcError> {
            Ok(params)
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

    fn scoped_server(home: &std::path::Path) -> McpServer {
        scoped_server_with_tools(home, vec!["hello".into()])
    }

    fn scoped_server_with_tools(home: &std::path::Path, tools: Vec<String>) -> McpServer {
        use makakoo_core::agents::{AgentRegistry, AgentSlot};

        let slot = AgentSlot {
            slot_id: "scoped".into(),
            name: "Scoped".into(),
            persona: None,
            inherit_baseline: false,
            allowed_paths: vec![],
            forbidden_paths: vec![],
            tools,
            process_mode: "supervised_pair".into(),
            transports: vec![],
            llm: None,
            runtime: None,
            triggers: Vec::new(),
        };
        AgentRegistry::create(home, &slot).unwrap();

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Hello));
        registry.register(Arc::new(Boom));
        McpServer::new(
            Arc::new(registry),
            Arc::new(ToolContext::empty(home.to_path_buf())),
        )
    }

    #[tokio::test]
    async fn agent_scope_filters_tool_discovery() {
        let tmp = tempfile::tempdir().unwrap();
        let server = scoped_server(tmp.path());
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":8,"method":"tools/list"}"#).unwrap();
        let resp = crate::dispatch::AGENT_ID
            .scope(Some("scoped".into()), server.handle(req))
            .await
            .unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "hello");
    }

    #[tokio::test]
    async fn empty_agent_tool_list_exposes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let server = scoped_server_with_tools(tmp.path(), vec![]);
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":11,"method":"tools/list"}"#).unwrap();
        let resp = crate::dispatch::AGENT_ID
            .scope(Some("scoped".into()), server.handle(req))
            .await
            .unwrap();
        assert!(resp.result.unwrap()["tools"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn agent_scope_rejects_out_of_scope_tool_call() {
        let tmp = tempfile::tempdir().unwrap();
        let server = scoped_server(tmp.path());
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"boom","arguments":{}}}"#,
        )
        .unwrap();
        let resp = crate::dispatch::AGENT_ID
            .scope(Some("scoped".into()), server.handle(req))
            .await
            .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not in scope"));
    }

    #[tokio::test]
    async fn agent_scope_fails_closed_when_slot_is_missing() {
        let server = empty_server();
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":10,"method":"tools/list"}"#).unwrap();
        let resp = crate::dispatch::AGENT_ID
            .scope(Some("missing".into()), server.handle(req))
            .await
            .unwrap();
        assert_eq!(resp.error.unwrap().code, INTERNAL_ERROR);
    }

    fn path_scoped_server(home: &std::path::Path) -> McpServer {
        use makakoo_core::agents::{AgentRegistry, AgentSlot};

        std::fs::create_dir_all(home.join("allowed")).unwrap();
        let slot = AgentSlot {
            slot_id: "path-scoped".into(),
            name: "Path scoped".into(),
            persona: None,
            inherit_baseline: false,
            allowed_paths: vec![home.join("allowed").display().to_string()],
            forbidden_paths: vec![home.join("allowed/private").display().to_string()],
            tools: vec!["path_echo".into()],
            process_mode: "supervised_pair".into(),
            transports: vec![],
            llm: None,
            runtime: None,
            triggers: Vec::new(),
        };
        AgentRegistry::create(home, &slot).unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(PathEcho));
        McpServer::new(
            Arc::new(registry),
            Arc::new(ToolContext::empty(home.to_path_buf())),
        )
    }

    async fn scoped_path_call(server: &McpServer, path: &str) -> Value {
        let req = Request {
            jsonrpc: "2.0".into(),
            id: Some(json!(12)),
            method: "tools/call".into(),
            params: json!({"name": "path_echo", "arguments": {"path": path}}),
        };
        crate::dispatch::AGENT_ID
            .scope(Some("path-scoped".into()), server.handle(req))
            .await
            .unwrap()
            .result
            .unwrap()
    }

    #[tokio::test]
    async fn agent_scope_enforces_filesystem_arguments_before_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let server = path_scoped_server(tmp.path());

        let allowed = scoped_path_call(&server, "allowed/notes.md").await;
        assert!(allowed.get("isError").is_none());

        let outside = scoped_path_call(&server, "/etc/passwd").await;
        assert_eq!(outside["isError"], true);
        assert!(outside["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not in scope"));

        let forbidden = scoped_path_call(&server, "allowed/private/key").await;
        assert_eq!(forbidden["isError"], true);
    }

    #[test]
    fn source_urls_are_not_treated_as_local_paths() {
        assert!(filesystem_paths(&json!({"source": "https://example.com/a"})).is_empty());
        assert!(filesystem_paths(&json!({"SOURCES": ["data:image/png;base64,AA"]})).is_empty());
        assert_eq!(
            filesystem_paths(&json!({"source_path": "docs/a.md"})),
            vec![PathBuf::from("docs/a.md")]
        );
        assert_eq!(
            filesystem_paths(&json!({"source": "/tmp/secret://payload"})),
            vec![PathBuf::from("/tmp/secret://payload")]
        );
        assert_eq!(
            filesystem_paths(&json!({"source": "file:///tmp/secret"})),
            vec![PathBuf::from("file:///tmp/secret")]
        );
    }

    #[test]
    fn non_http_schemes_are_local_paths_at_scope_boundary() {
        // Downstream consumers treat only http/https/data as remote, so
        // the boundary must too: a Windows drive-letter path whose
        // "scheme" is `C` (or any other non-http scheme) must still go
        // through `check_path` instead of bypassing allowed_paths.
        for value in [
            "C://Users/victim/secret.txt",
            "c://windows/system32/config/sam",
            "ftp://example.com/a",
            "gopher://example.com/a",
        ] {
            assert_eq!(
                filesystem_paths(&json!({"source": value})),
                vec![PathBuf::from(value)],
                "non-http scheme escaped check_path: {value}"
            );
        }
        assert!(filesystem_paths(&json!({"source": "http://example.com/a"})).is_empty());
        assert!(filesystem_paths(&json!({"source": "https://example.com/a"})).is_empty());
        assert!(filesystem_paths(&json!({"source": "data:text/plain,hello"})).is_empty());
    }

    #[test]
    fn filesystem_argument_names_fail_closed_across_common_aliases() {
        for key in [
            "path",
            "paths",
            "filepath",
            "filepaths",
            "file",
            "files",
            "dir",
            "dirs",
            "directory",
            "folder",
            "cwd",
            "workdir",
            "working_directory",
            "target",
            "output",
            "destination",
            "dest",
            "source",
            "sources",
            "config_path",
            "OUTPUT_FILE",
            "myfilepath",
            "outFilePath",
            "workingDirectory",
            "src",
            "dst",
            "filename",
            "filenames",
            "fullpath",
            "fullPath",
            "output_filename",
            "backup_dst",
        ] {
            assert!(is_filesystem_key(key), "filesystem alias escaped: {key}");
        }
        assert!(!is_filesystem_key("query"));
        assert!(!is_filesystem_key("target_language"));
    }

    #[test]
    fn tilde_paths_remain_expandable_at_scope_boundary() {
        assert_eq!(
            root_candidate(Path::new("/tmp/home"), PathBuf::from("~/Office/report.md")),
            PathBuf::from("~/Office/report.md")
        );
        assert_eq!(
            root_candidate(Path::new("/tmp/home"), PathBuf::from("relative/report.md")),
            PathBuf::from("/tmp/home/relative/report.md")
        );
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
