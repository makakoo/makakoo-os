//! Per-plugin Unix domain socket + PID-verified accept loop.
//!
//! Spec: `spec/CAPABILITIES.md §4`. Each running plugin gets a dedicated
//! socket at `$MAKAKOO_HOME/run/plugins/<name>.sock`. The plugin
//! connects to its own socket, the kernel reads the peer's PID off the
//! socket, and every request the plugin sends is checked against the
//! plugin's declared `GrantTable` before the kernel serves the call.
//!
//! Wire protocol: newline-delimited JSON objects (one JSON per line,
//! no stream framing). Simpler than length-prefixed JSON-RPC and works
//! directly with `jq` for debugging. Every request/response conforms
//! to `CapabilityRequest` / `CapabilityResponse` below, which are a
//! minimal subset of JSON-RPC 2.0 with capability-specific error codes.
//!
//! **Phase E/2 scope** (this module):
//! - Server struct that owns the listener + GrantTable + AuditLog
//! - `serve()` async loop that accepts + spawns a handler task per conn
//! - PID verification via `libc::getsockopt` on macOS + Linux
//! - Windows + Redox: compile-time stubs that return `NotSupported`
//!   at runtime. Real Windows named-pipe impl deferred to Phase F
//!   where it's validated on a real Windows VM as part of the
//!   cross-OS installer — see `spec/CAPABILITIES.md §4.2`.
//! - Pluggable `Handler` trait so Phase E/3 can wire brain/llm/state
//!   service impls without reshaping the dispatch core
//!
//! Socket cleanup is handled by `Drop` on the listener binding + a
//! best-effort `unlink` before `bind` to avoid EADDRINUSE after a
//! crash.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::audit::{AuditEntry, AuditLog, AuditResult};
use super::grants::{GrantCheck, GrantTable};

/// Canonical socket path for a plugin under a given Makakoo home.
pub fn socket_path(makakoo_home: &Path, plugin_name: &str) -> PathBuf {
    makakoo_home
        .join("run")
        .join("plugins")
        .join(format!("{plugin_name}.sock"))
}

/// JSON-RPC-ish request body. Every call carries a `verb` (the
/// capability being exercised, e.g. `"net/http"`) and a concrete
/// `scope` (the URL, path, key name, etc. the plugin is touching).
/// `method` specifies the concrete service operation (e.g.
/// `"brain.read"`, `"http.get"`). The grant check runs on verb + scope;
/// the handler routes on method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub verb: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub correlation_id: Option<String>,
}

/// JSON-RPC-ish response body. Either `result` or `error` is set,
/// never both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityResponse {
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<CapabilityError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<serde_json::Value>,
}

impl CapabilityError {
    /// Spec §2 step 5 — deny code is `-32001`.
    pub fn denied(verb: &str, scope: &str, reason: &str) -> Self {
        Self {
            code: -32001,
            message: format!("capability denied: {verb}:{scope}"),
            data: Some(serde_json::json!({ "reason": reason })),
        }
    }

    pub fn handler(msg: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: msg.into(),
            data: None,
        }
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: msg.into(),
            data: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum SocketError {
    #[error("bind failed on {path}: {source}")]
    Bind {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("io error: {source}")]
    Io {
        #[source]
        source: std::io::Error,
    },
    #[error("pid verification failed: expected {expected}, peer reported {peer:?}")]
    PidMismatch {
        expected: u32,
        peer: Option<u32>,
    },
    #[error("capability sockets are not supported on this platform in v0.1")]
    NotSupported,
}

/// Pluggable service layer. Phase E/3 will ship concrete handlers for
/// brain/llm/state; for now we expose a trait object so tests + the
/// kernel can inject behaviour.
#[async_trait::async_trait]
pub trait CapabilityHandler: Send + Sync {
    async fn handle(
        &self,
        request: &CapabilityRequest,
        matched_scope: Option<&str>,
    ) -> Result<serde_json::Value, CapabilityError>;
}

/// A handler that accepts every call and echoes the method name +
/// matched scope. Useful for tests and for the first dogfood round of
/// `makakoo-client` against the kernel.
pub struct EchoHandler;

#[async_trait::async_trait]
impl CapabilityHandler for EchoHandler {
    async fn handle(
        &self,
        request: &CapabilityRequest,
        matched_scope: Option<&str>,
    ) -> Result<serde_json::Value, CapabilityError> {
        Ok(serde_json::json!({
            "echo": request.method,
            "verb": request.verb,
            "scope": request.scope,
            "matched": matched_scope,
        }))
    }
}

/// A capability server bound to one plugin.
///
/// Use [`CapabilityServer::bind`] to create, then [`CapabilityServer::
/// serve`] to start the accept loop. The accept loop runs as a tokio
/// task; callers can shut it down by dropping the handle.
pub struct CapabilityServer {
    socket: PathBuf,
    grants: Arc<GrantTable>,
    audit: Arc<AuditLog>,
    handler: Arc<dyn CapabilityHandler>,
    expected_pid: Option<u32>,
}

impl CapabilityServer {
    /// Construct a server. The socket is not yet bound.
    pub fn new(
        socket: PathBuf,
        grants: Arc<GrantTable>,
        audit: Arc<AuditLog>,
        handler: Arc<dyn CapabilityHandler>,
    ) -> Self {
        Self {
            socket,
            grants,
            audit,
            handler,
            expected_pid: None,
        }
    }

