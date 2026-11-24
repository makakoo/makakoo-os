//! The Brain's top-up path and the cross-process rebuild claim (review r1 HIGH:
//! `sync_vectors_with_embedder` was a second, uncoordinated rebuild path).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;
use crate::superbrain::folder_index::model_meta::SHORT_BULK_MISMATCH_REASON;
use crate::superbrain::folder_index::IndexModelState;
use crate::superbrain::graph::GraphStore;
use crate::superbrain::ingest::IngestEngine;
use crate::superbrain::store::SuperbrainStore;
use crate::superbrain::vector_model::{
    rebuild_brain_vectors, BrainRebuildCoordinator, DEFAULT_EMBED_TRUNCATE_CHARS,
    EMBED_MODEL_CHANGED_REASON,
};

/// One endpoint for every alias: answers with `model` at `dim`, counts calls.
#[derive(Clone)]
struct Endpoint {
    model: Arc<Mutex<String>>,
    dim: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl Respond for Endpoint {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let inputs = body
            .get("input")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(1);
        let mut vec = vec![0.0_f32; self.dim.load(Ordering::SeqCst)];
        vec[0] = 1.0;
        let data: Vec<Value> = (0..inputs).map(|_| json!({ "embedding": vec })).collect();
        ResponseTemplate::new(200)
            .set_body_json(json!({ "model": self.model.lock().unwrap().clone(), "data": data }))
    }
}

struct Rig {
    _dir: TempDir,
    home: TempDir,
    store: Arc<SuperbrainStore>,
    endpoint: Endpoint,
    _server: MockServer,
    bulk: Arc<EmbeddingClient>,
}

async fn rig(model: &str, docs: &[&str]) -> Rig {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    for (i, text) in docs.iter().enumerate() {
        store
            .write_document(&format!("pages/{i}.md"), text, "page", json!([]))
            .unwrap();
    }
    let endpoint = Endpoint {
        model: Arc::new(Mutex::new(model.to_string())),
        dim: Arc::new(AtomicUsize::new(8)),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(endpoint.clone())
        .mount(&server)
        .await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed-bulk"));
    Rig {
        _dir: dir,
        home: TempDir::new().unwrap(),
        store,
        endpoint,
        _server: server,
        bulk,
    }
}

fn vector_models(store: &SuperbrainStore) -> Vec<Option<String>> {
    let conn = store.conn_arc();
    let conn = conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT model FROM brain_vectors ORDER BY doc_id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| r.get::<_, Option<String>>(0))
        .unwrap();
    rows.map(Result::unwrap).collect()
}

async fn top_up(r: &Rig) -> usize {
    r.store
        .sync_vectors_with_embedder(&r.bulk, 10, DEFAULT_EMBED_TRUNCATE_CHARS)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_up_writes_nothing_until_the_index_is_ready() {
    let r = rig("model-1", &["alpha"]).await;
    assert_eq!(top_up(&r).await, 0);
    assert!(vector_models(&r.store).is_empty());
    assert_eq!(r.endpoint.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_up_fills_missing_documents_under_the_active_model() {
    let r = rig("model-1", &["alpha"]).await;
    rebuild_brain_vectors(Arc::clone(&r.store), Arc::clone(&r.bulk), None, None)
        .await
        .unwrap();
    r.store
        .write_document("pages/new.md", "beta", "page", json!([]))
        .unwrap();
    assert_eq!(top_up(&r).await, 1);
    let models = vector_models(&r.store);
    assert_eq!(models.len(), 2);
    assert!(models.iter().all(|m| m.as_deref() == Some("model-1")));
    assert_eq!(
        r.store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_up_hands_a_model_change_to_the_rebuild_and_writes_nothing() {
    let r = rig("model-1", &["alpha"]).await;
    rebuild_brain_vectors(Arc::clone(&r.store), Arc::clone(&r.bulk), None, None)
        .await
        .unwrap();
    r.store
        .write_document("pages/new.md", "beta", "page", json!([]))
        .unwrap();
    *r.endpoint.model.lock().unwrap() = "model-2".to_string();
    assert_eq!(top_up(&r).await, 0);
    let status = r.store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(
        status.stale_reason.as_deref(),
        Some(EMBED_MODEL_CHANGED_REASON)
    );
    assert_eq!(vector_models(&r.store), vec![Some("model-1".to_string())]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_top_up_write_never_lands_inside_a_running_rebuild() {
    let r = rig("model-1", &["alpha"]).await;
    rebuild_brain_vectors(Arc::clone(&r.store), Arc::clone(&r.bulk), None, None)
        .await
        .unwrap();
    r.store
        .write_document("pages/new.md", "beta", "page", json!([]))
        .unwrap();
    assert!(r
        .store
        .try_mark_vectors_rebuilding("other_process")
        .unwrap());
    let vec = vec![1.0_f32; 8];
    assert!(!r
        .store
        .store_vector_if_ready_for("pages/new.md", &vec, "model-1", 8)
        .unwrap());
    assert_eq!(top_up(&r).await, 0);
    assert_eq!(vector_models(&r.store).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_one_rebuild_claim_wins_and_the_coordinator_respects_it() {
    let r = rig("model-1", &["alpha"]).await;
    assert!(r.store.try_mark_vectors_rebuilding("cli_rebuild").unwrap());
    assert!(!r.store.try_mark_vectors_rebuilding("cli_rebuild").unwrap());
    let coord = Arc::new(BrainRebuildCoordinator::new());
    coord.spawn_rebuild(Arc::clone(&r.store), Arc::clone(&r.bulk), None, None);
    assert!(!coord.is_active(r.store.path()));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(r.endpoint.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        r.store.vector_status().unwrap().stale_reason.as_deref(),
        Some("cli_rebuild")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_embed_pending_fills_a_fresh_brain_through_the_guarded_rebuild() {
    let r = rig("model-1", &["alpha", "beta"]).await;
    let graph = Arc::new(GraphStore::new(r.store.conn_arc()));
    let engine = IngestEngine::new(Arc::clone(&r.store), graph, r.home.path());
    assert_eq!(
        engine
            .embed_pending(Arc::clone(&r.bulk), Some(Arc::clone(&r.bulk)), 10)
            .await
            .unwrap(),
        2
    );
    let status = r.store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_embed_pending_does_not_rebuild_over_a_query_model_mismatch() {
    let r = rig("model-1", &["alpha"]).await;
    r.store
        .mark_vectors_stale(SHORT_BULK_MISMATCH_REASON)
        .unwrap();
    let graph = Arc::new(GraphStore::new(r.store.conn_arc()));
    let engine = IngestEngine::new(Arc::clone(&r.store), graph, r.home.path());
    assert_eq!(
        engine
            .embed_pending(Arc::clone(&r.bulk), Some(Arc::clone(&r.bulk)), 10)
            .await
            .unwrap(),
        0
    );
    assert_eq!(r.endpoint.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_mismatched_vectors_also_drops_rows_without_a_model() {
    let r = rig("model-1", &["alpha", "beta"]).await;
    rebuild_brain_vectors(Arc::clone(&r.store), Arc::clone(&r.bulk), None, None)
        .await
        .unwrap();
    r.store
        .conn_arc()
        .lock()
        .unwrap()
        .execute(
            "UPDATE brain_vectors SET model = NULL WHERE doc_id = (SELECT MIN(doc_id) FROM brain_vectors)",
            [],
        )
        .unwrap();
    r.store.delete_vectors_not_matching("model-1", 8).unwrap();
    assert_eq!(vector_models(&r.store), vec![Some("model-1".to_string())]);
}
