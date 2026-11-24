//! MCP handler tests for Brain vector model isolation, background rebuild
//! empty vector returns, and FTS continuity.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use makakoo_core::embeddings::EmbeddingClient;
use makakoo_core::superbrain::folder_index::IndexModelState;
use makakoo_core::superbrain::graph::GraphStore;
use makakoo_core::superbrain::memory_stack::MemoryStack;
use makakoo_core::superbrain::store::SuperbrainStore;
use makakoo_core::superbrain::vector_model::rebuild_brain_vectors;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;

#[derive(Clone)]
struct McpBrainResponder {
    short_alias: String,
    bulk_alias: String,
    model: Arc<std::sync::Mutex<String>>,
    dim: Arc<AtomicUsize>,
    gate_bulk: Arc<AtomicBool>,
    gate_entered: Arc<AtomicBool>,
    gate_release: Arc<AtomicBool>,
}

impl McpBrainResponder {
    fn new(short_alias: &str, bulk_alias: &str, model: &str, dim: usize) -> Self {
        Self {
            short_alias: short_alias.to_string(),
            bulk_alias: bulk_alias.to_string(),
            model: Arc::new(std::sync::Mutex::new(model.to_string())),
            dim: Arc::new(AtomicUsize::new(dim)),
            gate_bulk: Arc::new(AtomicBool::new(false)),
            gate_entered: Arc::new(AtomicBool::new(false)),
            gate_release: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Respond for McpBrainResponder {
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

        if !is_probe && is_bulk && self.gate_bulk.load(Ordering::SeqCst) {
            // wiremock answers on its own current-thread runtime: block with std, not tokio.
            self.gate_entered.store(true, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !self.gate_release.load(Ordering::SeqCst) && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        let dim = self.dim.load(Ordering::SeqCst);
        let mut vec = vec![0.0_f32; dim];
        vec[0] = 1.0;
        let data: Vec<Value> = (0..inputs).map(|_| json!({ "embedding": vec })).collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "model": self.model.lock().unwrap().clone(),
            "data": data,
        }))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_brain_vector_search_returns_empty_during_rebuild_while_fts_works() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    let graph = Arc::new(GraphStore::new(store.conn_arc()));
    let memory = Arc::new(MemoryStack::new(store.conn_arc(), graph));
    store
        .write_document(
            "pages/runbook.md",
            "Canary rollout protocol requires smoke verification.",
            "page",
            json!([]),
        )
        .unwrap();

    let responder = McpBrainResponder::new("short-a", "bulk-a", "model-1", 8);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-a"));
    let bulk_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-a"));
    rebuild_brain_vectors(
        Arc::clone(&store),
        Arc::clone(&bulk_emb),
        Some(Arc::clone(&short_emb)),
        None,
    )
    .await
    .unwrap();

    let ctx = Arc::new(
        ToolContext::empty(dir.path().to_path_buf())
            .with_store(Arc::clone(&store))
            .with_memory(memory)
            .with_embeddings(short_emb)
            .with_bulk_embeddings(bulk_emb),
    );
    let vec_h = HarveySuperbrainVectorSearchHandler::new(Arc::clone(&ctx));
    let fts_h = BrainSearchHandler::new(Arc::clone(&ctx));
    let ctx_h = BrainContextHandler::new(Arc::clone(&ctx));

    let v1 = vec_h
        .call(json!({ "query": "smoke verification" }))
        .await
        .unwrap();
    assert_eq!(v1.as_array().unwrap().len(), 1);

    // Switch model and hold bulk rebuild mid-flight.
    *responder.model.lock().unwrap() = "model-2".to_string();
    responder.dim.store(16, Ordering::SeqCst);
    responder.gate_bulk.store(true, Ordering::SeqCst);

    let v_during = vec_h
        .call(json!({ "query": "smoke verification" }))
        .await
        .unwrap();
    assert!(v_during.as_array().unwrap().is_empty());

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

    // brain_search and brain_context stay responsive during rebuild.
    let fts_res = fts_h
        .call(json!({ "query": "smoke verification" }))
        .await
        .unwrap();
    assert_eq!(fts_res.as_array().unwrap().len(), 1);
    let bctx = ctx_h
        .call(json!({ "query": "smoke verification" }))
        .await
        .unwrap();
    assert!(bctx.get("context").is_some());

    // Finish rebuild -> vector search returns hits under model-2.
    responder.gate_bulk.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    ctx.brain_rebuild.wait(store.path()).await;

    let v_after = vec_h
        .call(json!({ "query": "smoke verification" }))
        .await
        .unwrap();
    assert_eq!(v_after.as_array().unwrap().len(), 1);
}

#[test]
fn mcp_tool_context_shares_single_brain_rebuild_coordinator_arc() {
    let dir = tempfile::tempdir().unwrap();
    let coord = Arc::new(makakoo_core::superbrain::vector_model::BrainRebuildCoordinator::new());
    let ctx = Arc::new(
        ToolContext::empty(dir.path().to_path_buf()).with_brain_rebuild(Arc::clone(&coord)),
    );
    let vec_h = HarveySuperbrainVectorSearchHandler::new(Arc::clone(&ctx));
    let ctx_h = BrainContextHandler::new(Arc::clone(&ctx));
    assert!(Arc::ptr_eq(&coord, &ctx.brain_rebuild));
    assert!(Arc::ptr_eq(&coord, &vec_h.ctx.brain_rebuild));
    assert!(Arc::ptr_eq(&coord, &ctx_h.ctx.brain_rebuild));
}
