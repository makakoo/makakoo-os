//! Guard tests for `SuperbrainSyncEmbedHandler`:
//! - SANCHO never rebuilds inline on a stale/non-ready index; it schedules
//!   through `ctx.brain_rebuild` and returns immediately.
//! - Top-up on a `Ready` index stays bounded by `max_docs`.
//! - `short_bulk_model_mismatch` is skipped with zero HTTP calls.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;
use crate::event_bus::PersistentEventBus;
use crate::llm::LlmClient;
use crate::sancho::handlers::{BgEmbedEnvGuard, BG_EMBED_ENV_LOCK};
use crate::sancho::{SanchoContext, SanchoHandler, SuperbrainSyncEmbedHandler};
use crate::superbrain::folder_index::model_meta::SHORT_BULK_MISMATCH_REASON;
use crate::superbrain::folder_index::IndexModelState;
use crate::superbrain::store::SuperbrainStore;
use crate::superbrain::vector_model::{
    rebuild_brain_vectors, BrainRebuildCoordinator, BrainSyncOutcome, DEFAULT_EMBED_TRUNCATE_CHARS,
};

#[derive(Clone)]
struct SyncEmbedResponder {
    model: String,
    dim: usize,
    gate_batches: Arc<AtomicBool>,
    gate_entered: Arc<AtomicBool>,
    gate_release: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}