    /// Pin the expected client PID. When set, peers whose PID does not
    /// match are rejected at accept time. Spec §4.1: the kernel spawns
    /// the plugin, remembers its PID, and verifies on the first connect.
    pub fn expect_pid(mut self, pid: u32) -> Self {
        self.expected_pid = Some(pid);
        self
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Bind the Unix socket + spawn an accept loop. Returns a handle
    /// that cleans up the socket on drop.
    pub async fn serve(self) -> Result<ServerHandle, SocketError> {
        let listener = bind_socket(&self.socket).await?;
        info!("capability socket listening at {}", self.socket.display());

        let grants = self.grants.clone();
        let audit = self.audit.clone();
        let handler = self.handler.clone();
        let expected_pid = self.expected_pid;
        let socket_path = self.socket.clone();

        // Running connection count — purely informational for tests.
        let running = Arc::new(Mutex::new(0usize));

        let accept_running = Arc::clone(&running);
        let task = tokio::spawn(async move {
            loop {
                match accept_one(&listener).await {
                    Ok((stream, peer_pid)) => {
                        if let (Some(expected), Some(peer)) = (expected_pid, peer_pid) {
                            if expected != peer {
                                warn!(
                                    "rejecting peer pid {peer} (expected {expected}) on {}",
                                    socket_path.display()
                                );
                                continue;
                            }
                        }
                        debug!(
                            "accepted connection peer_pid={:?} on {}",
                            peer_pid,
                            socket_path.display()
                        );
                        {
                            let mut n = accept_running.lock().await;
                            *n += 1;
                        }
                        let g = Arc::clone(&grants);
                        let a = Arc::clone(&audit);
                        let h = Arc::clone(&handler);
                        let running_for_conn = Arc::clone(&accept_running);
                        tokio::spawn(async move {
                            let _ = handle_connection(stream, g, a, h).await;
                            let mut n = running_for_conn.lock().await;
                            *n = n.saturating_sub(1);
                        });
                    }
                    Err(e) => {
                        warn!("accept loop error on {}: {e}", socket_path.display());
                        // If the listener is gone (drop), exit cleanly.
                        if matches!(e.kind(), std::io::ErrorKind::NotFound) {
                            break;
                        }
                    }
                }
            }
        });

        Ok(ServerHandle {
            socket: self.socket.clone(),
            task: Some(task),
            running,
        })
    }
}

/// Drop-on-end handle to a running server. Dropping removes the socket
/// file and aborts the accept task.
pub struct ServerHandle {
    socket: PathBuf,
    task: Option<tokio::task::JoinHandle<()>>,
    running: Arc<Mutex<usize>>,
}

impl ServerHandle {
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Number of in-flight connections. For diagnostics only.
    pub async fn running_connections(&self) -> usize {
        *self.running.lock().await
    }

