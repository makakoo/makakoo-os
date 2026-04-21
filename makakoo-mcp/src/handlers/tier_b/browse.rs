//! `harvey_browse` MCP handler — drives the local Chrome CDP harness
//! shipped by the `agent-browser-harness` plugin.
//!
//! Input:
//!     { "code": "...", "browser": "default", "timeout_s": 60 }
//!
//! The handler shells into
//! `$MAKAKOO_HOME/plugins/agent-browser-harness/.venv/bin/python
//!  run.py` with the supplied Python snippet piped through stdin
//! and `BU_NAME` exported so the harness's daemon keys its socket
//! path off a stable per-browser identifier.
//!
//! If the plugin isn't installed (or isn't started) the handler returns
//! a clear RPC error pointing the caller at `makakoo plugin install
//! agent-browser-harness`. No silent fallbacks, no partial success.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 600;

pub struct HarveyBrowseHandler {
    ctx: Arc<ToolContext>,
}

impl HarveyBrowseHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }

    fn plugin_dir(&self) -> PathBuf {
        self.ctx.home.join("plugins").join("agent-browser-harness")
    }

    fn venv_python(&self) -> PathBuf {
        self.plugin_dir().join(".venv").join("bin").join("python")
    }

    fn runner(&self) -> PathBuf {
        // run.py lives in the upstream clone under <plugin_dir>/upstream/.
        self.plugin_dir().join("upstream").join("run.py")
    }
}

#[async_trait]
impl ToolHandler for HarveyBrowseHandler {
    fn name(&self) -> &str {
        "harvey_browse"
    }
    fn description(&self) -> &str {
        "Drive the user's local Chrome via the Browser Use CDP harness. \
         Send a Python snippet; receive stdout with page info, DOM \
         queries, screenshots (base64), or scraped text. Requires \
         agent-browser-harness plugin installed and Chrome running \
         with --remote-debugging-port=9222."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["code"],
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Python snippet executed by browser-harness's run.py. Has access to every helpers.py primitive: goto, click, read, fill, screenshot, etc."
                },
                "browser": {
                    "type": "string",
                    "description": "BU_NAME for the daemon socket — lets multiple browsers be driven concurrently. Default: $BU_NAME env or \"default\"."
                },
                "timeout_s": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_SECS,
                    "description": "Per-call timeout in seconds (default 60, max 600)."
                }
            }
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let code = params
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("missing required field `code`"))?
            .to_string();

        let browser = params
            .get("browser")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .or_else(|| std::env::var("BU_NAME").ok())
            .unwrap_or_else(|| "default".into());

        let timeout_s = params
            .get("timeout_s")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        let python = self.venv_python();
        let runner = self.runner();

        if !python.is_file() {
            return Err(RpcError::internal(format!(
                "agent-browser-harness venv python missing at {}. Run `makakoo plugin install agent-browser-harness`.",
                python.display()
            )));
        }
        if !runner.is_file() {
            return Err(RpcError::internal(format!(
                "agent-browser-harness upstream run.py missing at {}. Re-run `makakoo plugin install agent-browser-harness` to refresh upstream clone.",
                runner.display()
            )));
        }

        let mut cmd = Command::new(&python);
        cmd.arg(&runner);
        cmd.env("BU_NAME", &browser);
        cmd.env("MAKAKOO_PLUGIN_DIR", self.plugin_dir());
        cmd.current_dir(self.plugin_dir().join("upstream"));
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| RpcError::internal(format!("harvey_browse spawn: {e}")))?;

        if let Some(stdin) = child.stdin.take() {
            let mut stdin = stdin;
            stdin
                .write_all(code.as_bytes())
                .await
                .map_err(|e| RpcError::internal(format!("harvey_browse stdin: {e}")))?;
        }

        let output = match timeout(Duration::from_secs(timeout_s), child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(RpcError::internal(format!(
                    "harvey_browse wait failed: {e}"
                )))
            }
            Err(_) => {
                return Err(RpcError::internal(format!(
                    "harvey_browse timed out after {timeout_s}s (code was still running)"
                )))
            }
        };

        Ok(json!({
            "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
            "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
            "exit_code": output.status.code().unwrap_or(-1),
            "browser": browser,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx_with_home(home: PathBuf) -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(home))
    }

    #[tokio::test]
    async fn name_description_and_schema_shape() {
        let h = HarveyBrowseHandler::new(ctx_with_home(PathBuf::from("/tmp")));
        assert_eq!(h.name(), "harvey_browse");
        let desc = h.description();
        assert!(!desc.is_empty());
        assert!(desc.len() >= 20);
        let schema = h.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["code"].is_object());
        assert!(schema["required"].as_array().unwrap().iter().any(|v| v == "code"));
    }

    #[tokio::test]
    async fn missing_plugin_returns_clear_rpc_error() {
        let tmp = tempdir().unwrap();
        let h = HarveyBrowseHandler::new(ctx_with_home(tmp.path().to_path_buf()));
        let err = h
            .call(json!({ "code": "print('hi')" }))
            .await
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("agent-browser-harness"),
            "error must reference plugin name: {msg}"
        );
    }

    #[tokio::test]
    async fn missing_code_rejected_as_invalid_params() {
        let tmp = tempdir().unwrap();
        let h = HarveyBrowseHandler::new(ctx_with_home(tmp.path().to_path_buf()));
        let err = h.call(json!({})).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("code"),
            "error must mention missing `code`: {msg}"
        );
    }

    #[tokio::test]
    async fn timeout_capped_at_max() {
        // Not a behavioral test (needs real python) — just pins the
        // default so a future rename of MAX_TIMEOUT_SECS stays in sync.
        assert_eq!(MAX_TIMEOUT_SECS, 600);
        assert_eq!(DEFAULT_TIMEOUT_SECS, 60);
    }
}
