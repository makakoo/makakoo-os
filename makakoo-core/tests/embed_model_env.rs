//! Isolated integration tests verifying process-env selection between
//! `MAKAKOO_EMBED_MODEL` (short) and `MAKAKOO_BULK_EMBED_MODEL` (bulk),
//! and `SuperbrainSyncEmbedHandler` bulk embedding + reported model capture.

use std::sync::{Arc, Mutex as StdMutex};

use makakoo_core::embeddings::{
    EmbeddingClient, DEFAULT_EMBED_MODEL, EMBED_MAX_BATCH_ENV, EMBED_MAX_INPUT_BYTES_ENV,
};
use makakoo_core::event_bus::PersistentEventBus;
use makakoo_core::llm::LlmClient;
use makakoo_core::sancho::{SanchoContext, SanchoHandler, SuperbrainSyncEmbedHandler};
use makakoo_core::superbrain::folder_index::IndexModelState;
use makakoo_core::superbrain::store::SuperbrainStore;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::Mutex;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

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
    assert!(
        report.message.contains("rebuild scheduled"),
        "expected scheduled background rebuild, got: {}",
        report.message
    );
    ctx.brain_rebuild.wait(store.path()).await;

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("Qwen3-Embedding-0.6B"));
    assert_eq!(status.embed_dim, Some(4));
    assert_eq!(store.stats().unwrap().vector_count, 1);

    match prev_flag {
        Some(v) => std::env::set_var("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", v),
        None => std::env::remove_var("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC"),
    }
}

struct EnvRestore(Vec<(&'static str, Option<String>)>);

impl EnvRestore {
    fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in pairs {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        Self(saved)
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (k, v) in &self.0 {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

#[derive(Clone)]
struct EnvLimitResponder {
    max_batch: usize,
    max_bytes: usize,
    batches: Arc<StdMutex<Vec<usize>>>,
    received: Arc<StdMutex<Vec<String>>>,
}

impl Respond for EnvLimitResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let arr = body
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if arr.len() > self.max_batch {
            return ResponseTemplate::new(400).set_body_json(json!({ "error": "batch too big" }));
        }
        let mut data = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let text = item.as_str().unwrap_or("");
            if text.len() > self.max_bytes {
                return ResponseTemplate::new(400)
                    .set_body_json(json!({ "error": "input too big" }));
            }
            self.received.lock().unwrap().push(text.to_string());
            let tag = text
                .split(':')
                .next()
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(-1.0);
            data.push(json!({ "index": i, "embedding": [tag, 1.0] }));
        }
        self.batches.lock().unwrap().push(arr.len());
        ResponseTemplate::new(200).set_body_json(json!({ "model": "env-model", "data": data }))
    }
}

#[tokio::test]
async fn env_embed_max_batch_and_input_bytes_override() {
    let _guard = ENV_LOCK.lock().await;
    let _env = EnvRestore::set(&[
        (EMBED_MAX_BATCH_ENV, Some("5")),
        (EMBED_MAX_INPUT_BYTES_ENV, Some("120")),
    ]);

    let responder = EnvLimitResponder {
        max_batch: 5,
        max_bytes: 120,
        batches: Arc::new(StdMutex::new(Vec::new())),
        received: Arc::new(StdMutex::new(Vec::new())),
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let texts: Vec<String> = (0..12)
        .map(|i| format!("{i}:{}", "Zażółć Prüfung ".repeat(20)))
        .collect();
    let client = EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed");
    let res = client.embed_batch_with_meta(&texts).await.unwrap();

    assert_eq!(res.vectors.len(), 12);
    assert_eq!(responder.batches.lock().unwrap().as_slice(), &[5, 5, 2]);
    for (i, vec) in res.vectors.iter().enumerate() {
        assert_eq!(vec[0], i as f32);
    }
    for (orig, sent) in texts.iter().zip(responder.received.lock().unwrap().iter()) {
        assert!(sent.len() <= 120);
        assert!(orig.starts_with(sent.as_str()));
    }
}
