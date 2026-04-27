//! Newline-delimited JSON-RPC stdio framing.
//!
//! # Wire format
//!
//! MCP stdio transport is **newline-delimited JSON**, NOT LSP-style
//! `Content-Length:` headers. One JSON object per line, terminated by `\n`.
//!
//! This is confirmed by:
//!   - the Python reference (`core/mcp/harvey_mcp.py` `send_response` /
//!     `read_message`)
//!   - T1 acceptance tests (`tests/migration/test_mcp_framing.py`) which
//!     spawn the real server as a subprocess and speak NDJSON over stdio
//!   - Real MCP clients in the wild: Claude Code, Codex, Gemini CLI,
//!     OpenCode, Cursor, Vibe, Qwen Code — all NDJSON
//!
//! The original T12 brief referenced Content-Length framing; that was a
//! misreading of the LSP spec. The T1 ground-truth tests win.
//!
//! # Malformed-JSON gotcha (T1 pinned behavior)
//!
//! The Python server returns `None` from `read_message()` on **both** EOF
//! and `JSONDecodeError`, which causes the main loop to exit cleanly on
//! the first line of malformed JSON. T1's `test_malformed_json_current_behavior`
//! asserts `rc == 0` for this case. We reproduce this behavior verbatim so
//! a future ensemble gate can flip to skip-and-continue at will.
//!
//! # Blank line tolerance
//!
//! Some clients pad messages with extra `\n`. Both the Python reference
//! and T1's `test_blank_lines_tolerated` expect blank lines to be silently
//! skipped. We do the same.

use crate::jsonrpc::{Request, Response};
use serde_json::Value;
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::warn;

/// Newline-delimited JSON-RPC reader. Generic over any async reader so
/// the main server can use `tokio::io::Stdin` while tests can use an
/// in-memory `tokio::io::DuplexStream`.
pub struct FrameReader<R> {
    inner: BufReader<R>,
}

impl<R: tokio::io::AsyncRead + Unpin> FrameReader<R> {
    pub fn new(r: R) -> Self {
        Self {
            inner: BufReader::new(r),
        }
    }

    /// Read one framed JSON-RPC message.
    ///
    /// Returns:
    ///   - `Ok(Some(Request))` on a well-formed message
    ///   - `Ok(None)` on clean EOF **or** on the first malformed JSON
    ///     line (matches Python reference — see module docs)
    ///   - `Err(_)` only on I/O errors
    pub async fn read_message(&mut self) -> io::Result<Option<Request>> {
        loop {
            let mut line = String::new();
            let n = self.inner.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None); // clean EOF
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue; // tolerate blank padding
            }
            match serde_json::from_str::<Request>(trimmed) {
                Ok(req) => return Ok(Some(req)),
                Err(e) => {
                    // Pinned behavior: exit cleanly on first parse error,
                    // matching the Python reference and T1's
                    // test_malformed_json_current_behavior.
                    warn!(
                        "parse error on line: {:?} ({})",
                        &trimmed[..trimmed.len().min(120)],
                        e
                    );
                    return Ok(None);
                }
            }
        }
    }

    /// Read one framed raw JSON value (used by tests that send
    /// partially-formed requests we still want to inspect).
    #[allow(dead_code)]
    pub async fn read_raw(&mut self) -> io::Result<Option<Value>> {
        loop {
            let mut line = String::new();
            let n = self.inner.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            return match serde_json::from_str::<Value>(trimmed) {
                Ok(v) => Ok(Some(v)),
                Err(_) => Ok(None),
            };
        }
    }
}

/// Newline-delimited JSON-RPC writer.
pub struct FrameWriter<W> {
    inner: W,
}

impl<W: tokio::io::AsyncWrite + Unpin> FrameWriter<W> {
    pub fn new(w: W) -> Self {
        Self { inner: w }
    }

