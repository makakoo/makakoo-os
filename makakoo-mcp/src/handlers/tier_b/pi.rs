//! Pi (badlogic/pi-mono) MCP tool handlers.
//!
//! Exposes pi as a first-class worker callable from any MCP-connected
//! CLI. Every handler spawns `pi --rpc` as a subprocess, pipes JSONL over
//! stdin/stdout, and returns the parsed result.
//!
//! Handlers shipped in v0.2 B.3/B.4:
//!   * `pi_run(prompt, session_id?, model?, timeout_s?)` — one turn
//!   * `pi_session_fork(session_id, from_msg_id)` — branch a session
//!   * `pi_session_label(session_id, msg_id, label)` — label a message
//!   * `pi_session_export(session_id, format)` — export as html|md
//!   * `pi_set_model(session_id, provider, model_id)` — mid-session swap
//!   * `pi_steer(session_id, message)` — inject guidance between turns
//!
//! All tools fail with a clear `RpcError::internal` if the `pi` binary
//! isn't on PATH — run `makakoo plugin health agent-pi` to diagnose.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

/// Default per-turn timeout if the caller doesn't specify one.
const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// Upper bound the caller cannot exceed — protects the kernel from a
/// runaway pi process hanging forever.
const MAX_TIMEOUT_SECS: u64 = 1800;

fn which_pi() -> Result<std::path::PathBuf, RpcError> {
    let pi = std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths).find_map(|p| {
                let candidate = p.join("pi");
                if candidate.is_file() {
                    Some(candidate)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            RpcError::internal(
                "pi binary not on PATH. Install pi and run `makakoo plugin health agent-pi`.",
            )
        })?;
    Ok(pi)
}

/// Spawn `pi --rpc` with the given extra args, returning the live child.
fn spawn_pi_rpc(extra_args: &[&str]) -> Result<Child, RpcError> {
    let pi = which_pi()?;
    let mut cmd = Command::new(pi);
    cmd.arg("--rpc");
    for a in extra_args {
        cmd.arg(a);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd.spawn()
        .map_err(|e| RpcError::internal(format!("pi --rpc spawn failed: {e}")))
}

/// Write one JSONL message + newline to pi's stdin, then close stdin.
async fn write_one_request(child: &mut Child, payload: Value) -> Result<(), RpcError> {
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| RpcError::internal("pi child stdin unavailable"))?;
    let mut stdin = stdin;
    let bytes = serde_json::to_vec(&payload)
        .map_err(|e| RpcError::internal(format!("encode pi request: {e}")))?;
    stdin
        .write_all(&bytes)
        .await
        .map_err(|e| RpcError::internal(format!("pi stdin write: {e}")))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| RpcError::internal(format!("pi stdin newline: {e}")))?;
    // Drop stdin to signal EOF — pi finishes processing and exits.
    drop(stdin);
    Ok(())
}

/// Drain pi's stdout, collecting each JSONL frame. Returns every parsed
/// JSON object emitted by pi during the turn.
async fn drain_stdout(child: &mut Child) -> Result<Vec<Value>, RpcError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RpcError::internal("pi child stdout unavailable"))?;
    let mut reader = BufReader::new(stdout).lines();
    let mut frames: Vec<Value> = Vec::new();
    while let Some(line) = reader
        .next_line()
        .await
        .map_err(|e| RpcError::internal(format!("pi stdout read: {e}")))?
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(v) => frames.push(v),
            Err(e) => {
                // Pi may emit non-JSON debug lines; surface once, don't die.
                tracing::debug!(target: "makakoo.pi", raw = %trimmed, err = %e, "non-JSON stdout line");
            }
        }
    }
    Ok(frames)
}

/// Run a whole RPC turn against pi: spawn, write one request, drain
/// stdout, wait with timeout. Returns the parsed frames pi produced.
async fn rpc_turn(
    request: Value,
    timeout_s: u64,
    args: &[&str],
) -> Result<Vec<Value>, RpcError> {
    let mut child = spawn_pi_rpc(args)?;
    write_one_request(&mut child, request).await?;

    // Drain first (closes stdout when pi exits), then wait on the child.
    // We can't hold two mutable borrows across an await, so interleave
    // via a single async block.
    let result = timeout(Duration::from_secs(timeout_s), async {
        let frames = drain_stdout(&mut child).await?;
        let status = child
            .wait()
            .await
            .map_err(|e| RpcError::internal(format!("pi wait: {e}")))?;
        Ok::<_, RpcError>((frames, status))
    })
    .await;

    match result {
        Ok(Ok((frames, status))) => {
            if !status.success() {
                return Err(RpcError::internal(format!(
                    "pi --rpc exited with status {status:?}"
                )));
            }
            Ok(frames)
        }
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // kill_on_drop handles the child cleanup.
            Err(RpcError::internal(format!(
                "pi --rpc timed out after {timeout_s}s"
            )))
        }
    }
}