    /// Abort the accept loop + remove the socket file. Idempotent.
    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Bind a Unix socket at `path`, cleaning up any stale file first.
/// Creates parent dirs as needed. No-op on non-Unix platforms — they
/// hit `SocketError::NotSupported` instead.
#[cfg(unix)]
async fn bind_socket(path: &Path) -> Result<tokio::net::UnixListener, SocketError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SocketError::Io { source })?;
    }
    // Remove stale socket from a previous run.
    let _ = std::fs::remove_file(path);
    tokio::net::UnixListener::bind(path).map_err(|source| SocketError::Bind {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
async fn bind_socket(_path: &Path) -> Result<tokio::net::UnixListener, SocketError> {
    Err(SocketError::NotSupported)
}

/// Accept one incoming connection and extract the peer PID via the
/// platform-native syscall.
#[cfg(unix)]
async fn accept_one(
    listener: &tokio::net::UnixListener,
) -> Result<(tokio::net::UnixStream, Option<u32>), std::io::Error> {
    let (stream, _addr) = listener.accept().await?;
    let pid = peer_pid(&stream).ok();
    Ok((stream, pid))
}

#[cfg(not(unix))]
async fn accept_one(
    _listener: &tokio::net::UnixListener,
) -> Result<(tokio::net::UnixStream, Option<u32>), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "capability sockets require Unix",
    ))
}

/// Get the peer PID from an already-accepted Unix stream.
///
/// - macOS: `LOCAL_PEERPID` socket option on `SOL_LOCAL`
/// - Linux: `SO_PEERCRED` socket option on `SOL_SOCKET`, returns a
///   `ucred` struct whose `pid` field we extract
///
/// Returns `std::io::Error` on any syscall failure so the accept loop
/// can decide whether to reject the connection.
#[cfg(target_os = "macos")]
pub fn peer_pid(stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERPID: libc::c_int = 2;
    let fd = stream.as_raw_fd();
    let mut pid: libc::pid_t = 0;
    let mut len: libc::socklen_t = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: getsockopt writes up to `len` bytes into `&mut pid`.
    // SOL_LOCAL / LOCAL_PEERPID are documented on Darwin.
    let ret = unsafe {
        libc::getsockopt(
            fd,
            SOL_LOCAL,
            LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(pid as u32)
}

#[cfg(target_os = "linux")]
pub fn peer_pid(stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Ucred {
        pid: libc::pid_t,
        uid: libc::uid_t,
        gid: libc::gid_t,
    }
    let fd = stream.as_raw_fd();
    let mut cred = Ucred::default();
    let mut len: libc::socklen_t = std::mem::size_of::<Ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes exactly size_of::<Ucred>() bytes when
    // the option is SO_PEERCRED on SOL_SOCKET and the socket is a
    // connected Unix stream. Layout matches kernel's struct ucred.
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(cred.pid as u32)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn peer_pid(_stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "peer_pid not implemented on this platform",
    ))
}

/// Per-connection read/write loop. Reads newline-delimited JSON, checks
/// capability, dispatches to handler, writes response.
#[cfg(unix)]
async fn handle_connection(
    stream: tokio::net::UnixStream,
    grants: Arc<GrantTable>,
    audit: Arc<AuditLog>,
    handler: Arc<dyn CapabilityHandler>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(()); // peer closed
        }

        let start = std::time::Instant::now();
        let response = match serde_json::from_str::<CapabilityRequest>(line.trim()) {
            Ok(req) => dispatch(&req, &grants, &audit, &*handler, start).await,
            Err(e) => CapabilityResponse {
                id: serde_json::Value::Null,
                result: None,
                error: Some(CapabilityError::bad_request(format!(
                    "invalid request: {e}"
                ))),
            },
        };

        let bytes = serde_json::to_vec(&response).unwrap_or_else(|_| b"{}".to_vec());
        writer.write_all(&bytes).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
}

#[cfg(not(unix))]
async fn handle_connection(
    _stream: tokio::net::UnixStream,
    _grants: Arc<GrantTable>,
    _audit: Arc<AuditLog>,
    _handler: Arc<dyn CapabilityHandler>,
) -> std::io::Result<()> {
    Ok(())
}

