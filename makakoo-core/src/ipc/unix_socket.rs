//! Unix-domain socket bridge for the per-slot IPC.
//!
//! Two roles:
//!   - `IpcWriter` — used by the Rust transport runtime.  Accepts
//!     `MakakooFrame` objects and writes them newline-delimited to
//!     the slot's Unix socket.  On a broken pipe (Python gateway
//!     died), the in-flight frame is dropped with a structured WARN
//!     log and the writer enters exponential-backoff reconnect on
//!     the next send.
//!   - `IpcServer` — used by the Python gateway side OR by tests.
//!     Listens on the socket path and yields per-connection readers
//!     that decode newline-delimited frames.  Rust-side tests use
//!     this to verify the wire format end-to-end.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use crate::transport::frame::MakakooFrame;
use crate::{MakakooError, Result};

const RECONNECT_INITIAL_MS: u64 = 500;
const RECONNECT_CAP_MS: u64 = 30_000;

/// Build the canonical IPC socket path for a slot:
/// `$MAKAKOO_HOME/run/agents/<slot_id>/ipc.sock`.
pub fn slot_socket_path(makakoo_home: &Path, slot_id: &str) -> PathBuf {
    makakoo_home
        .join("run")
        .join("agents")
        .join(slot_id)
        .join("ipc.sock")
}

/// Create the parent directory of the socket path with mode 0700.
pub fn ensure_socket_parent(socket_path: &Path) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(parent)?.permissions();
            perms.set_mode(0o700);
            std::fs::set_permissions(parent, perms)?;
        }
    }
    Ok(())
}

/// Writer side of the IPC.  One instance per slot (shared by all
/// the slot's transport tasks via `Arc`).
#[cfg(unix)]
pub struct IpcWriter {
    socket_path: PathBuf,
    /// Shared mutex around the connected stream.  Tokio mutex (not
    /// std) so concurrent transport tasks block their sends rather
    /// than interleave bytes on the wire.
    stream: Mutex<Option<UnixStream>>,
    /// Backoff state for the next reconnect attempt.
    backoff_ms: Mutex<u64>,
}

#[cfg(unix)]
impl IpcWriter {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            stream: Mutex::new(None),
            backoff_ms: Mutex::new(RECONNECT_INITIAL_MS),
        }
    }

    /// Connect or return the existing connection.  Honors backoff
    /// — if the previous connect failed within the backoff window
    /// we sleep before the next attempt.
    async fn ensure_connected(&self) -> std::io::Result<()> {
        let mut guard = self.stream.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let backoff = *self.backoff_ms.lock().await;
        if backoff > RECONNECT_INITIAL_MS {
            // Apply jitter — between 0.5x and 1.0x of the current
            // backoff window.
            use rand::Rng;
            let jitter = rand::thread_rng().gen_range(backoff / 2..=backoff);
            tokio::time::sleep(Duration::from_millis(jitter)).await;
        }
        match UnixStream::connect(&self.socket_path).await {
            Ok(s) => {
                *guard = Some(s);
                *self.backoff_ms.lock().await = RECONNECT_INITIAL_MS;
                Ok(())
            }
            Err(e) => {
                let mut bo = self.backoff_ms.lock().await;
                *bo = (*bo).saturating_mul(2).min(RECONNECT_CAP_MS);
                Err(e)
            }
        }
    }

    /// Send a frame.  At-most-once: any failure during the send
    /// drops the frame with a structured WARN log and closes the
    /// stream so the next call reconnects.
    pub async fn send(&self, frame: &MakakooFrame, transport_id: &str) -> Result<()> {
        if let Err(e) = self.ensure_connected().await {
            tracing::warn!(
                target: "makakoo_core::ipc",
                event = "ipc.gateway_unavailable",
                transport_id = transport_id,
                error = %e,
                drop = true,
                "ipc connect failed — frame dropped (at-most-once)"
            );
            return Ok(());
        }
        let line = frame.to_line()?;
        let mut guard = self.stream.lock().await;
        let Some(stream) = guard.as_mut() else {
            // Another task disconnected between ensure_connected and
            // this lock acquisition.  Treat as drop.
            tracing::warn!(
                target: "makakoo_core::ipc",
                event = "ipc.gateway_unavailable",
                transport_id = transport_id,
                drop = true,
                "ipc stream gone before write — frame dropped"
            );
            return Ok(());
        };
        if let Err(e) = stream.write_all(line.as_bytes()).await {
            tracing::warn!(
                target: "makakoo_core::ipc",
                event = "ipc.gateway_unavailable",
                transport_id = transport_id,
                error = %e,
                drop = true,
                "ipc write failed — frame dropped, stream reset"
            );
            *guard = None;
            return Ok(());
        }
        if let Err(e) = stream.flush().await {
            tracing::warn!(
                target: "makakoo_core::ipc",
                event = "ipc.gateway_unavailable",
                transport_id = transport_id,
                error = %e,
                drop = true,
                "ipc flush failed — frame dropped, stream reset"
            );
            *guard = None;
        }
        Ok(())
    }
}

