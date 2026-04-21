//! Transport layer — produces `TransportResponse { body, meta }` from a
//! manifest + prompt, no output parsing. Four transport kinds:
//!
//! - `openai-compatible` (HTTP POST /chat/completions)
//! - `subprocess` (spawn binary, capture stdout)
//! - `mcp-http` (POST JSON-RPC to the MCP endpoint)
//! - `mcp-stdio` (spawn binary, send MCP JSON-RPC over stdio)
//!
//! Env-var expansion (`${VAR}` / `$VAR`) is **only** applied to HTTP
//! headers + HTTP body + basic-auth credentials — never to URLs or
//! subprocess commands (injection risk). The one prompt substitution
//! token `{prompt}` is expanded inside subprocess argv and inside HTTP
//! body as a last pass; env expansion happens first.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::json;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::manifest::{AuthScheme, Manifest, TransportKind};

static ENV_VAR_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$\{([A-Z0-9_]+)\}|\$([A-Z0-9_]+)").expect("valid regex"));

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("required env var `{0}` is missing")]
    MissingEnv(String),
    #[error("malformed transport config in manifest: {0}")]
    BadManifest(String),
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("subprocess failed: {0}")]
    Subprocess(String),
    #[error("timeout after {secs}s")]
    Timeout { secs: u64 },
    #[error("transport I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Raw response from the transport layer. The output parser takes over
/// from here to turn `body` into a [`ValidatorResult`].
#[derive(Debug, Clone)]
pub struct TransportResponse {
    pub body: Vec<u8>,
    pub meta: ResponseMeta,
}

#[derive(Debug, Clone, Default)]
pub struct ResponseMeta {
    pub duration: Duration,
    pub http_status: Option<u16>,
    pub exit_code: Option<i32>,
}

/// Context passed to a transport call. `env` lets tests pin the env
/// without touching the real process environment.
#[derive(Debug, Clone, Default)]
pub struct CallContext {
    pub timeout_seconds: Option<u64>,
    pub env: Option<HashMap<String, String>>,
}

impl CallContext {
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_seconds = Some(secs);
        self
    }

    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = Some(env);
        self
    }

    pub fn resolve_env(&self, name: &str) -> Option<String> {
        if let Some(m) = &self.env {
            return m.get(name).cloned();
        }
        std::env::var(name).ok()
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn call(
        &self,
        manifest: &Manifest,
        prompt: &str,
        ctx: &CallContext,
    ) -> Result<TransportResponse, TransportError>;
}

/// Dispatch to the concrete transport based on `manifest.transport.kind`.
pub async fn call_transport(
    manifest: &Manifest,
    prompt: &str,
    ctx: &CallContext,
) -> Result<TransportResponse, TransportError> {
    match manifest.transport.kind {
        TransportKind::OpenAiCompatible => {
            HttpTransport::default().call(manifest, prompt, ctx).await
        }
        TransportKind::Subprocess => {
            SubprocessTransport::default().call(manifest, prompt, ctx).await
        }
        TransportKind::McpHttp => McpHttpTransport::default().call(manifest, prompt, ctx).await,
        TransportKind::McpStdio => McpStdioTransport::default().call(manifest, prompt, ctx).await,
    }
}

/// Public helper — expand `${VAR}` / `$VAR` using the call-context env
/// (or the process env when none). Missing vars yield [`TransportError::MissingEnv`].
pub fn expand_env(s: &str, ctx: &CallContext) -> Result<String, TransportError> {
    let mut out = String::new();
    let mut last = 0;
    for cap in ENV_VAR_RE.captures_iter(s) {
        let m = cap.get(0).unwrap();
        out.push_str(&s[last..m.start()]);
        let name = cap
            .get(1)
            .or_else(|| cap.get(2))
            .map(|m| m.as_str())
            .unwrap_or("");
        let value = ctx
            .resolve_env(name)
            .ok_or_else(|| TransportError::MissingEnv(name.to_string()))?;
        out.push_str(&value);
        last = m.end();
    }
    out.push_str(&s[last..]);
    Ok(out)
}

// ────────────────────────── HTTP ─────────────────────────────────

#[derive(Debug, Default)]
pub struct HttpTransport;

