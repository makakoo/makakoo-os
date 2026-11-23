//! Guard-focused unit tests for Brain `brain_vectors` model isolation,
//! stale detection, background rebuilds, and FTS fallback.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;
use crate::superbrain::folder_index::IndexModelState;
use crate::superbrain::store::SuperbrainStore;
use crate::superbrain::vector_model::{
    probe_and_reconcile_brain, search_brain_vectors_checked, BrainRebuildCoordinator,
    DEFAULT_EMBED_TRUNCATE_CHARS,
};

#[derive(Clone)]
struct BrainEmbedResponder {
    short_alias: String,
    bulk_alias: String,
    short_model: Arc<std::sync::Mutex<String>>,
    bulk_model: Arc<std::sync::Mutex<String>>,
    short_dim: Arc<AtomicUsize>,
    bulk_dim: Arc<AtomicUsize>,
    slot: Arc<AtomicUsize>,
    gate_bulk: Arc<AtomicBool>,
    gate_entered: Arc<AtomicBool>,
    gate_release: Arc<AtomicBool>,
    bulk_calls: Arc<AtomicUsize>,
}

impl BrainEmbedResponder {
    fn new(short_alias: &str, bulk_alias: &str, model: &str, dim: usize) -> Self {
        Self {
            short_alias: short_alias.to_string(),
            bulk_alias: bulk_alias.to_string(),
            short_model: Arc::new(std::sync::Mutex::new(model.to_string())),
            bulk_model: Arc::new(std::sync::Mutex::new(model.to_string())),
            short_dim: Arc::new(AtomicUsize::new(dim)),
            bulk_dim: Arc::new(AtomicUsize::new(dim)),
            slot: Arc::new(AtomicUsize::new(0)),
            gate_bulk: Arc::new(AtomicBool::new(false)),
            gate_entered: Arc::new(AtomicBool::new(false)),
            gate_release: Arc::new(AtomicBool::new(false)),
            bulk_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn switch_both(&self, model: &str, dim: usize, slot: usize) {
        *self.short_model.lock().unwrap() = model.to_string();
        *self.bulk_model.lock().unwrap() = model.to_string();
        self.short_dim.store(dim, Ordering::SeqCst);
        self.bulk_dim.store(dim, Ordering::SeqCst);
        self.slot.store(slot, Ordering::SeqCst);
    }
}

impl Respond for BrainEmbedResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let req_model = body.get("model").and_then(Value::as_str).unwrap_or("");
        let inputs = body
            .get("input")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(1);
        let is_bulk = req_model == self.bulk_alias && self.bulk_alias != self.short_alias;
        let is_probe = inputs == 1
            && body
                .get("input")
                .and_then(|v| v.get(0))
                .and_then(Value::as_str)
                == Some("probe");

        if !is_probe && (is_bulk || inputs > 1) {
            self.bulk_calls.fetch_add(1, Ordering::SeqCst);
            if self.gate_bulk.load(Ordering::SeqCst) {
                // wiremock answers on its own current-thread runtime: block with std, not tokio.
                self.gate_entered.store(true, Ordering::SeqCst);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !self.gate_release.load(Ordering::SeqCst)
                    && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }

        let (model_name, dim) = if is_bulk {
            (
                self.bulk_model.lock().unwrap().clone(),
                self.bulk_dim.load(Ordering::SeqCst),
            )
        } else {
            (
                self.short_model.lock().unwrap().clone(),
                self.short_dim.load(Ordering::SeqCst),
            )
        };
        let slot = self.slot.load(Ordering::SeqCst);
        let mut vec = vec![0.0_f32; dim];
        vec[slot % dim] = 1.0;
        let data: Vec<Value> = (0..inputs).map(|_| json!({ "embedding": vec })).collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "model": model_name,
            "data": data,
        }))
    }
}