    /// Write one framed response: JSON body + `\n`, then flush. Real MCP
    /// clients depend on the flush — without it, stdout stays buffered
    /// and the client hangs waiting for the response.
    pub async fn write_response(&mut self, resp: &Response) -> io::Result<()> {
        let body = serde_json::to_vec(resp).map_err(io::Error::other)?;
        self.inner.write_all(&body).await?;
        self.inner.write_all(b"\n").await?;
        self.inner.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonrpc::Response;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;

    async fn reader_from(bytes: &'static [u8]) -> FrameReader<&'static [u8]> {
        FrameReader::new(bytes)
    }

    #[tokio::test]
    async fn happy_path_single_message() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n" as &[u8];
        let mut r = FrameReader::new(input);
        let msg = r.read_message().await.unwrap().unwrap();
        assert_eq!(msg.method, "ping");
        assert_eq!(msg.id, Some(json!(1)));
        // Next read is clean EOF.
        assert!(r.read_message().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn happy_path_multi_message() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"a\"}\n\
                      {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"b\"}\n" as &[u8];
        let mut r = FrameReader::new(input);
        let m1 = r.read_message().await.unwrap().unwrap();
        let m2 = r.read_message().await.unwrap().unwrap();
        assert_eq!(m1.method, "a");
        assert_eq!(m2.method, "b");
        assert!(r.read_message().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn eof_before_any_data() {
        let mut r = reader_from(b"").await;
        assert!(r.read_message().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn blank_lines_are_tolerated() {
        // T1's test_blank_lines_tolerated scenario: three blank lines
        // followed by a real initialize message.
        let input = b"\n\n\n{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"initialize\"}\n" as &[u8];
        let mut r = FrameReader::new(input);
        let msg = r.read_message().await.unwrap().unwrap();
        assert_eq!(msg.method, "initialize");
        assert_eq!(msg.id, Some(json!(42)));
    }

    #[tokio::test]
    async fn malformed_json_returns_none() {
        // T1's test_malformed_json_current_behavior — pin Python's
        // "exit on first parse error" semantics. If a future ensemble
        // flips this to skip-and-continue, flip the assertion.
        let input = b"this is not json\n" as &[u8];
        let mut r = FrameReader::new(input);
        assert!(r.read_message().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn partial_trailing_line_without_newline() {
        // A line without a trailing \n should still be parsed when EOF
        // follows — this matches `BufReader::read_line` behavior and
        // lets small fixtures work in tests.
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}" as &[u8];
        let mut r = FrameReader::new(input);
        let msg = r.read_message().await.unwrap().unwrap();
        assert_eq!(msg.method, "ping");
    }

    #[tokio::test]
    async fn utf8_multibyte_payload_round_trips() {
        // Japanese + emoji — make sure we don't split on bytes.
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"say\",\"params\":{\"text\":\"こんにちは 🦉\"}}\n".as_bytes();
        let mut r = FrameReader::new(input);
        let msg = r.read_message().await.unwrap().unwrap();
        assert_eq!(msg.method, "say");
        assert_eq!(msg.params["text"], "こんにちは 🦉");
    }

    #[tokio::test]
    async fn writer_emits_trailing_newline_and_flushes() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = FrameWriter::new(&mut buf);
            let resp = Response::success(json!(1), json!({"ok": true}));
            w.write_response(&resp).await.unwrap();
        }
        let wire = String::from_utf8(buf).unwrap();
        assert!(wire.ends_with('\n'));
        assert!(wire.contains(r#""jsonrpc":"2.0""#));
        assert!(wire.contains(r#""id":1"#));
        assert!(wire.contains(r#""result":{"ok":true}"#));
    }

    #[tokio::test]
    async fn writer_does_not_double_encode() {
        // Regression guard: make sure we're not writing a JSON-encoded
        // string of the body (i.e., escaped quotes wrapping the whole
        // thing) — that was a real bug we've hit before.
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = FrameWriter::new(&mut buf);
            let resp = Response::failure(json!(2), -32601, "nope");
            w.write_response(&resp).await.unwrap();
        }
        let wire = String::from_utf8(buf).unwrap();
        // First character must be '{', not '"'.
        assert!(wire.starts_with('{'));
        // Valid JSON round-trip back to a generic value.
        let parsed: serde_json::Value = serde_json::from_str(wire.trim()).unwrap();
        assert_eq!(parsed["id"], json!(2));
        assert_eq!(parsed["error"]["code"], -32601);
        assert_eq!(parsed["error"]["message"], "nope");
    }

    #[tokio::test]
    async fn duplex_stream_round_trip() {
        // Exercise the reader against a real async pipe to mirror
        // production stdin/stdout usage.
        let (mut client, server) = tokio::io::duplex(4096);
        let handle = tokio::spawn(async move {
            let mut r = FrameReader::new(server);
            r.read_message().await.unwrap().unwrap()
        });
        client
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"duplex\"}\n")
            .await
            .unwrap();
        client.flush().await.unwrap();
        drop(client);
        let msg = handle.await.unwrap();
        assert_eq!(msg.method, "duplex");
    }
}
