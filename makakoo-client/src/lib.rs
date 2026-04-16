//! `makakoo-client` — Rust client for the Makakoo kernel capability socket.
//!
//! Plugins written in Rust link against this crate and call typed
//! methods like `client.state_write(path, bytes).await?` instead of
//! hand-rolling the JSON-RPC envelope over the Unix socket. The client
//! reads `$MAKAKOO_SOCKET_PATH` from env, connects over Unix domain
//! socket, and exposes the Phase E/3 method surface: state (read,
//! write, list, delete) and secrets (read).
//!
//! Brain and LLM bindings land in the E/3b slice alongside their
//! handler implementations — those need careful wiring to the
//! existing Brain + LLM subsystems. This crate's API surface is
//! append-only: older plugins keep compiling as new methods arrive.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("capability socket path not set (checked $MAKAKOO_SOCKET_PATH)")]
    NoSocketPath,
    #[error("io error: {source}")]
    Io {
        #[source]
        source: std::io::Error,
    },
    #[error("json error: {source}")]
    Json {
        #[source]
        source: serde_json::Error,
    },
    #[error("bad base64 from server: {source}")]
    Base64 {
        #[source]
        source: base64::DecodeError,
    },
    #[error("capability denied {verb}:{scope}: {reason}")]
    Denied {
        verb: String,
        scope: String,
        reason: String,
    },
    #[error("server error (code {code}): {message}")]
    Server { code: i32, message: String },
    #[error("server closed connection before replying")]
    Disconnected,
    #[error("server returned neither result nor error")]
    BadResponse,
}

#[derive(Debug, Serialize)]
struct Request<'a> {
    id: u64,
    method: &'a str,
    params: serde_json::Value,
    verb: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    scope: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Response {
    #[allow(dead_code)]
    id: serde_json::Value,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<ResponseError>,
}

