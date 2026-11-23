//! End-to-end MCP tests for folder index embedding model tracking,
//! background rebuild, FTS fallback, and bulk vs short alias split.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use makakoo_core::embeddings::EmbeddingClient;
use makakoo_core::superbrain::folder_index::FolderIndexManager;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::dispatch::{ToolContext, ToolRegistry};

use super::register_folder_index;

/// How long a gated bulk answer is held: the mid-rebuild assertions run inside it.
const GATE_HOLD_SECS: u64 = 10;

#[derive(Clone)]
struct McpEmbedResponder {
    short_alias: String,
    bulk_alias: String,
    short_model: Arc<std::sync::Mutex<String>>,
    bulk_model: Arc<std::sync::Mutex<String>>,
    short_dim: Arc<AtomicUsize>,
    bulk_dim: Arc<AtomicUsize>,
    slot: Arc<AtomicUsize>,
    gate_bulk: Arc<AtomicBool>,
    gate_entered: Arc<AtomicBool>,
}

impl McpEmbedResponder {
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
        }
    }

    fn switch(&self, model: &str, dim: usize, slot: usize) {
        *self.short_model.lock().unwrap() = model.to_string();
        *self.bulk_model.lock().unwrap() = model.to_string();
        self.short_dim.store(dim, Ordering::SeqCst);
        self.bulk_dim.store(dim, Ordering::SeqCst);
        self.slot.store(slot, Ordering::SeqCst);
    }
}

impl Respond for McpEmbedResponder {
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

        // Hold a gated bulk answer back with wiremock's async delay: blocking here would
        // stall the mock server's single thread and every other request with it.
        let gated = !is_probe && is_bulk && self.gate_bulk.load(Ordering::SeqCst);
        if gated {
            self.gate_entered.store(true, Ordering::SeqCst);
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
        let template = ResponseTemplate::new(200).set_body_json(json!({
            "model": model_name,
            "data": data,
        }));
        if gated {
            template.set_delay(std::time::Duration::from_secs(GATE_HOLD_SECS))
        } else {
            template
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_folder_index_model_switch_rebuild_and_states() {
    let tmp = tempfile::tempdir().unwrap();
    let responder = McpEmbedResponder::new("ail-embed", "ail-embed-bulk", "qwen3-v1", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed-bulk"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path().join("indexes"))
            .with_embedders(Arc::clone(&short_emb), Arc::clone(&bulk_emb)),
    );
    let ctx = Arc::new(
        ToolContext::empty(tmp.path().to_path_buf())
            .with_embeddings(short_emb)
            .with_bulk_embeddings(bulk_emb)
            .with_folder_index(Arc::clone(&mgr)),
    );
    let mut reg = ToolRegistry::new();
    register_folder_index(&mut reg, ctx);

    let md = "Clause 7: indemnity cap equals twelve months of fees.";
    let sha = hex::encode(Sha256::digest(md.as_bytes()));
    let up = reg
        .call(
            "folder_index_upsert",
            json!({
                "folder_id": "legal",
                "file_path": "terms.pdf",
                "file_version": "v1",
                "dim": 32,
                "file_engine_output": {
                    "contract": "v1",
                    "format": "pdf",
                    "pages": [{
                        "page_number": 7,
                        "unit": "page",
                        "title": null,
                        "markdown": md,
                        "markdown_sha256": sha
                    }],
                    "warnings": []
                }
            }),
        )
        .await
        .unwrap();
    assert_eq!(up["chunks_written"], 1);

    let st1 = reg
        .call(
            "folder_index_states",
            json!({ "folder_id": "legal", "dim": 32 }),
        )
        .await
        .unwrap();
    assert_eq!(st1["index_state"], "ready");
    assert_eq!(st1["embed_model"], "qwen3-v1");
    assert_eq!(st1["embed_dim"], 64);

    // Model behind alias changes; gate bulk rebuild so mid-rebuild assertions are deterministic.
    responder.switch("bge-v2", 48, 3);
    responder.gate_bulk.store(true, Ordering::SeqCst);

    let mid_query = reg
        .call(
            "folder_index_search",
            json!({
                "folder_id": "legal",
                "query": "indemnity cap",
                "k": 5,
                "dim": 32
            }),
        )
        .await
        .unwrap();
    assert_eq!(mid_query["mode"], "fts");
    assert_eq!(mid_query["stale"], true);
    assert_eq!(mid_query["rebuilding"], true);
    assert_eq!(mid_query["passages"].as_array().unwrap().len(), 1);
    assert_eq!(mid_query["passages"][0]["anchor"]["value"], "7");

    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bulk embed call never reached the gate");

    // query_vector-only while stale returns empty passages and stale=true.
    let mid_vec_only = reg
        .call(
            "folder_index_search",
            json!({
                "folder_id": "legal",
                "query_vector": vec![0.1_f32; 32],
                "k": 5,
                "dim": 32
            }),
        )
        .await
        .unwrap();
    assert_eq!(mid_vec_only["stale"], true);
    assert_eq!(mid_vec_only["rebuilding"], true);
    assert!(mid_vec_only["passages"].as_array().unwrap().is_empty());

    let st_mid = reg
        .call(
            "folder_index_states",
            json!({ "folder_id": "legal", "dim": 32 }),
        )
        .await
        .unwrap();
    assert_eq!(st_mid["index_state"], "rebuilding", "{st_mid}");

    // Let the held answer through and verify vector search resumes with new model/dim.
    responder.gate_bulk.store(false, Ordering::SeqCst);
    mgr.wait_for_rebuild("legal").await;

    let st_after = reg
        .call(
            "folder_index_states",
            json!({ "folder_id": "legal", "dim": 32 }),
        )
        .await
        .unwrap();
    assert_eq!(st_after["index_state"], "ready");
    assert_eq!(st_after["embed_model"], "bge-v2");
    assert_eq!(st_after["embed_dim"], 48);

    let after = reg
        .call(
            "folder_index_search",
            json!({
                "folder_id": "legal",
                "query": "indemnity cap",
                "k": 5,
                "dim": 32
            }),
        )
        .await
        .unwrap();
    assert_eq!(after["mode"], "vector");
    assert_eq!(after["stale"], false);
    assert_eq!(after["rebuilding"], false);
    assert_eq!(after["embed_model"], "bge-v2");
    assert_eq!(after["embed_dim"], 48);
    assert_eq!(after["passages"].as_array().unwrap().len(), 1);
}
