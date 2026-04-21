//! End-to-end smoke test for `harvey_browse` — drives a real Chrome via
//! the agent-browser-harness plugin and asserts the round-trip works.
//!
//! **Gated on `MAKAKOO_CHROME_E2E=1`.** CI has no Chrome, so the default
//! cargo test run skips this. Local dogfood runs it manually with the gate
//! set. The skip-path logs a clear note so future readers know the gate
//! exists and what it unlocks.
//!
//! Prereqs when `MAKAKOO_CHROME_E2E=1` is set:
//!   1. Chrome running with `--remote-debugging-port=9222` + a separate
//!      `--user-data-dir` (so the CDP profile doesn't clash with your
//!      daily browser). The sprint doc ships a one-liner:
//!        `open -na "Google Chrome" --args --remote-debugging-port=9222 \
//!         --user-data-dir=/tmp/chrome-cdp`
//!   2. The `agent-browser-harness` plugin installed via
//!      `MAKAKOO_VENV_PYTHON=python3.13 makakoo plugin install --core \
//!       agent-browser-harness`.
//!   3. The daemon started with `BU_CDP_WS` pointing at the CDP WebSocket
//!      (derivable from `http://localhost:9222/json/version`'s
//!      `webSocketDebuggerUrl` field).
//!   4. The `makakoo-mcp` binary built (`cargo install --path makakoo-mcp`
//!      or the workspace target `target/debug/makakoo-mcp`).
//!
//! If any prereq is missing the test returns the clear upstream error —
//! it never silently passes.

#![cfg(unix)]

use std::io::Write;
use std::process::{Command, Stdio};

const ENV_GATE: &str = "MAKAKOO_CHROME_E2E";
const HANDSHAKE: &str = concat!(
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"browse_e2e","version":"0"}}}"#,
    "\n",
    r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"harvey_browse","arguments":{"code":"goto(\"https://example.com\"); print(page_info())"}}}"#,
    "\n",
);

fn locate_mcp_binary() -> Option<std::path::PathBuf> {
    // Prefer the workspace-local debug build so `cargo test` picks up
    // fresh changes without requiring a `cargo install`. Fall back to
    // `makakoo-mcp` on PATH so the test also works against an installed
    // binary (what Sebastian actually dogfoods).
    let cargo_manifest_dir: std::path::PathBuf =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = cargo_manifest_dir
        .parent()
        .expect("makakoo/ is inside workspace root");
    for rel in ["target/debug/makakoo-mcp", "target/release/makakoo-mcp"] {
        let p = workspace_root.join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join("makakoo-mcp"))
        .find(|p| p.is_file())
}

#[test]
fn harvey_browse_round_trips_against_real_chrome() {
    if std::env::var(ENV_GATE).ok().as_deref() != Some("1") {
        eprintln!(
            "browse_e2e skipped — set {ENV_GATE}=1 + start Chrome CDP 9222 + \
             agent-browser-harness daemon to run this end-to-end"
        );
        return;
    }

    let binary = locate_mcp_binary()
        .expect("makakoo-mcp binary not found — build the workspace or put it on PATH");
    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn makakoo-mcp");

    child
        .stdin
        .as_mut()
        .expect("mcp stdin missing")
        .write_all(HANDSHAKE.as_bytes())
        .expect("failed writing MCP handshake");
    // Dropping stdin by closing the child's stdin triggers EOF so the
    // server exits cleanly after emitting the two responses.
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .expect("makakoo-mcp did not exit cleanly");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("Example Domain"),
        "harvey_browse did not return example.com title.\n---stdout---\n{stdout}\n---stderr---\n{stderr}\n"
    );
    // The inner JSON is escaped inside the MCP text block — match the
    // escaped form. Non-zero would appear as `\"exit_code\":1` etc.
    assert!(
        stdout.contains("\\\"exit_code\\\":0"),
        "harvey_browse reported non-zero exit. Full stdout:\n{stdout}\n---stderr---\n{stderr}"
    );
}