/// Extract the final assistant text out of a message_end frame.
fn collect_assistant_text(frames: &[Value]) -> Option<String> {
    for frame in frames.iter().rev() {
        let ty = frame.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty != "event" {
            continue;
        }
        let event = frame.get("event").and_then(|v| v.as_object())?;
        let ev_ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ev_ty != "message_end" {
            continue;
        }
        if event.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                return Some(text.to_string());
            }
            if let Some(msg) = event
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|v| v.as_str())
            {
                return Some(msg.to_string());
            }
        }
    }
    None
}

/// Extract usage totals if pi reported them.
fn collect_usage(frames: &[Value]) -> Option<Value> {
    for frame in frames.iter().rev() {
        if let Some(u) = frame.get("usage") {
            return Some(u.clone());
        }
        if let Some(u) = frame
            .get("event")
            .and_then(|v| v.get("usage"))
        {
            return Some(u.clone());
        }
    }
    None
}

fn require_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::invalid_params(format!("missing or non-string '{key}'")))
}

pub struct PiRunHandler {
    _ctx: Arc<ToolContext>,
}

impl PiRunHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { _ctx: ctx }
    }
}

#[async_trait]
impl ToolHandler for PiRunHandler {
    fn name(&self) -> &str {
        "pi_run"
    }
    fn description(&self) -> &str {
        "Run a single turn of badlogic/pi-mono as a subagent. Spawns \
         `pi --rpc`, pipes the prompt as a JSONL message, returns the \
         assistant text + usage. Use when routing a code-task-shaped \
         request that benefits from pi's fork/rewind/label sophistication."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string" },
                "session_id": { "type": "string", "description": "Optional existing session to attach to" },
                "model": { "type": "string", "description": "Provider-prefixed model id, e.g. 'switchai:ail-compound'" },
                "timeout_s": {
                    "type": "integer",
                    "default": DEFAULT_TIMEOUT_SECS,
                    "description": "Per-turn timeout; clamped to 1800s"
                }
            },
            "required": ["prompt"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let prompt = require_str(&params, "prompt")?.to_string();
        let session_id = params
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let model = params
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let timeout_s = params
            .get("timeout_s")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        let mut req = json!({
            "id": format!("mcp-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "type": "prompt",
            "message": prompt,
        });
        if let Some(sid) = session_id.as_deref() {
            req["session_id"] = json!(sid);
        }
        if let Some(m) = model.as_deref() {
            req["model"] = json!(m);
        }

        let frames = rpc_turn(req, timeout_s, &[]).await?;
        let text = collect_assistant_text(&frames)
            .unwrap_or_else(|| "<no assistant message produced>".into());
        let usage = collect_usage(&frames).unwrap_or(json!(null));

        Ok(json!({
            "text": text,
            "usage": usage,
            "frames": frames.len(),
            "session_id": session_id,
        }))
    }
}

pub struct PiSessionForkHandler {
    _ctx: Arc<ToolContext>,
}
impl PiSessionForkHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { _ctx: ctx }
    }
}
#[async_trait]
impl ToolHandler for PiSessionForkHandler {
    fn name(&self) -> &str {
        "pi_session_fork"
    }
    fn description(&self) -> &str {
        "Branch an existing pi session starting from a specific message id. \
         Non-destructive — the parent session keeps all its entries."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "from_msg_id": { "type": "string" }
            },
            "required": ["session_id", "from_msg_id"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let sid = require_str(&params, "session_id")?.to_string();
        let from = require_str(&params, "from_msg_id")?.to_string();
        let req = json!({
            "id": format!("mcp-fork-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "type": "fork",
            "session_id": sid,
            "from_msg_id": from,
        });
        let frames = rpc_turn(req, DEFAULT_TIMEOUT_SECS, &[]).await?;
        Ok(json!({
            "ok": true,
            "frames": frames,
        }))
    }
}

pub struct PiSessionLabelHandler {
    _ctx: Arc<ToolContext>,
}
impl PiSessionLabelHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { _ctx: ctx }
    }
}
#[async_trait]
impl ToolHandler for PiSessionLabelHandler {
    fn name(&self) -> &str {
        "pi_session_label"
    }
    fn description(&self) -> &str {
        "Attach a human-readable label to a pi session entry so you can \
         rewind to this point later."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "msg_id": { "type": "string" },
                "label": { "type": "string" }
            },
            "required": ["session_id", "msg_id", "label"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let sid = require_str(&params, "session_id")?.to_string();
        let msg_id = require_str(&params, "msg_id")?.to_string();
        let label = require_str(&params, "label")?.to_string();
        let req = json!({
            "id": format!("mcp-label-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "type": "label",
            "session_id": sid,
            "msg_id": msg_id,
            "label": label,
        });
        let frames = rpc_turn(req, DEFAULT_TIMEOUT_SECS, &[]).await?;
        Ok(json!({
            "ok": true,
            "frames": frames,
        }))
    }
}

pub struct PiSessionExportHandler {
    _ctx: Arc<ToolContext>,
}
impl PiSessionExportHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { _ctx: ctx }
    }
}
#[async_trait]
impl ToolHandler for PiSessionExportHandler {
    fn name(&self) -> &str {
        "pi_session_export"
    }
    fn description(&self) -> &str {
        "Export a pi session as html or md. Returns the rendered body plus \
         the path pi wrote to (if any)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "format": { "type": "string", "enum": ["html", "md"], "default": "html" }
            },
            "required": ["session_id"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let sid = require_str(&params, "session_id")?.to_string();
        let fmt = params
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("html");
        if !matches!(fmt, "html" | "md") {
            return Err(RpcError::invalid_params(format!(
                "format must be 'html' or 'md', got {fmt}"
            )));
        }
        let req = json!({
            "id": format!("mcp-export-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "type": "export",
            "session_id": sid,
            "format": fmt,
        });
        let frames = rpc_turn(req, DEFAULT_TIMEOUT_SECS, &[]).await?;
        Ok(json!({
            "ok": true,
            "format": fmt,
            "frames": frames,
        }))
    }
}

pub struct PiSetModelHandler {
    _ctx: Arc<ToolContext>,
}
impl PiSetModelHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { _ctx: ctx }
    }
}
#[async_trait]
impl ToolHandler for PiSetModelHandler {
    fn name(&self) -> &str {
        "pi_set_model"
    }
    fn description(&self) -> &str {
        "Hot-swap the model driving an in-flight pi session. The change \
         takes effect on the next turn."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "provider": { "type": "string" },
                "model_id": { "type": "string" }
            },
            "required": ["session_id", "provider", "model_id"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let sid = require_str(&params, "session_id")?.to_string();
        let provider = require_str(&params, "provider")?.to_string();
        let model_id = require_str(&params, "model_id")?.to_string();
        let req = json!({
            "id": format!("mcp-setmodel-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "type": "set_model",
            "session_id": sid,
            "provider": provider,
            "modelId": model_id,
        });
        let frames = rpc_turn(req, DEFAULT_TIMEOUT_SECS, &[]).await?;
        Ok(json!({ "ok": true, "frames": frames }))
    }
}

pub struct PiSteerHandler {
    _ctx: Arc<ToolContext>,
}
impl PiSteerHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { _ctx: ctx }
    }
}
#[async_trait]
impl ToolHandler for PiSteerHandler {
    fn name(&self) -> &str {
        "pi_steer"
    }
    fn description(&self) -> &str {
        "Inject mid-turn guidance into a running pi session. Useful when \
         the orchestrator spots the subagent going off-rails."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "message": { "type": "string" }
            },
            "required": ["session_id", "message"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let sid = require_str(&params, "session_id")?.to_string();
        let message = require_str(&params, "message")?.to_string();
        let req = json!({
            "id": format!("mcp-steer-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "type": "steer",
            "session_id": sid,
            "message": message,
        });
        let frames = rpc_turn(req, DEFAULT_TIMEOUT_SECS, &[]).await?;
        Ok(json!({ "ok": true, "frames": frames }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_assistant_text_finds_last_message_end() {
        let frames = vec![
            json!({"type": "event", "event": {"type": "message_start"}}),
            json!({"type": "event", "event": {"type": "message_end", "role": "user", "text": "hi"}}),
            json!({"type": "event", "event": {"type": "token", "text": "hello"}}),
            json!({"type": "event", "event": {"type": "message_end", "role": "assistant", "text": "ready"}}),
        ];
        assert_eq!(collect_assistant_text(&frames).as_deref(), Some("ready"));
    }

    #[test]
    fn collect_assistant_text_returns_none_without_end() {
        let frames = vec![json!({"type": "event", "event": {"type": "token"}})];
        assert!(collect_assistant_text(&frames).is_none());
    }

    #[test]
    fn collect_usage_prefers_top_level_field() {
        let frames = vec![json!({"usage": {"input": 10, "output": 20}})];
        assert_eq!(
            collect_usage(&frames),
            Some(json!({"input": 10, "output": 20}))
        );
    }

    #[test]
    fn require_str_rejects_missing_and_non_string() {
        let p = json!({"a": "ok", "b": 42});
        assert!(require_str(&p, "a").is_ok());
        assert!(require_str(&p, "b").is_err());
        assert!(require_str(&p, "missing").is_err());
    }
}