#[derive(Debug, Deserialize)]
struct ResponseError {
    code: i32,
    message: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// A connected client handle. Every RPC serializes through an internal
/// lock so concurrent calls from the same `Client` don't interleave
/// writes on the socket.
pub struct Client {
    socket: PathBuf,
    stream: Mutex<Connection>,
    next_id: AtomicU64,
    correlation_id: Option<String>,
}

struct Connection {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl Client {
    /// Connect to the socket at `path`.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, ClientError> {
        let path = path.as_ref().to_path_buf();
        let stream = UnixStream::connect(&path)
            .await
            .map_err(|source| ClientError::Io { source })?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            socket: path,
            stream: Mutex::new(Connection {
                reader: BufReader::new(reader),
                writer,
            }),
            next_id: AtomicU64::new(1),
            correlation_id: None,
        })
    }

    /// Connect using `$MAKAKOO_SOCKET_PATH`. Typical for plugin processes
    /// spawned by the kernel — the kernel exports this env var before
    /// launching the plugin.
    pub async fn connect_from_env() -> Result<Self, ClientError> {
        let path = std::env::var("MAKAKOO_SOCKET_PATH")
            .map_err(|_| ClientError::NoSocketPath)?;
        Self::connect(path).await
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Attach a correlation id that will be included on every subsequent
    /// request. Useful for tracing a multi-step plugin action through
    /// the audit log.
    pub fn with_correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }

    async fn call(
        &self,
        method: &str,
        verb: &str,
        scope: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = Request {
            id,
            method,
            params,
            verb,
            scope,
            correlation_id: self.correlation_id.clone(),
        };
        let mut guard = self.stream.lock().await;

        let line =
            serde_json::to_string(&req).map_err(|source| ClientError::Json { source })?;
        guard
            .writer
            .write_all(line.as_bytes())
            .await
            .map_err(|source| ClientError::Io { source })?;
        guard
            .writer
            .write_all(b"\n")
            .await
            .map_err(|source| ClientError::Io { source })?;
        guard
            .writer
            .flush()
            .await
            .map_err(|source| ClientError::Io { source })?;

        let mut buf = String::new();
        let n = guard
            .reader
            .read_line(&mut buf)
            .await
            .map_err(|source| ClientError::Io { source })?;
        if n == 0 {
            return Err(ClientError::Disconnected);
        }
        let resp: Response = serde_json::from_str(buf.trim())
            .map_err(|source| ClientError::Json { source })?;

        if let Some(err) = resp.error {
            if err.code == -32001 {
                // Capability denied — surface as its own variant so
                // plugins can distinguish "I shouldn't have tried that"
                // from real server trouble.
                let reason = err
                    .data
                    .as_ref()
                    .and_then(|d| d.get("reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return Err(ClientError::Denied {
                    verb: verb.to_string(),
                    scope: scope.to_string(),
                    reason,
                });
            }
            return Err(ClientError::Server {
                code: err.code,
                message: err.message,
            });
        }
        resp.result.ok_or(ClientError::BadResponse)
    }

    // ── state ─────────────────────────────────────────────────────────

    /// Read bytes from `path` under the plugin's state dir.
    pub async fn state_read(&self, path: &str) -> Result<Vec<u8>, ClientError> {
        let v = self
            .call(
                "state.read",
                "state/plugin",
                "",
                serde_json::json!({ "path": path }),
            )
            .await?;
        let b64 = v
            .get("bytes_b64")
            .and_then(|x| x.as_str())
            .ok_or(ClientError::BadResponse)?;
        BASE64
            .decode(b64.as_bytes())
            .map_err(|source| ClientError::Base64 { source })
    }

    /// Write `bytes` to `path` under the plugin's state dir.
    pub async fn state_write(
        &self,
        path: &str,
        bytes: &[u8],
    ) -> Result<usize, ClientError> {
        let v = self
            .call(
                "state.write",
                "state/plugin",
                "",
                serde_json::json!({
                    "path": path,
                    "bytes_b64": BASE64.encode(bytes),
                }),
            )
            .await?;
        Ok(v.get("bytes_written")
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as usize)
    }

    /// List files + dirs one level under `path` (root state dir if None).
    pub async fn state_list(
        &self,
        path: Option<&str>,
    ) -> Result<Vec<StateEntry>, ClientError> {
        let params = match path {
            Some(p) => serde_json::json!({ "path": p }),
            None => serde_json::json!({}),
        };
        let v = self.call("state.list", "state/plugin", "", params).await?;
        let entries = v
            .get("entries")
            .and_then(|x| x.as_array())
            .ok_or(ClientError::BadResponse)?;
        let mut out = Vec::new();
        for e in entries {
            let name = e
                .get("name")
                .and_then(|s| s.as_str())
                .ok_or(ClientError::BadResponse)?
                .to_string();
            let is_dir = e
                .get("is_dir")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            out.push(StateEntry { name, is_dir });
        }
        Ok(out)
    }

    /// Remove `path` from the plugin's state dir. Returns whether
    /// something was removed.
    pub async fn state_delete(&self, path: &str) -> Result<bool, ClientError> {
        let v = self
            .call(
                "state.delete",
                "state/plugin",
                "",
                serde_json::json!({ "path": path }),
            )
            .await?;
        Ok(v.get("removed")
            .and_then(|b| b.as_bool())
            .unwrap_or(false))
    }

    // ── secrets ──────────────────────────────────────────────────────

    /// Read a secret value by key name. The plugin must declare
    /// `secrets/read:<NAME>` in its manifest — requests for undeclared
    /// keys return `ClientError::Denied`.
    pub async fn secret_read(&self, name: &str) -> Result<String, ClientError> {
        let v = self
            .call(
                "secrets.read",
                "secrets/read",
                name,
                serde_json::json!({ "name": name }),
            )
            .await?;
        let s = v
            .get("value")
            .and_then(|x| x.as_str())
            .ok_or(ClientError::BadResponse)?;
        Ok(s.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct StateEntry {
    pub name: String,
    pub is_dir: bool,
}