impl SyncEmbedResponder {
    fn new(model: &str, dim: usize, gate_batches: bool) -> Self {
        Self {
            model: model.to_string(),
            dim,
            gate_batches: Arc::new(AtomicBool::new(gate_batches)),
            gate_entered: Arc::new(AtomicBool::new(false)),
            gate_release: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Respond for SyncEmbedResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let inputs = body
            .get("input")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(1);
        let is_probe = inputs == 1
            && body
                .get("input")
                .and_then(|v| v.get(0))
                .and_then(Value::as_str)
                == Some("probe");
        if !is_probe && self.gate_batches.load(Ordering::SeqCst) {
            self.gate_entered.store(true, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(30);
            while !self.gate_release.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let mut vec = vec![0.0_f32; self.dim];
        vec[0] = 1.0;
        let data: Vec<Value> = (0..inputs).map(|_| json!({ "embedding": vec })).collect();
        ResponseTemplate::new(200).set_body_json(json!({ "model": self.model, "data": data }))
    }
}

fn build_ctx(
    dir: &TempDir,
    store: Arc<SuperbrainStore>,
    server_uri: &str,
    coord: Arc<BrainRebuildCoordinator>,
) -> SanchoContext {
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("data/Brain/journals")).unwrap();
    let bus = PersistentEventBus::open(&dir.path().join("bus.db")).unwrap();
    let llm = Arc::new(LlmClient::new());
    let short_emb = Arc::new(EmbeddingClient::with_base_url(server_uri).with_model("short-alias"));
    let bulk_emb = Arc::new(EmbeddingClient::with_base_url(server_uri).with_model("bulk-alias"));
    SanchoContext::new(store, bus, llm, short_emb, home)
        .with_bulk_embeddings(bulk_emb)
        .with_brain_rebuild(coord)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sancho_tick_on_stale_index_returns_immediately_and_coordinator_completes() {
    let _lock = BG_EMBED_ENV_LOCK.lock().await;
    let _env = BgEmbedEnvGuard::enable();

    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/a.md", "alpha body", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/b.md", "beta body", "page", json!([]))
        .unwrap();
    store.mark_vectors_stale("model_changed").unwrap();

    let responder = SyncEmbedResponder::new("model-v2", 4, true);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let coord = Arc::new(BrainRebuildCoordinator::new());
    let ctx = build_ctx(&dir, Arc::clone(&store), &server.uri(), Arc::clone(&coord));

    // Must return while the bulk batch embed gate is still closed (an inline
    // rebuild would block until timeout).
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        SuperbrainSyncEmbedHandler::new().run(&ctx),
    )
    .await
    .expect("SANCHO tick blocked on rebuild instead of scheduling in background")
    .unwrap();
    assert!(report.ok);
    assert!(
        report.message.contains("rebuild scheduled"),
        "unexpected report message: {}",
        report.message
    );
    assert_eq!(
        store.stats().unwrap().vector_count,
        0,
        "SANCHO tick must return without embedding inline"
    );

    // Wait for the spawned coordinator rebuild to hit the batch gate.
    tokio::time::timeout(Duration::from_secs(10), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("coordinator rebuild never started");
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Rebuilding
    );

    // Release gate and confirm the coordinator finishes Ready with all docs embedded.
    responder.gate_batches.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    coord.wait(store.path()).await;

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-v2"));
    assert_eq!(status.embed_dim, Some(4));
    assert_eq!(store.stats().unwrap().vector_count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sancho_tick_top_up_on_ready_index_stays_bounded_by_limit() {
    let _lock = BG_EMBED_ENV_LOCK.lock().await;
    let _env = BgEmbedEnvGuard::enable();

    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/seed.md", "seed doc", "page", json!([]))
        .unwrap();

    let responder = SyncEmbedResponder::new("model-v1", 4, false);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let coord = Arc::new(BrainRebuildCoordinator::new());
    let ctx = build_ctx(&dir, Arc::clone(&store), &server.uri(), Arc::clone(&coord));

    // Bring index to Ready with model-v1.
    rebuild_brain_vectors(Arc::clone(&store), Arc::clone(&ctx.bulk_emb), None, None)
        .await
        .unwrap();
    assert_eq!(store.stats().unwrap().vector_count, 1);

    // Add 5 unembedded docs and tick with max_docs = 2.
    for i in 0..5 {
        store
            .write_document(
                &format!("pages/extra-{i}.md"),
                &format!("extra content {i}"),
                "page",
                json!([]),
            )
            .unwrap();
    }

    let handler = SuperbrainSyncEmbedHandler { max_docs: 2 };
    let report = handler.run(&ctx).await.unwrap();
    assert!(report.ok);
    assert!(
        report.message.contains("2 vectors embedded"),
        "unexpected report message: {}",
        report.message
    );
    assert_eq!(store.stats().unwrap().vector_count, 3);
    assert_eq!(store.docs_missing_vectors(10).unwrap().len(), 3);
    assert!(!coord.is_active(store.path()));
    assert_eq!(coord.spawned_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sancho_tick_skips_when_short_bulk_mismatched() {
    let _lock = BG_EMBED_ENV_LOCK.lock().await;
    let _env = BgEmbedEnvGuard::enable();

    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/a.md", "alpha body", "page", json!([]))
        .unwrap();
    store
        .mark_vectors_stale(SHORT_BULK_MISMATCH_REASON)
        .unwrap();

    let responder = SyncEmbedResponder::new("model-v1", 4, false);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let coord = Arc::new(BrainRebuildCoordinator::new());
    let ctx = build_ctx(&dir, Arc::clone(&store), &server.uri(), Arc::clone(&coord));

    let report = SuperbrainSyncEmbedHandler::new().run(&ctx).await.unwrap();
    assert!(report.ok);
    assert!(
        report.message.contains("skipped"),
        "unexpected report message: {}",
        report.message
    );
    assert_eq!(responder.calls.load(Ordering::SeqCst), 0);
    assert!(!coord.is_active(store.path()));
    assert_eq!(coord.spawned_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sancho_tick_skips_when_live_rebuild_lease_held_by_other_owner() {
    let _lock = BG_EMBED_ENV_LOCK.lock().await;
    let _env = BgEmbedEnvGuard::enable();

    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/a.md", "alpha body", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/b.md", "beta body", "page", json!([]))
        .unwrap();
    // Another process holds a live rebuild lease on this DB.
    let _other = store
        .try_mark_vectors_rebuilding("other_process")
        .unwrap()
        .unwrap();

    let responder = SyncEmbedResponder::new("model-v1", 4, false);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let coord = Arc::new(BrainRebuildCoordinator::new());
    let ctx = build_ctx(&dir, Arc::clone(&store), &server.uri(), Arc::clone(&coord));

    let outcome = coord
        .sync_or_spawn_rebuild(
            Arc::clone(&ctx.store),
            Arc::clone(&ctx.bulk_emb),
            Some(Arc::clone(&ctx.emb)),
            32,
            DEFAULT_EMBED_TRUNCATE_CHARS,
        )
        .await
        .unwrap();
    assert_eq!(outcome, BrainSyncOutcome::RebuildSkipped);

    let report = SuperbrainSyncEmbedHandler::new().run(&ctx).await.unwrap();
    assert!(report.ok);
    assert!(
        report.message.contains("skipped"),
        "unexpected report message: {}",
        report.message
    );
    assert_eq!(responder.calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.stats().unwrap().vector_count, 0);
    assert!(!coord.is_active(store.path()));
    assert_eq!(coord.spawned_count(), 0);
    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Rebuilding);
    assert_eq!(status.stale_reason.as_deref(), Some("other_process"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sancho_tick_top_up_races_with_rebuild_claim_writes_nothing() {
    let _lock = BG_EMBED_ENV_LOCK.lock().await;
    let _env = BgEmbedEnvGuard::enable();

    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/seed.md", "seed doc", "page", json!([]))
        .unwrap();

    let responder = SyncEmbedResponder::new("model-v1", 4, false);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let coord = Arc::new(BrainRebuildCoordinator::new());
    let ctx = Arc::new(build_ctx(
        &dir,
        Arc::clone(&store),
        &server.uri(),
        Arc::clone(&coord),
    ));

    // Bring index to Ready(model-v1, dim 4) with pages/seed.md embedded.
    rebuild_brain_vectors(Arc::clone(&store), Arc::clone(&ctx.bulk_emb), None, None)
        .await
        .unwrap();
    assert_eq!(store.stats().unwrap().vector_count, 1);

    // Add a missing doc and gate the top-up embed request so a rebuild claim
    // lands strictly between `vector_status()` (Ready) and the top-up write.
    store
        .write_document(
            "pages/race.md",
            "doc racing a rebuild claim",
            "page",
            json!([]),
        )
        .unwrap();
    responder.gate_batches.store(true, Ordering::SeqCst);

    let ctx_task = Arc::clone(&ctx);
    let tick_task =
        tokio::spawn(async move { SuperbrainSyncEmbedHandler::new().run(&ctx_task).await });

    tokio::time::timeout(Duration::from_secs(5), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("top-up embed request never reached the gate");

    // Competing rebuild claims ownership via a second independent DB connection
    // (simulating another process) before the top-up write runs.
    let other_conn = SuperbrainStore::open(store.path()).unwrap();
    let _owner = other_conn
        .try_mark_vectors_rebuilding("concurrent_rebuild")
        .unwrap()
        .unwrap();

    responder.gate_batches.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    let report = tokio::time::timeout(Duration::from_secs(5), tick_task)
        .await
        .expect("top-up tick did not finish after gate release")
        .unwrap()
        .unwrap();
    assert!(report.ok);
    assert!(
        report.message.contains("0 vectors embedded"),
        "top-up racing a rebuild claim must write 0 vectors, got: {}",
        report.message
    );
    assert_eq!(
        store.stats().unwrap().vector_count,
        1,
        "racing top-up must not write a vector into a rebuilding index"
    );
    assert_eq!(coord.spawned_count(), 0);
    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Rebuilding);
    assert_eq!(status.stale_reason.as_deref(), Some("concurrent_rebuild"));
}

#[test]
fn sancho_context_with_brain_rebuild_shares_coordinator_arc() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    let coord = Arc::new(BrainRebuildCoordinator::new());
    let ctx = build_ctx(&dir, store, "http://127.0.0.1:9", Arc::clone(&coord));
    assert!(Arc::ptr_eq(&coord, &ctx.brain_rebuild));
}