#[async_trait]
impl Transport for HttpTransport {
    async fn call(
        &self,
        manifest: &Manifest,
        prompt: &str,
        ctx: &CallContext,
    ) -> Result<TransportResponse, TransportError> {
        let base = manifest
            .transport
            .base_url
            .as_deref()
            .ok_or_else(|| TransportError::BadManifest("transport.base_url missing".into()))?;
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));

        // Build the chat-completions body. Prompt goes into a user message;
        // model comes from `transport.model` (can be empty; the upstream
        // picks a default in that case).
        let model = manifest.transport.model.clone().unwrap_or_default();
        let body = json!({
            "model": model,
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "stream": false
        });

        let timeout = Duration::from_secs(ctx.timeout_seconds.unwrap_or(60));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| TransportError::Http(e.to_string()))?;

        let mut req = client.post(&url).json(&body);
        req = apply_http_auth(req, manifest, ctx)?;

        let start = Instant::now();
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(TransportError::Timeout {
                    secs: timeout.as_secs(),
                });
            }
            Err(e) => return Err(TransportError::Http(e.to_string())),
        };
        let status = resp.status().as_u16();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| TransportError::Http(e.to_string()))?;
        Ok(TransportResponse {
            body: bytes.to_vec(),
            meta: ResponseMeta {
                duration: start.elapsed(),
                http_status: Some(status),
                exit_code: None,
            },
        })
    }
}

fn apply_http_auth(
    req: reqwest::RequestBuilder,
    manifest: &Manifest,
    ctx: &CallContext,
) -> Result<reqwest::RequestBuilder, TransportError> {
    match manifest.auth.scheme {
        AuthScheme::Bearer => {
            let name = manifest
                .auth
                .key_env
                .as_deref()
                .ok_or_else(|| TransportError::BadManifest("auth.key_env missing".into()))?;
            let value = ctx
                .resolve_env(name)
                .ok_or_else(|| TransportError::MissingEnv(name.to_string()))?;
            Ok(req.header("Authorization", format!("Bearer {value}")))
        }
        AuthScheme::Header => {
            let hname = manifest
                .auth
                .header_name
                .as_deref()
                .ok_or_else(|| TransportError::BadManifest("auth.header_name missing".into()))?;
            let env = manifest
                .auth
                .key_env
                .as_deref()
                .ok_or_else(|| TransportError::BadManifest("auth.key_env missing".into()))?;
            let value = ctx
                .resolve_env(env)
                .ok_or_else(|| TransportError::MissingEnv(env.to_string()))?;
            Ok(req.header(hname, value))
        }
        AuthScheme::Basic => {
            let u_env = manifest.auth.user_env.as_deref().ok_or_else(|| {
                TransportError::BadManifest("auth.user_env missing".into())
            })?;
            let p_env = manifest.auth.pass_env.as_deref().ok_or_else(|| {
                TransportError::BadManifest("auth.pass_env missing".into())
            })?;
            let user = ctx
                .resolve_env(u_env)
                .ok_or_else(|| TransportError::MissingEnv(u_env.to_string()))?;
            let pass = ctx
                .resolve_env(p_env)
                .ok_or_else(|| TransportError::MissingEnv(p_env.to_string()))?;
            let token = base64::engine::general_purpose::STANDARD
                .encode(format!("{user}:{pass}").as_bytes());
            Ok(req.header("Authorization", format!("Basic {token}")))
        }
        AuthScheme::None | AuthScheme::OAuth => Ok(req),
    }
}

// ──────────────────────── Subprocess ─────────────────────────────

#[derive(Debug, Default)]
pub struct SubprocessTransport;

#[async_trait]
impl Transport for SubprocessTransport {
    async fn call(
        &self,
        manifest: &Manifest,
        prompt: &str,
        ctx: &CallContext,
    ) -> Result<TransportResponse, TransportError> {
        if manifest.transport.command.is_empty() {
            return Err(TransportError::BadManifest(
                "transport.command must be non-empty for subprocess".into(),
            ));
        }
        // {prompt} substitution in argv (env expansion deliberately skipped
        // to avoid shell-injection via AI-crafted prompts).
        let argv: Vec<String> = manifest
            .transport
            .command
            .iter()
            .map(|a| a.replace("{prompt}", prompt))
            .collect();

        let (program, args) = argv.split_first().expect("non-empty per check above");
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        if manifest.transport.stdin {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }
        // Propagate the resolved env vars from the call-context override
        // only if explicitly provided — otherwise inherit the process env.
        if let Some(env) = &ctx.env {
            cmd.env_clear();
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let timeout = Duration::from_secs(ctx.timeout_seconds.unwrap_or(60));

        let start = Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::Subprocess(format!("spawn failed: {e}")))?;

        if manifest.transport.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes()).await?;
                drop(stdin); // EOF
            }
        }

        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(TransportError::Subprocess(format!("wait failed: {e}")));
            }
            Err(_) => {
                return Err(TransportError::Timeout {
                    secs: timeout.as_secs(),
                });
            }
        };

        Ok(TransportResponse {
            body: output.stdout,
            meta: ResponseMeta {
                duration: start.elapsed(),
                http_status: None,
                exit_code: output.status.code(),
            },
        })
    }
}

