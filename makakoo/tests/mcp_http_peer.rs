//! v0.6 Phase B — two-peer round-trip integration test.
//!
//! Spawns two `makakoo-mcp --http` subprocesses on random loopback
//! ports, generates a keypair for peer A, writes a trust file so peer
//! B trusts A, then sends a signed `tools/list` request from A → B and
//! asserts a valid MCP response comes back.
//!
//! Runs unconditionally — no external service required. If your dev
//! environment doesn't have `cargo build --bin makakoo-mcp`'ed output
//! available, the test builds it up front.

#![cfg(unix)]

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine;
use ed25519_dalek::SigningKey;
use tempfile::TempDir;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn pick_free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn makakoo_mcp_bin() -> PathBuf {
    // Prefer target/debug/makakoo-mcp built by the test's own crate.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest_dir.parent().unwrap().join("target/debug/makakoo-mcp"),
        manifest_dir.parent().unwrap().join("target/release/makakoo-mcp"),
    ];
    for c in &candidates {
        if c.is_file() {
            return c.clone();
        }
    }
    // Fallback: `cargo build` the binary into target/debug so the test
    // is self-bootstrapping when invoked cold.
    let workspace = manifest_dir.parent().unwrap();
    let status = Command::new("cargo")
        .current_dir(workspace)
        .args(["build", "--bin", "makakoo-mcp"])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .expect("cargo build");
    assert!(status.success(), "cargo build makakoo-mcp failed");
    workspace.join("target/debug/makakoo-mcp")
}

struct Peer {
    child: std::process::Child,
    port: u16,
    verifying: ed25519_dalek::VerifyingKey,
    _home: TempDir,
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn a peer. The trust file is seeded with the caller's `trusted`
/// name → pubkey before the process boots.
fn spawn_peer(
    bin: &PathBuf,
    trusted: Option<(&str, &ed25519_dalek::VerifyingKey)>,
) -> Peer {
    use makakoo_core::adapter::peer as peerlib;

    let home = TempDir::new().expect("home");
    let home_path = home.path().to_path_buf();
    std::fs::create_dir_all(home_path.join("config/peers")).unwrap();

    // Generate a keypair into the peers dir so the child inherits it
    // (rather than letting the child auto-gen; we need to know its
    // pubkey before starting).
    use rand::RngCore;
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    let signing = SigningKey::from_bytes(&secret);
    let verifying = signing.verifying_key();
    std::fs::write(
        peerlib::default_signing_key_path(&home_path),
        B64.encode(signing.to_bytes()),
    )
    .unwrap();
    std::fs::write(
        peerlib::default_signing_pub_path(&home_path),
        B64.encode(verifying.to_bytes()),
    )
    .unwrap();

    // Trust file with the peer who'll send us requests.
    if let Some((name, pubkey)) = trusted {
        let line = format!("{} {}\n", name, B64.encode(pubkey.to_bytes()));
        std::fs::write(peerlib::default_trust_file(&home_path), line).unwrap();
    }

    let port = pick_free_port();
    let child = Command::new(bin)
        .arg("--http")
        .arg(format!("127.0.0.1:{port}"))
        .env("MAKAKOO_HOME", &home_path)
        .env("MAKAKOO_PEER_SIGNING_KEY", B64.encode(signing.to_bytes()))
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn makakoo-mcp");

    // Wait for the port to accept a TCP connection.
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("peer on :{port} never accepted a connection");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    Peer {
        child,
        port,
        verifying,
        _home: home,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_peers_signed_round_trip() {
    use makakoo_core::adapter::peer as peerlib;

    let bin = makakoo_mcp_bin();

    // Caller generates its own keypair first so peer B can be started
    // with A's pubkey already in its trust file.
    use rand::RngCore;
    let mut secret_a = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret_a);
    let signing_a = SigningKey::from_bytes(&secret_a);
    let verifying_a = signing_a.verifying_key();

    // Spawn peer B trusting "peer-a".
    let peer_b = spawn_peer(&bin, Some(("peer-a", &verifying_a)));
    let _ = &peer_b.verifying; // keep field alive

    // Construct + sign a tools/list request.
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let ts = peerlib::now_millis();
    let sig = peerlib::sign_request(&signing_a, body, ts);
    let url = format!("http://127.0.0.1:{}/rpc", peer_b.port);

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header(peerlib::PEER_HEADER, "peer-a")
        .header(peerlib::TS_HEADER, ts.to_string())
        .header(
            peerlib::SIG_HEADER,
            format!("{}{}", peerlib::SIG_PREFIX, sig),
        )
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await
        .expect("POST");
    assert!(resp.status().is_success(), "status: {}", resp.status());
    let parsed: serde_json::Value = resp.json().await.unwrap();
    assert!(parsed["result"]["tools"].is_array(), "body: {parsed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_peer_rejected_with_401() {
    use makakoo_core::adapter::peer as peerlib;

    let bin = makakoo_mcp_bin();

    // Spawn peer B with an EMPTY trust file.
    let peer_b = spawn_peer(&bin, None);
    let _ = &peer_b.verifying;

    // A fresh signing key that B doesn't know about.
    use rand::RngCore;
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    let signing = SigningKey::from_bytes(&secret);

    let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let ts = peerlib::now_millis();
    let sig = peerlib::sign_request(&signing, body, ts);
    let url = format!("http://127.0.0.1:{}/rpc", peer_b.port);

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header(peerlib::PEER_HEADER, "stranger")
        .header(peerlib::TS_HEADER, ts.to_string())
        .header(
            peerlib::SIG_HEADER,
            format!("{}{}", peerlib::SIG_PREFIX, sig),
        )
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status().as_u16(), 401);
}
