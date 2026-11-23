//! Isolated integration tests verifying process-env selection between
//! `MAKAKOO_EMBED_MODEL` (short) and `MAKAKOO_BULK_EMBED_MODEL` (bulk),
//! and `SuperbrainSyncEmbedHandler` bulk embedding + reported model capture.

use std::sync::Arc;

use makakoo_core::embeddings::{EmbeddingClient, DEFAULT_EMBED_MODEL};
use makakoo_core::event_bus::PersistentEventBus;
use makakoo_core::llm::LlmClient;
use makakoo_core::sancho::{SanchoContext, SanchoHandler, SuperbrainSyncEmbedHandler};
use makakoo_core::superbrain::folder_index::IndexModelState;
use makakoo_core::superbrain::store::SuperbrainStore;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Mutex;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn env_bulk_vs_short_model_selection() {
    let _guard = ENV_LOCK.lock().await;
    let prev_short = std::env::var("MAKAKOO_EMBED_MODEL").ok();
    let prev_bulk = std::env::var("MAKAKOO_BULK_EMBED_MODEL").ok();

    std::env::remove_var("MAKAKOO_EMBED_MODEL");
    std::env::remove_var("MAKAKOO_BULK_EMBED_MODEL");
    assert_eq!(EmbeddingClient::new().model(), DEFAULT_EMBED_MODEL);
    assert_eq!(EmbeddingClient::new_bulk().model(), DEFAULT_EMBED_MODEL);

    std::env::set_var("MAKAKOO_EMBED_MODEL", "ail-embed");
    assert_eq!(EmbeddingClient::new().model(), "ail-embed");
    assert_eq!(EmbeddingClient::new_bulk().model(), "ail-embed");

    std::env::set_var("MAKAKOO_BULK_EMBED_MODEL", "ail-embed-bulk");
    assert_eq!(EmbeddingClient::new().model(), "ail-embed");
    assert_eq!(EmbeddingClient::new_bulk().model(), "ail-embed-bulk");

    match prev_short {
        Some(v) => std::env::set_var("MAKAKOO_EMBED_MODEL", v),
        None => std::env::remove_var("MAKAKOO_EMBED_MODEL"),
    }
    match prev_bulk {
        Some(v) => std::env::set_var("MAKAKOO_BULK_EMBED_MODEL", v),
        None => std::env::remove_var("MAKAKOO_BULK_EMBED_MODEL"),
    }
}

#[tokio::test]
async fn sancho_sync_embed_uses_bulk_client_and_records_reported_model() {
    let _guard = ENV_LOCK.lock().await;
    let prev_flag = std::env::var("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC").ok();
    std::env::set_var("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", "1");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .and(body_string_contains("\"bulk-alias\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "Qwen3-Embedding-0.6B",
            "data": [{ "embedding": [1.0, 0.0, 0.0, 0.0] }]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("data/Brain/journals")).unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document(
            "pages/note.md",
            "Sufficiently long note for background embedding test.",
            "page",
            json!([]),
        )
        .unwrap();
    let bus = PersistentEventBus::open(&dir.path().join("bus.db")).unwrap();
    let llm = Arc::new(LlmClient::new());
    let short_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));

    let ctx = SanchoContext::new(Arc::clone(&store), bus, llm, short_emb, home)
        .with_bulk_embeddings(bulk_emb);
    let report = SuperbrainSyncEmbedHandler::new().run(&ctx).await.unwrap();
    assert!(report.ok);
    assert!(report.message.contains("1 vectors embedded"));

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("Qwen3-Embedding-0.6B"));
    assert_eq!(status.embed_dim, Some(4));

    match prev_flag {
        Some(v) => std::env::set_var("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", v),
        None => std::env::remove_var("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC"),
    }
}