// ──────────────────────── MCP (minimal) ──────────────────────────
//
// v0.3 ships single-shot MCP handlers: send one `tools/call` request,
// return the result body. Multi-call sessions + streaming are out of
// scope for Phase B. When a real MCP adapter lands (Phase F or later),
// the shapes below are the stable extension point.

#[derive(Debug, Default)]
pub struct McpStdioTransport;

#[async_trait]
impl Transport for McpStdioTransport {
    async fn call(
        &self,
        manifest: &Manifest,
        prompt: &str,
        ctx: &CallContext,
    ) -> Result<TransportResponse, TransportError> {
        // Best-effort stdio loop: spawn, write one JSON-RPC request on
        // stdin, read single-line response on stdout. The adapter itself
        // is responsible for speaking the right dialect.
        if manifest.transport.command.is_empty() {
            return Err(TransportError::BadManifest(
                "transport.command missing for mcp-stdio".into(),
            ));
        }
        let mut cmd = Command::new(&manifest.transport.command[0]);
        cmd.args(&manifest.transport.command[1..]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(env) = &ctx.env {
            cmd.env_clear();
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let start = Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::Subprocess(format!("spawn failed: {e}")))?;

        let rpc = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "chat",
                "arguments": { "prompt": prompt }
            }
        });
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(rpc.to_string().as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            drop(stdin);
        }

        let timeout = Duration::from_secs(ctx.timeout_seconds.unwrap_or(60));
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(TransportError::Subprocess(format!("wait failed: {e}")));
            }
            Err(_) => {
                return Err(TransportError::Timeout {
                    secs: timeout.as_secs(),
                });
            }
        };

        Ok(TransportResponse {
            body: output.stdout,
            meta: ResponseMeta {
                duration: start.elapsed(),
                http_status: None,
                exit_code: output.status.code(),
            },
        })
    }
}

#[derive(Debug, Default)]
pub struct McpHttpTransport;

#[async_trait]
impl Transport for McpHttpTransport {
    async fn call(
        &self,
        manifest: &Manifest,
        prompt: &str,
        ctx: &CallContext,
    ) -> Result<TransportResponse, TransportError> {
        let url = manifest
            .transport
            .url
            .as_deref()
            .ok_or_else(|| TransportError::BadManifest("transport.url missing".into()))?;
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "chat",
                "arguments": { "prompt": prompt }
            }
        });
        let timeout = Duration::from_secs(ctx.timeout_seconds.unwrap_or(60));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| TransportError::Http(e.to_string()))?;
        let mut req = client.post(url).json(&body);
        req = apply_http_auth(req, manifest, ctx)?;
        let start = Instant::now();
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(TransportError::Timeout {
                    secs: timeout.as_secs(),
                });
            }
            Err(e) => return Err(TransportError::Http(e.to_string())),
        };
        let status = resp.status().as_u16();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| TransportError::Http(e.to_string()))?;
        Ok(TransportResponse {
            body: bytes.to_vec(),
            meta: ResponseMeta {
                duration: start.elapsed(),
                http_status: Some(status),
                exit_code: None,
            },
        })
    }
}

// ──────────────────────────── Tests ──────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_env_basic() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let ctx = CallContext::default().with_env(env);
        let out = expand_env("value-${FOO}-and-$FOO-end", &ctx).unwrap();
        assert_eq!(out, "value-bar-and-bar-end");
    }

    #[test]
    fn expand_env_missing_errors() {
        let ctx = CallContext::default().with_env(HashMap::new());
        let err = expand_env("x-${MISSING}-y", &ctx).unwrap_err();
        assert!(matches!(err, TransportError::MissingEnv(s) if s == "MISSING"));
    }

    #[test]
    fn expand_env_leaves_non_matches_alone() {
        let ctx = CallContext::default().with_env(HashMap::new());
        let out = expand_env("no-vars-here", &ctx).unwrap();
        assert_eq!(out, "no-vars-here");
    }

    #[test]
    fn expand_env_multiple() {
        let mut env = HashMap::new();
        env.insert("A".to_string(), "1".to_string());
        env.insert("B".to_string(), "2".to_string());
        let ctx = CallContext::default().with_env(env);
        let out = expand_env("${A}${B}${A}", &ctx).unwrap();
        assert_eq!(out, "121");
    }
}