/// Server-side acceptor.  In production the Python gateway runs the
/// listener; in Rust unit/integration tests we use this to verify
/// the wire format end-to-end.
#[cfg(unix)]
pub struct IpcServer {
    listener: UnixListener,
}

#[cfg(unix)]
impl IpcServer {
    /// Bind to `socket_path`.  If the file already exists (e.g.
    /// stale socket from a previous run), it is removed first.
    pub fn bind(socket_path: &Path) -> Result<Self> {
        ensure_socket_parent(socket_path)?;
        if socket_path.exists() {
            std::fs::remove_file(socket_path)?;
        }
        let listener = UnixListener::bind(socket_path)
            .map_err(|e| MakakooError::Io(e))?;
        Ok(Self { listener })
    }

    /// Accept one connection, returning a stream that yields
    /// decoded frames.  Caller drives the stream with
    /// `next_frame()` in a loop.
    pub async fn accept(&self) -> Result<IpcConnection> {
        let (stream, _addr) = self.listener.accept().await?;
        Ok(IpcConnection {
            reader: BufReader::new(stream),
            buf: String::new(),
        })
    }
}

#[cfg(unix)]
pub struct IpcConnection {
    reader: BufReader<UnixStream>,
    buf: String,
}

#[cfg(unix)]
impl IpcConnection {
    /// Read one newline-delimited frame.  Returns `Ok(None)` on
    /// EOF (peer closed the socket).
    pub async fn next_frame(&mut self) -> Result<Option<MakakooFrame>> {
        self.buf.clear();
        let n = self.reader.read_line(&mut self.buf).await?;
        if n == 0 {
            return Ok(None);
        }
        let line = self.buf.trim_end_matches('\n');
        if line.is_empty() {
            return Ok(None);
        }
        Ok(Some(MakakooFrame::from_line(line)?))
    }
}

/// Convenience type alias.  `Arc<IpcWriter>` is what transport
/// adapters hold and call `.send(..)` on.
#[cfg(unix)]
pub type SharedIpcWriter = Arc<IpcWriter>;

// On non-unix targets the IPC layer is a no-op stub; the agent
// process model is unix-only in v1 (LaunchAgent / systemd).  Phase 5
// adds Windows named-pipe support.
#[cfg(not(unix))]
pub struct IpcWriter;

#[cfg(not(unix))]
impl IpcWriter {
    pub fn new(_socket_path: PathBuf) -> Self {
        Self
    }
    pub async fn send(&self, _frame: &MakakooFrame, _transport_id: &str) -> Result<()> {
        Err(crate::MakakooError::Internal(
            "IPC writer is unix-only in v1".into(),
        ))
    }
}

