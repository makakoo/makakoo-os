//! Cross-language Gate 4 integration test.
//!
//! Spins up a real `CapabilityServer` + `CompositeHandler(state +
//! secrets)` in-process, then shells out to `python3` with the shipped
//! `makakoo-client-py` package on `sys.path`. The Python subprocess
//! exercises the capability socket through the Python client, hitting
//! the Rust server across a process boundary + a language boundary.
//!
//! This proves three things the pure-Rust e2e can't:
//!
//! 1. The newline-delimited JSON protocol is language-agnostic — the
//!    Python `json.dumps` output parses cleanly in Rust's `serde_json`
//!    and vice versa.
//! 2. A Python plugin (every plugins-core manifest is Python) can
//!    actually talk to the kernel. Without this, `plugins-core/*`
//!    stays dark once the socket layer ships.
//! 3. `$MAKAKOO_SOCKET_PATH` is the right env-var contract — the
//!    Python side reads it and connects without the kernel needing a
//!    language-specific spawn hook.
//!
//! Skips gracefully if `python3` is not on PATH (e.g. a stripped CI
//! container). Runs on every macOS + Linux host with stdlib Python 3.
//!
//! Unix-only at compile time — the Python client uses AF_UNIX sockets;
//! a Windows named-pipe Python client arrives post-v0.1.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use tokio::process::Command;

use tempfile::TempDir;

use makakoo_core::capability::{
    service::{CompositeHandler, InMemorySecretBackend, SecretHandler, StateHandler},
    socket::CapabilityServer,
    AuditLog, CapabilityHandler, GrantTable, Verb,
};

fn grants() -> Arc<GrantTable> {
    let mut t = GrantTable::new("py-plugin", "1.0.0");
    t.insert(Verb {
        verb: "state/plugin".into(),
        scopes: vec![],
    });
    t.insert(Verb {
        verb: "secrets/read".into(),
        scopes: vec!["AIL_API_KEY".into()],
    });
    Arc::new(t)
}

fn python_client_src_dir() -> PathBuf {
    // makakoo-client/tests/py_e2e_socket.rs → ../../makakoo-client-py/src
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let p = std::path::Path::new(manifest_dir)
        .parent()
        .expect("manifest dir has parent")
        .join("makakoo-client-py/src");
    p
}

async fn skip_if_no_python() -> bool {
    match Command::new("python3").arg("--version").output().await {
        Ok(o) if o.status.success() => false,
        _ => {
            eprintln!("skipping py_e2e_socket: python3 not on PATH");
            true
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn python_client_round_trip_against_rust_server() {
    if skip_if_no_python().await {
        return;
    }

    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let state_dir = home.join("state/py-plugin");
    std::fs::create_dir_all(&state_dir).unwrap();

    let audit = Arc::new(AuditLog::open_default(home).unwrap());
    let grant_table = grants();

    let secret_backend =
        Arc::new(InMemorySecretBackend::new().with("AIL_API_KEY", "sk-py-secret"));
    let composite: Arc<dyn CapabilityHandler> = Arc::new(
        CompositeHandler::new()
            .register("state", Arc::new(StateHandler::new(state_dir.clone())))
            .register("secrets", Arc::new(SecretHandler::new(secret_backend))),
    );

    let socket_path = home.join("run/plugins/py-plugin.sock");
    let server = CapabilityServer::new(
        socket_path.clone(),
        grant_table,
        audit.clone(),
        composite,
    );
    let handle = server.serve().await.unwrap();

    let py_src = python_client_src_dir();
    let py_script = r#"
import os, sys
sys.path.insert(0, os.environ['MAKAKOO_PY_CLIENT_SRC'])
from makakoo_client import Client, CapabilityDenied

c = Client.connect_from_env()

# state round trip
n = c.state_write("notes.txt", b"hello-from-python")
assert n == len(b"hello-from-python"), f"unexpected n={n}"
data = c.state_read("notes.txt")
assert data == b"hello-from-python", f"round trip mismatch: {data!r}"

# list should have the file we just wrote
entries = c.state_list()
names = sorted(e.name for e in entries)
assert "notes.txt" in names, f"missing file in list: {names}"

# allowed secret
val = c.secret_read("AIL_API_KEY")
assert val == "sk-py-secret", f"unexpected secret: {val!r}"

# denied secret — must raise CapabilityDenied
try:
    c.secret_read("POLYMARKET_API_KEY")
except CapabilityDenied as e:
    assert e.verb == "secrets/read"
    assert e.scope == "POLYMARKET_API_KEY"
    print("OK: denied as expected")
else:
    raise SystemExit("expected CapabilityDenied")

c.close()
print("PYTHON_OK")
"#;

    let output = Command::new("python3")
        .arg("-c")
        .arg(py_script)
        .env("MAKAKOO_SOCKET_PATH", &socket_path)
        .env("MAKAKOO_PY_CLIENT_SRC", &py_src)
        .output()
        .await
        .expect("python3 failed to spawn");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "python subprocess failed:\n---stdout---\n{stdout}\n---stderr---\n{stderr}"
    );
    assert!(
        stdout.contains("PYTHON_OK"),
        "python did not reach the end: stdout={stdout}"
    );

    // The Rust side's view: state dir has the file, audit log has the
    // expected entry sequence.
    let written = std::fs::read(state_dir.join("notes.txt")).unwrap();
    assert_eq!(written, b"hello-from-python");

    let raw = std::fs::read_to_string(home.join("logs/audit.jsonl")).unwrap();
    let entries: Vec<serde_json::Value> = raw
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    // write, read, list, secrets.read(ail), secrets.read(polymarket) = 5
    assert_eq!(entries.len(), 5);
    let verbs: Vec<&str> = entries
        .iter()
        .map(|e| e["verb"].as_str().unwrap())
        .collect();
    assert_eq!(
        verbs,
        vec![
            "state/plugin",
            "state/plugin",
            "state/plugin",
            "secrets/read",
            "secrets/read",
        ]
    );
    let results: Vec<&str> = entries
        .iter()
        .map(|e| e["result"].as_str().unwrap())
        .collect();
    assert_eq!(
        results,
        vec!["allowed", "allowed", "allowed", "allowed", "denied"]
    );

    handle.shutdown().await;
}