#[test]
fn vector_search_for_model_never_compares_across_models_even_at_equal_dim() {
    let dir = TempDir::new().unwrap();
    let store = SuperbrainStore::open(&dir.path().join("sb.db")).unwrap();
    store
        .write_document("pages/a.md", "alpha memory note", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/b.md", "beta memory note", "page", json!([]))
        .unwrap();

    // Identical 4-dim vectors stored under two different models.
    store
        .store_vector_with_model("pages/a.md", &[1.0, 0.0, 0.0, 0.0], "model-a")
        .unwrap();
    store
        .store_vector_with_model("pages/b.md", &[1.0, 0.0, 0.0, 0.0], "model-b")
        .unwrap();
    store
        .set_vectors_ready("model-b", 4, "bulk-alias", Some("short-alias"))
        .unwrap();

    // Querying model-b returns ONLY pages/b.md (never pages/a.md).
    let hits_b = store
        .vector_search_for_model(&[1.0, 0.0, 0.0, 0.0], "model-b", 10)
        .unwrap();
    assert_eq!(hits_b.len(), 1);
    assert_eq!(hits_b[0].doc_id, "pages/b.md");

    // Querying model-a returns empty because active model is model-b.
    let hits_a = store
        .vector_search_for_model(&[1.0, 0.0, 0.0, 0.0], "model-a", 10)
        .unwrap();
    assert!(hits_a.is_empty());

    // Even if brain_vector_meta were cleared, vector_search never mixes distinct models.
    store
        .conn_arc()
        .lock()
        .unwrap()
        .execute("DELETE FROM brain_vector_meta", [])
        .unwrap();
    let hits_mixed = store.vector_search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
    assert!(hits_mixed.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brain_model_switch_rebuilds_in_background_and_returns_empty_vectors_until_ready() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document(
            "pages/launch.md",
            "Launch checklist requires signed release artifacts.",
            "page",
            json!([]),
        )
        .unwrap();

    let responder = BrainEmbedResponder::new("ail-embed", "ail-embed-bulk", "model-v1", 8);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed-bulk"));
    let coord = Arc::new(BrainRebuildCoordinator::new());

    let count = store
        .sync_vectors_with_embedder(
            &bulk_emb,
            Some(short_emb.model()),
            10,
            DEFAULT_EMBED_TRUNCATE_CHARS,
        )
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Baseline vector search succeeds under model-v1.
    let hits1 = search_brain_vectors_checked(
        &store,
        &short_emb,
        Some(&bulk_emb),
        &coord,
        "signed release",
        5,
    )
    .await
    .unwrap();
    assert_eq!(hits1.len(), 1);

    // Switch underlying model and gate bulk rebuild in flight.
    responder.switch_both("model-v2", 12, 3);
    responder.gate_bulk.store(true, Ordering::SeqCst);

    let during_vec = search_brain_vectors_checked(
        &store,
        &short_emb,
        Some(&bulk_emb),
        &coord,
        "signed release",
        5,
    )
    .await
    .unwrap();
    assert!(
        during_vec.is_empty(),
        "vector search must return empty during rebuild"
    );

    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bulk embed call never reached the gate");
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Rebuilding
    );

    // FTS search still works while vectors rebuild.
    let fts_hits = store.search("signed release", 5).unwrap();
    assert_eq!(fts_hits.len(), 1);
    assert_eq!(fts_hits[0].doc_id, "pages/launch.md");

    // Release rebuild and verify vector search resumes under model-v2.
    responder.gate_bulk.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    coord.wait(store.path()).await;

    let status_after = store.vector_status().unwrap();
    assert_eq!(status_after.index_state, IndexModelState::Ready);
    assert_eq!(status_after.embed_model.as_deref(), Some("model-v2"));
    assert_eq!(status_after.embed_dim, Some(12));

    let after_vec = search_brain_vectors_checked(
        &store,
        &short_emb,
        Some(&bulk_emb),
        &coord,
        "signed release",
        5,
    )
    .await
    .unwrap();
    assert_eq!(after_vec.len(), 1);
    assert_eq!(after_vec[0].doc_id, "pages/launch.md");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brain_short_vs_bulk_mismatch_blocks_vectors_and_unreachable_probe_keeps_state() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document(
            "pages/x.md",
            "Xylophone acoustic design notes.",
            "page",
            json!([]),
        )
        .unwrap();
    store
        .store_vector_with_model("pages/x.md", &[1.0, 0.0, 0.0, 0.0], "model-a")
        .unwrap();
    store
        .set_vectors_ready("model-a", 4, "bulk-alias", Some("short-alias"))
        .unwrap();

    // Unreachable probe keeps Ready state.
    let dead =
        Arc::new(EmbeddingClient::with_base_url("http://127.0.0.1:9").with_model("bulk-alias"));
    let coord = Arc::new(BrainRebuildCoordinator::new());
    probe_and_reconcile_brain(
        Arc::clone(&store),
        Some(Arc::clone(&dead)),
        dead,
        Arc::clone(&coord),
    )
    .await;
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );

    // Mismatched short vs bulk probe marks stale without spin-loop.
    let responder = BrainEmbedResponder::new("short-alias", "bulk-alias", "model-a", 4);
    *responder.bulk_model.lock().unwrap() = "model-b".to_string();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    probe_and_reconcile_brain(
        Arc::clone(&store),
        Some(Arc::clone(&short_emb)),
        Arc::clone(&bulk_emb),
        Arc::clone(&coord),
    )
    .await;
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Stale
    );
    assert!(search_brain_vectors_checked(
        &store,
        &short_emb,
        Some(&bulk_emb),
        &coord,
        "Xylophone",
        5,
    )
    .await
    .unwrap()
    .is_empty());
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);
}
