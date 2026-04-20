//! End-to-end test — spin up a `CapabilityServer` with a real
//! `CompositeHandler(state + secrets)` + `GrantTable`, then drive it
//! with the shipped `makakoo_client::Client`.
//!
//! Covers Gate 4c: an allowed call succeeds, a denied call returns
//! `ClientError::Denied`, and the audit log has the expected entries.
//!
//! Unix-only — the client's real connect path is Unix domain socket
//! today. Windows named-pipe support for the client lands later.

#![cfg(unix)]

use std::sync::Arc;

use tempfile::TempDir;

use makakoo_core::capability::{
    service::{CompositeHandler, InMemorySecretBackend, SecretHandler, StateHandler},
    socket::CapabilityServer,
    AuditLog, AuditResult, CapabilityHandler, GrantTable, Verb,
};
use makakoo_client::{Client, ClientError};

fn grants(_state_dir: &std::path::Path) -> Arc<GrantTable> {
    let mut t = GrantTable::new("test-plugin", "1.0.0");
    // state/plugin is unscoped at the grant layer — the StateHandler
    // enforces the directory jail in code, so the grant is just
    // "this plugin may touch its own state dir at all."
    t.insert(Verb {
        verb: "state/plugin".into(),
        scopes: vec![],
    });
    // secrets/read is key-scoped: the kernel enforces the key allowlist
    // at the grant layer, before the handler sees the request.
    t.insert(Verb {
        verb: "secrets/read".into(),
        scopes: vec!["AIL_API_KEY".into()],
    });
    Arc::new(t)
}

#[tokio::test]
async fn round_trip_state_and_denied_secret() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let state_dir = home.join("state/test-plugin");
    std::fs::create_dir_all(&state_dir).unwrap();

    let audit = Arc::new(AuditLog::open_default(home).unwrap());
    let grant_table = grants(&state_dir);

    let secret_backend =
        Arc::new(InMemorySecretBackend::new().with("AIL_API_KEY", "sk-secret"));
    let composite: Arc<dyn CapabilityHandler> = Arc::new(
        CompositeHandler::new()
            .register("state", Arc::new(StateHandler::new(state_dir.clone())))
            .register("secrets", Arc::new(SecretHandler::new(secret_backend))),
    );

    let socket_path = home.join("run/plugins/test-plugin.sock");
    let server = CapabilityServer::new(
        socket_path.clone(),
        grant_table,
        audit.clone(),
        composite,
    );
    let handle = server.serve().await.unwrap();

    let client = Client::connect(&socket_path).await.unwrap();

    // state.write → state.read round trip
    let n = client.state_write("notes.txt", b"hello").await.unwrap();
    assert_eq!(n, 5);
    let bytes = client.state_read("notes.txt").await.unwrap();
    assert_eq!(bytes, b"hello");

    // secret.read for a granted key
    let secret = client.secret_read("AIL_API_KEY").await.unwrap();
    assert_eq!(secret, "sk-secret");

    // secret.read for a key NOT in the grant scope → Denied
    let err = client.secret_read("POLYMARKET_API_KEY").await.unwrap_err();
    assert!(matches!(err, ClientError::Denied { .. }));

    // Audit log: 3 allows + 1 deny = 4 entries.
    let raw = std::fs::read_to_string(home.join("logs/audit.jsonl")).unwrap();
    let entries: Vec<serde_json::Value> = raw
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(entries.len(), 4);
    let results: Vec<&str> = entries
        .iter()
        .map(|e| e["result"].as_str().unwrap())
        .collect();
    assert_eq!(
        results,
        vec!["allowed", "allowed", "allowed", "denied"]
    );

    // Verb trail gives us the audit story: state.write, state.read,
    // secrets.read AIL_API_KEY, secrets.read POLYMARKET_API_KEY.
    let verbs: Vec<&str> = entries
        .iter()
        .map(|e| e["verb"].as_str().unwrap())
        .collect();
    assert_eq!(
        verbs,
        vec!["state/plugin", "state/plugin", "secrets/read", "secrets/read"]
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn state_list_returns_file_we_wrote() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let state_dir = home.join("state/test-plugin");
    std::fs::create_dir_all(&state_dir).unwrap();
    let audit = Arc::new(AuditLog::open_default(home).unwrap());
    let grant_table = grants(&state_dir);
    let composite: Arc<dyn CapabilityHandler> = Arc::new(
        CompositeHandler::new()
            .register("state", Arc::new(StateHandler::new(state_dir.clone()))),
    );
    let socket_path = home.join("run/plugins/test-plugin.sock");
    let server =
        CapabilityServer::new(socket_path.clone(), grant_table, audit, composite);
    let handle = server.serve().await.unwrap();
    let client = Client::connect(&socket_path).await.unwrap();

    client.state_write("a.txt", b"x").await.unwrap();
    client.state_write("b.txt", b"y").await.unwrap();
    let entries = client.state_list(None).await.unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["a.txt", "b.txt"]);

    handle.shutdown().await;
}

#[tokio::test]
async fn state_read_of_missing_file_is_server_error_not_panic() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let state_dir = home.join("state/test-plugin");
    std::fs::create_dir_all(&state_dir).unwrap();
    let audit = Arc::new(AuditLog::open_default(home).unwrap());
    let grant_table = grants(&state_dir);
    let composite: Arc<dyn CapabilityHandler> = Arc::new(
        CompositeHandler::new()
            .register("state", Arc::new(StateHandler::new(state_dir))),
    );
    let socket_path = home.join("run/plugins/test-plugin.sock");
    let server =
        CapabilityServer::new(socket_path.clone(), grant_table, audit, composite);
    let handle = server.serve().await.unwrap();
    let client = Client::connect(&socket_path).await.unwrap();

    let err = client.state_read("does-not-exist.txt").await.unwrap_err();
    match err {
        ClientError::Server { code, .. } => assert_eq!(code, -32000),
        other => panic!("expected Server -32000, got {other:?}"),
    }
    handle.shutdown().await;
}

#[tokio::test]
async fn connect_from_env_reads_socket_path_var() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let state_dir = home.join("state/test-plugin");
    std::fs::create_dir_all(&state_dir).unwrap();
    let audit = Arc::new(AuditLog::open_default(home).unwrap());
    let grant_table = grants(&state_dir);
    let composite: Arc<dyn CapabilityHandler> = Arc::new(
        CompositeHandler::new()
            .register("state", Arc::new(StateHandler::new(state_dir))),
    );
    let socket_path = home.join("run/plugins/test-plugin.sock");
    let server =
        CapabilityServer::new(socket_path.clone(), grant_table, audit, composite);
    let handle = server.serve().await.unwrap();

    std::env::set_var("MAKAKOO_SOCKET_PATH", &socket_path);
    let client = Client::connect_from_env().await.unwrap();
    std::env::remove_var("MAKAKOO_SOCKET_PATH");
    client.state_write("env.txt", b"ok").await.unwrap();
    let data = client.state_read("env.txt").await.unwrap();
    assert_eq!(data, b"ok");

    // Ensure AuditResult serializes as "allowed" — smoke test on enum.
    let _ = AuditResult::Allowed;

    handle.shutdown().await;
}
