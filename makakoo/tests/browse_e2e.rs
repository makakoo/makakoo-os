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
//!      `open -na "Google Chrome" --args --remote-debugging-port=9222 \
//!      --user-data-dir=/tmp/chrome-cdp`
//!   2. The `agent-browser-harness` plugin installed via
//!      `MAKAKOO_VENV_PYTHON=python3.13 makakoo plugin install --core \
//!       agent-browser-harness`.
//!   3. The daemon started with `BU_CDP_WS` pointing at the CDP WebSocket
//!      (derivable from `http://localhost:9222/json/version`'s
//!      `webSocketDebuggerUrl` field).
//!   4. The `makakoo-mcp` workspace binary built alongside `makakoo`
//!      (`cargo build --workspace`). To test an installed `makakoo-mcp` on
//!      `PATH` outside CI, opt in explicitly with `MAKAKOO_E2E_USE_PATH=1`.
//!
//! If any prereq is missing the test returns the clear upstream error —
//! it never silently passes.

#![cfg(unix)]

mod common;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const ENV_GATE: &str = "MAKAKOO_CHROME_E2E";
const ENV_USE_PATH: &str = "MAKAKOO_E2E_USE_PATH";
const HANDSHAKE: &str = concat!(
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"browse_e2e","version":"0"}}}"#,
    "\n",
    r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"harvey_browse","arguments":{"code":"goto(\"https://example.com\"); print(page_info())"}}}"#,
    "\n",
);

fn locate_mcp_binary() -> Result<PathBuf, String> {
    // Resolve `makakoo-mcp` directly from `CARGO_BIN_EXE_makakoo`'s parent
    // directory. Never fall back to PATH unless `MAKAKOO_E2E_USE_PATH=1` is set
    // outside CI, so an installed binary cannot silently shadow the workspace
    // build under test.
    let makakoo = PathBuf::from(env!("CARGO_BIN_EXE_makakoo"));
    let use_path = std::env::var(ENV_USE_PATH).ok().as_deref() == Some("1");
    let is_ci = common::is_ci_env();
    let path_env = std::env::var_os("PATH");
    let (bin, used_path) = common::resolve_sibling_bin_with_policy(
        &makakoo,
        "makakoo-mcp",
        use_path,
        is_ci,
        path_env.as_deref(),
    )?;
    if used_path {
        eprintln!(
            "browse_e2e: {ENV_USE_PATH}=1 enabled; using makakoo-mcp from PATH: {}",
            bin.display()
        );
    }
    Ok(bin)
}

#[test]
fn harvey_browse_round_trips_against_real_chrome() {
    if common::is_ci_env() {
        // In CI, panic immediately if the sibling workspace binary is missing
        // instead of letting the Chrome gate mask a broken binary lookup.
        locate_mcp_binary().unwrap_or_else(|err| panic!("{err}"));
    }

    if std::env::var(ENV_GATE).ok().as_deref() != Some("1") {
        eprintln!(
            "browse_e2e skipped — set {ENV_GATE}=1 + start Chrome CDP 9222 + \
             agent-browser-harness daemon to run this end-to-end"
        );
        return;
    }

    let binary = locate_mcp_binary().unwrap_or_else(|err| panic!("{err}"));
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