#[cfg(not(unix))]
pub type SharedIpcWriter = Arc<IpcWriter>;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame, ThreadKind};
    use chrono::Utc;
    use tempfile::TempDir;

    fn sample_inbound() -> MakakooFrame {
        MakakooFrame::Inbound(MakakooInboundFrame {
            agent_slot_id: "secretary".into(),
            transport_id: "telegram-main".into(),
            transport_kind: "telegram".into(),
            account_id: "12345678".into(),
            conversation_id: "746496145".into(),
            sender_id: "746496145".into(),
            thread_id: None,
            thread_kind: None,
            message_id: "42".into(),
            text: "hi".into(),
            transport_timestamp: Some("1714123456".into()),
            received_at: Utc::now(),
            raw_metadata: Default::default(),
        })
    }

    #[test]
    fn slot_socket_path_layout() {
        let p = slot_socket_path(Path::new("/tmp/makakoo"), "secretary");
        assert_eq!(
            p,
            PathBuf::from("/tmp/makakoo/run/agents/secretary/ipc.sock")
        );
    }

    #[tokio::test]
    async fn end_to_end_one_frame() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("ipc.sock");

        let server = IpcServer::bind(&socket).unwrap();
        let server_handle = tokio::spawn(async move {
            let mut conn = server.accept().await.unwrap();
            conn.next_frame().await.unwrap()
        });

        // Tiny breath so the listener is accept-ready.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let writer = IpcWriter::new(socket.clone());
        writer
            .send(&sample_inbound(), "telegram-main")
            .await
            .unwrap();

        let received = server_handle.await.unwrap().unwrap();
        match received {
            MakakooFrame::Inbound(f) => {
                assert_eq!(f.transport_id, "telegram-main");
                assert_eq!(f.text, "hi");
            }
            _ => panic!("expected inbound"),
        }
    }

    #[tokio::test]
    async fn at_most_once_drops_when_gateway_absent() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("ipc.sock");
        // Don't bind a server — connect should fail.
        ensure_socket_parent(&socket).unwrap();
        let writer = IpcWriter::new(socket);
        // Per spec: drop is silent (Ok(())) — the structured WARN
        // is the visible side effect; we just confirm no error.
        let res = writer.send(&sample_inbound(), "telegram-main").await;
        assert!(res.is_ok(), "at-most-once: drop must not surface as error");
    }

    #[tokio::test]
    async fn concurrent_writers_do_not_interleave_bytes() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("ipc.sock");

        let server = IpcServer::bind(&socket).unwrap();
        let server_handle = tokio::spawn(async move {
            let mut conn = server.accept().await.unwrap();
            let mut frames = vec![];
            for _ in 0..6 {
                if let Some(f) = conn.next_frame().await.unwrap() {
                    frames.push(f);
                }
            }
            frames
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        let writer = Arc::new(IpcWriter::new(socket.clone()));

        // Two concurrent tasks slamming the same socket.
        let w1 = writer.clone();
        let w2 = writer.clone();
        let t1 = tokio::spawn(async move {
            for i in 0..3 {
                let mut frame = match sample_inbound() {
                    MakakooFrame::Inbound(f) => f,
                    _ => unreachable!(),
                };
                frame.message_id = format!("a{}", i);
                w1.send(&MakakooFrame::Inbound(frame), "telegram-main")
                    .await
                    .unwrap();
            }
        });
        let t2 = tokio::spawn(async move {
            for i in 0..3 {
                let frame = MakakooOutboundFrame {
                    transport_id: "slack-main".into(),
                    transport_kind: "slack".into(),
                    conversation_id: "D0123ABCD".into(),
                    thread_id: None,
                    thread_kind: Some(ThreadKind::SlackThread),
                    text: format!("b{}", i),
                    reply_to_message_id: None,
                };
                w2.send(&MakakooFrame::Outbound(frame), "slack-main")
                    .await
                    .unwrap();
            }
        });
        let _ = tokio::join!(t1, t2);

        let received = server_handle.await.unwrap();
        // All 6 frames decoded cleanly — no interleaved-bytes corruption.
        assert_eq!(received.len(), 6);
    }
}