async fn dispatch(
    req: &CapabilityRequest,
    grants: &GrantTable,
    audit: &AuditLog,
    handler: &dyn CapabilityHandler,
    start: std::time::Instant,
) -> CapabilityResponse {
    let check = grants.check(&req.verb, &req.scope);
    let (allowed, matched_scope_for_audit, response) = match &check {
        GrantCheck::Allow { matched_scope } => {
            let matched = matched_scope.clone();
            match handler.handle(req, matched_scope.as_deref()).await {
                Ok(result) => (
                    true,
                    matched,
                    CapabilityResponse {
                        id: req.id.clone(),
                        result: Some(result),
                        error: None,
                    },
                ),
                Err(err) => (
                    true,
                    matched,
                    CapabilityResponse {
                        id: req.id.clone(),
                        result: None,
                        error: Some(err),
                    },
                ),
            }
        }
        GrantCheck::Deny { reason } => (
            false,
            None,
            CapabilityResponse {
                id: req.id.clone(),
                result: None,
                error: Some(CapabilityError::denied(&req.verb, &req.scope, reason)),
            },
        ),
    };

    let entry = AuditEntry {
        ts: chrono::Utc::now(),
        plugin: grants.plugin.clone(),
        plugin_version: grants.plugin_version.clone(),
        verb: req.verb.clone(),
        scope_requested: req.scope.clone(),
        scope_granted: matched_scope_for_audit,
        result: if allowed {
            match &response.error {
                Some(_) => AuditResult::Error,
                None => AuditResult::Allowed,
            }
        } else {
            AuditResult::Denied
        },
        duration_ms: Some(start.elapsed().as_millis() as u64),
        bytes_in: None,
        bytes_out: None,
        correlation_id: req.correlation_id.clone(),
    };
    if let Err(e) = audit.append(&entry) {
        warn!(
            "audit append failed for plugin {} verb {}: {e}",
            grants.plugin, req.verb
        );
    }
    response
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::capability::verb::Verb;
    use tempfile::TempDir;
    use tokio::io::AsyncBufReadExt;

    fn grants_with(verbs: &[(&str, Vec<&str>)], plugin: &str) -> Arc<GrantTable> {
        let mut t = GrantTable::new(plugin, "1.0.0");
        for (v, scopes) in verbs {
            t.insert(Verb {
                verb: v.to_string(),
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
            });
        }
        Arc::new(t)
    }

    async fn send_request(
        socket: &Path,
        req: &CapabilityRequest,
    ) -> CapabilityResponse {
        let stream = tokio::net::UnixStream::connect(socket).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let line = serde_json::to_string(req).unwrap();
        writer.write_all(line.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        writer.flush().await.unwrap();

        let mut buf = String::new();
        reader.read_line(&mut buf).await.unwrap();
        serde_json::from_str(buf.trim()).unwrap()
    }

    #[tokio::test]
    async fn socket_allow_request_round_trip() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let audit = Arc::new(AuditLog::open_default(home).unwrap());
        let grants = grants_with(&[("brain/read", vec![])], "test-plugin");
        let sock = socket_path(home, "test-plugin");

        let server = CapabilityServer::new(
            sock.clone(),
            grants,
            audit.clone(),
            Arc::new(EchoHandler),
        );
        let handle = server.serve().await.unwrap();

        let req = CapabilityRequest {
            id: serde_json::json!(1),
            method: "brain.read".into(),
            params: serde_json::json!({}),
            verb: "brain/read".into(),
            scope: "journals/today".into(),
            correlation_id: Some("corr-1".into()),
        };
        let resp = send_request(&sock, &req).await;
        assert!(resp.error.is_none(), "expected allow, got {:?}", resp.error);
        assert!(resp.result.is_some());

        // Audit entry written.
        let raw = std::fs::read_to_string(home.join("logs/audit.jsonl")).unwrap();
        let entry: AuditEntry = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        assert_eq!(entry.result, AuditResult::Allowed);
        assert_eq!(entry.verb, "brain/read");
        assert_eq!(entry.correlation_id.as_deref(), Some("corr-1"));

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn socket_denies_when_verb_absent_from_grants() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let audit = Arc::new(AuditLog::open_default(home).unwrap());
        let grants = grants_with(&[("brain/read", vec![])], "test-plugin");
        let sock = socket_path(home, "test-plugin");

        let server = CapabilityServer::new(
            sock.clone(),
            grants,
            audit.clone(),
            Arc::new(EchoHandler),
        );
        let handle = server.serve().await.unwrap();

        let req = CapabilityRequest {
            id: serde_json::json!(2),
            method: "http.get".into(),
            params: serde_json::json!({}),
            verb: "net/http".into(),
            scope: "https://example.com/api".into(),
            correlation_id: None,
        };
        let resp = send_request(&sock, &req).await;
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32001);
        assert!(err.message.contains("net/http"));

        let raw = std::fs::read_to_string(home.join("logs/audit.jsonl")).unwrap();
        let entry: AuditEntry = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        assert_eq!(entry.result, AuditResult::Denied);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn socket_denies_when_scope_outside_grant() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let audit = Arc::new(AuditLog::open_default(home).unwrap());
        let grants = grants_with(
            &[("net/http", vec!["https://polymarket.com/*"])],
            "test-plugin",
        );
        let sock = socket_path(home, "test-plugin");

        let server = CapabilityServer::new(
            sock.clone(),
            grants,
            audit.clone(),
            Arc::new(EchoHandler),
        );
        let handle = server.serve().await.unwrap();

        let req = CapabilityRequest {
            id: serde_json::json!(3),
            method: "http.get".into(),
            params: serde_json::json!({}),
            verb: "net/http".into(),
            scope: "https://evil.example/steal".into(),
            correlation_id: None,
        };
        let resp = send_request(&sock, &req).await;
        assert!(resp.error.is_some());
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn bad_json_returns_error_not_crash() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let audit = Arc::new(AuditLog::open_default(home).unwrap());
        let grants = grants_with(&[("brain/read", vec![])], "test-plugin");
        let sock = socket_path(home, "test-plugin");

        let server = CapabilityServer::new(
            sock.clone(),
            grants,
            audit,
            Arc::new(EchoHandler),
        );
        let handle = server.serve().await.unwrap();

        let stream = tokio::net::UnixStream::connect(&sock).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        writer.write_all(b"not valid json\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut buf = String::new();
        reader.read_line(&mut buf).await.unwrap();
        let resp: CapabilityResponse = serde_json::from_str(buf.trim()).unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn peer_pid_returns_own_pid() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let audit = Arc::new(AuditLog::open_default(home).unwrap());
        let grants = grants_with(&[("brain/read", vec![])], "test-plugin");
        let sock = socket_path(home, "test-plugin");

        let server = CapabilityServer::new(
            sock.clone(),
            grants,
            audit,
            Arc::new(EchoHandler),
        );
        let handle = server.serve().await.unwrap();

        // Connect once to poke the accept loop.
        let stream = tokio::net::UnixStream::connect(&sock).await.unwrap();
        let my_pid = std::process::id();
        let peer = peer_pid(&stream).unwrap();
        // The client is this same test process.
        assert_eq!(peer, my_pid);

        drop(stream);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_removes_socket_file() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let audit = Arc::new(AuditLog::open_default(home).unwrap());
        let grants = grants_with(&[("brain/read", vec![])], "test-plugin");
        let sock = socket_path(home, "test-plugin");
        let server = CapabilityServer::new(
            sock.clone(),
            grants,
            audit,
            Arc::new(EchoHandler),
        );
        let handle = server.serve().await.unwrap();
        assert!(sock.exists());
        handle.shutdown().await;
        assert!(!sock.exists());
    }

    #[tokio::test]
    async fn expect_pid_rejects_mismatched_peer() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let audit = Arc::new(AuditLog::open_default(home).unwrap());
        let grants = grants_with(&[("brain/read", vec![])], "test-plugin");
        let sock = socket_path(home, "test-plugin");

        // Pin a PID that is NOT this test's PID. The server should
        // reject our connections at accept time.
        let bogus_pid = 1u32;
        let server = CapabilityServer::new(
            sock.clone(),
            grants,
            audit,
            Arc::new(EchoHandler),
        )
        .expect_pid(bogus_pid);
        let handle = server.serve().await.unwrap();

        // Try to send a request; the server closes the connection
        // before replying. We should get EOF, not a response.
        let stream = tokio::net::UnixStream::connect(&sock).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let req = CapabilityRequest {
            id: serde_json::json!(1),
            method: "brain.read".into(),
            params: serde_json::json!({}),
            verb: "brain/read".into(),
            scope: "x".into(),
            correlation_id: None,
        };
        let raw = serde_json::to_string(&req).unwrap();
        let _ = writer.write_all(raw.as_bytes()).await;
        let _ = writer.write_all(b"\n").await;
        let _ = writer.flush().await;
        let mut buf = String::new();
        // Either read_line returns 0 bytes (EOF) or it hangs until we
        // hit a timeout. We wrap with a 1s timeout to keep the test
        // snappy + deterministic.
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reader.read_line(&mut buf),
        )
        .await;
        // Either: EOF right away (read_line returns Ok(0)), or the
        // server never replied (timeout). Both prove the handshake
        // rejected us.
        match res {
            Ok(Ok(0)) => {}
            Err(_) => {}
            Ok(Ok(n)) if buf.trim().is_empty() => assert_eq!(n, 0),
            other => panic!("expected rejection, got {other:?}"),
        }

        handle.shutdown().await;
    }
}
