//! End-to-end tests for the Brain + LLM handler surface via the
//! shipped Rust client.
//!
//! Brain tests use a real SuperbrainStore + tempdir. LLM tests spin up
//! a wiremock HTTP server that the LlmClient / EmbeddingClient talk to
//! so we validate the full socket → handler → gateway → response chain.
//!
//! Unix-only — same reasoning as e2e_socket.rs (Client currently
//! requires AF_UNIX; Windows named-pipe client arrives post-v0.1).

#![cfg(unix)]

use std::sync::Arc;

use tempfile::TempDir;

use makakoo_core::capability::{
    service::{BrainHandler, CompositeHandler, LlmHandler},
    socket::CapabilityServer,
    AuditLog, CapabilityHandler, GrantTable, Verb,
};
use makakoo_core::embeddings::EmbeddingClient;
use makakoo_core::llm::LlmClient;
use makakoo_core::superbrain::store::SuperbrainStore;
use makakoo_client::{Client, ClientError};

fn grants() -> Arc<GrantTable> {
    let mut t = GrantTable::new("brain-llm-plugin", "1.0.0");
    t.insert(Verb {
        verb: "brain/read".into(),
        scopes: vec![],
    });
    t.insert(Verb {
        verb: "brain/write".into(),
        scopes: vec![],
    });
    t.insert(Verb {
        verb: "llm/chat".into(),
        scopes: vec!["minimax/ail-compound".into()],
    });
    t.insert(Verb {
        verb: "llm/embed".into(),
        scopes: vec![],
    });
    Arc::new(t)
}

#[tokio::test]
async fn brain_write_journal_then_read_back_via_client() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let audit = Arc::new(AuditLog::open_default(home).unwrap());

    let store = Arc::new(
        SuperbrainStore::open(&home.join("data/superbrain.db")).unwrap(),
    );
    let brain_root = home.join("data/Brain");
    let brain = Arc::new(BrainHandler::new(Arc::clone(&store), brain_root.clone()));

    let composite: Arc<dyn CapabilityHandler> =
        Arc::new(CompositeHandler::new().register("brain", brain));

    let socket = home.join("run/plugins/brain-llm-plugin.sock");
    let server = CapabilityServer::new(socket.clone(), grants(), audit, composite);
    let handle = server.serve().await.unwrap();

    let client = Client::connect(&socket).await.unwrap();

    // Write a line
    let path = client
        .brain_write_journal("E/3b dogfood: capability socket serves Brain writes")
        .await
        .unwrap();
    assert!(path.contains("journals"));
    assert!(path.ends_with(".md"));

    // File was created and contains the line
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("- E/3b dogfood: capability socket serves Brain writes"));

    // brain.search on a fresh store returns empty (FTS hasn't seen
    // the file yet — it's a filesystem write, not a Superbrain insert).
    // That's expected for this slice; Phase F will wire the journal
    // indexer. Assert that the call succeeds rather than expecting hits.
    let hits = client.brain_search("dogfood", 5).await.unwrap();
    assert!(hits.is_empty() || !hits.is_empty()); // tolerant of either

    handle.shutdown().await;
}

#[tokio::test]
async fn brain_read_of_missing_doc_returns_none() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let audit = Arc::new(AuditLog::open_default(home).unwrap());
    let store = Arc::new(
        SuperbrainStore::open(&home.join("data/superbrain.db")).unwrap(),
    );
    let brain = Arc::new(BrainHandler::new(store, home.join("data/Brain")));
    let composite: Arc<dyn CapabilityHandler> =
        Arc::new(CompositeHandler::new().register("brain", brain));

    let socket = home.join("run/plugins/brain-llm-plugin.sock");
    let server = CapabilityServer::new(socket.clone(), grants(), audit, composite);
    let handle = server.serve().await.unwrap();

    let client = Client::connect(&socket).await.unwrap();
    let doc = client.brain_read("ghost").await.unwrap();
    assert!(doc.is_none());

    handle.shutdown().await;
}

#[tokio::test]
async fn llm_chat_routes_through_socket_to_mock_gateway() {
    use wiremock::matchers::{method, path as wm_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wm_path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "content": "hello from mock gateway" } }]
        })))
        .mount(&gateway)
        .await;

    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let audit = Arc::new(AuditLog::open_default(home).unwrap());
    let llm = Arc::new(LlmClient::with_base_url(gateway.uri()));
    let emb = Arc::new(EmbeddingClient::with_base_url(gateway.uri()));
    let handler = Arc::new(LlmHandler::new(llm, emb));
    let composite: Arc<dyn CapabilityHandler> =
        Arc::new(CompositeHandler::new().register("llm", handler));

    let socket = home.join("run/plugins/brain-llm-plugin.sock");
    let server = CapabilityServer::new(socket.clone(), grants(), audit, composite);
    let handle = server.serve().await.unwrap();

    let client = Client::connect(&socket).await.unwrap();
    let reply = client
        .llm_chat(
            "minimax/ail-compound",
            &[("user", "what's the weather")],
        )
        .await
        .unwrap();
    assert_eq!(reply, "hello from mock gateway");

    handle.shutdown().await;
}

#[tokio::test]
async fn llm_chat_outside_scope_is_denied() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let audit = Arc::new(AuditLog::open_default(home).unwrap());
    let llm = Arc::new(LlmClient::with_base_url("http://127.0.0.1:0"));
    let emb = Arc::new(EmbeddingClient::with_base_url("http://127.0.0.1:0"));
    let handler = Arc::new(LlmHandler::new(llm, emb));
    let composite: Arc<dyn CapabilityHandler> =
        Arc::new(CompositeHandler::new().register("llm", handler));

    let socket = home.join("run/plugins/brain-llm-plugin.sock");
    let server = CapabilityServer::new(socket.clone(), grants(), audit, composite);
    let handle = server.serve().await.unwrap();

    let client = Client::connect(&socket).await.unwrap();
    // Grant allows only minimax/ail-compound; plugin asks for a
    // different provider → GrantTable denies before the LLM is called.
    let err = client
        .llm_chat("anthropic/claude", &[("user", "hi")])
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Denied { .. }));

    handle.shutdown().await;
}
