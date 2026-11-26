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
    bulk_calls: Arc<AtomicUsize>,
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
            bulk_calls: Arc::new(AtomicUsize::new(0)),
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
        if !is_probe && is_bulk {
            self.bulk_calls.fetch_add(1, Ordering::SeqCst);
        }
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
    let responder = McpEmbedResponder::new("test-embed", "test-embed-bulk", "qwen3-v1", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed-bulk"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path().join("indexes"))
            .with_embedders(Arc::clone(&short_emb), Arc::clone(&bulk_emb))
            .with_model_reprobe_interval(std::time::Duration::from_secs(1)),
    );
    let ctx = Arc::new(
        ToolContext::empty(tmp.path().to_path_buf())
            .with_embeddings(Arc::clone(&short_emb))
            .with_bulk_embeddings(Arc::clone(&bulk_emb))
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
    mgr.wait_for_model_probe("legal").await;

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

    // The same long-lived manager must observe and confirm the switch. Process
    // restart is not evidence that an alias change is stable.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    mgr.get_or_open("legal", 32).unwrap();
    mgr.wait_for_model_probe("legal").await;

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    mgr.get_or_open("legal", 32).unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bulk embed call never reached the gate");

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_rebuild_requests_once_is_idempotent_and_scope_denial_changes_nothing() {
    use crate::jsonrpc::Request as RpcRequest;
    use crate::server::McpServer;
    use makakoo_core::agents::{AgentRegistry, AgentSlot};

    let tmp = tempfile::tempdir().unwrap();
    let responder = McpEmbedResponder::new("test-embed", "test-embed-bulk", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed-bulk"));
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
    let mut registry = ToolRegistry::new();
    register_folder_index(&mut registry, Arc::clone(&ctx));

    let requests_before_unknown = server.received_requests().await.unwrap().len();
    let err = registry
        .call(
            "folder_index_rebuild",
            json!({"folder_id": "unknown", "dim": 32}),
        )
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"));
    assert!(!tmp.path().join("indexes/unknown").exists());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        requests_before_unknown,
        "unknown MCP rebuild started a model probe"
    );

    let md = "One rebuild surface fixture.";
    let sha = hex::encode(Sha256::digest(md.as_bytes()));
    registry
        .call(
            "folder_index_upsert",
            json!({
                "folder_id": "manual",
                "file_path": "a.md",
                "file_version": "v1",
                "dim": 32,
                "file_engine_output": {
                    "contract": "v1",
                    "format": "markdown",
                    "pages": [{
                        "page_number": 1,
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
    mgr.wait_for_model_probe("manual").await;
    let index = mgr.get_or_open("manual", 32).unwrap();
    index.mark_stale("operator_requested_test").unwrap();
    let calls_before_rebuild = responder.bulk_calls.load(Ordering::SeqCst);
    let starting_generation = index.model_status().unwrap().generation;

    responder.gate_bulk.store(true, Ordering::SeqCst);
    let first = registry
        .call(
            "folder_index_rebuild",
            json!({"folder_id": "manual", "dim": 32}),
        )
        .await
        .unwrap();
    assert_eq!(first["outcome"], "started");
    assert_eq!(first["index_state"], "rebuilding");
    assert_eq!(first["halted"], false);
    assert_eq!(first["generation"], starting_generation);
    assert!(first.get("unproven_vectors_since_gen").is_some());

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("MCP rebuild never reached the bulk endpoint");
    let repeated_while_running = registry
        .call(
            "folder_index_rebuild",
            json!({"folder_id": "manual", "dim": 32}),
        )
        .await
        .unwrap();
    assert_eq!(repeated_while_running["outcome"], "already_running");
    assert_eq!(
        responder.bulk_calls.load(Ordering::SeqCst),
        calls_before_rebuild + 1
    );

    responder.gate_bulk.store(false, Ordering::SeqCst);
    mgr.wait_for_rebuild("manual").await;
    let requests_before_current = server.received_requests().await.unwrap().len();
    let repeated_after_ready = registry
        .call(
            "folder_index_rebuild",
            json!({"folder_id": "manual", "dim": 32}),
        )
        .await
        .unwrap();
    assert_eq!(repeated_after_ready["outcome"], "already_current");
    assert_eq!(
        responder.bulk_calls.load(Ordering::SeqCst),
        calls_before_rebuild + 1
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        requests_before_current,
        "AlreadyCurrent performed a model probe"
    );

    // Baseline inheritance never grants this operator-costly tool. It is
    // available only when the slot names it explicitly.
    index.mark_stale("unauthorized_test").unwrap();
    let before_seed = index.model_status().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let retry = json!({
        "model": "model-a",
        "dim": 64,
        "error_class": "http_503",
        "failures": 3,
        "halted": true,
        "not_before_unix_ms": now + 60_000,
        "failure_generation": before_seed.generation,
        "backoff_failures": 3,
        "writer_races": 0
    });
    let conn = rusqlite::Connection::open(tmp.path().join("indexes/manual/chunks.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO folder_meta (key, value) VALUES ('rebuild_retry', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [retry.to_string()],
    )
    .unwrap();
    drop(conn);
    let before = index.model_status().unwrap();
    assert!(before.halted);
    let seeded_status = registry
        .call(
            "folder_index_states",
            json!({"folder_id": "manual", "dim": 32}),
        )
        .await
        .unwrap();
    assert_eq!(seeded_status["halted"], true);
    assert_eq!(seeded_status["generation"], before.generation);
    let slot = AgentSlot {
        slot_id: "scoped".into(),
        name: "Scoped".into(),
        persona: None,
        inherit_baseline: true,
        allowed_paths: vec![],
        forbidden_paths: vec![],
        tools: vec![],
        process_mode: "supervised_pair".into(),
        transports: vec![],
        llm: None,
        runtime: None,
        triggers: Vec::new(),
    };
    AgentRegistry::create(tmp.path(), &slot).unwrap();
    let mcp = McpServer::new(Arc::new(registry), ctx);
    let request: RpcRequest = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "folder_index_rebuild",
            "arguments": {"folder_id": "manual", "dim": 32}
        }
    }))
    .unwrap();
    let denied = crate::dispatch::AGENT_ID
        .scope(Some("scoped".into()), mcp.handle(request))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(denied["isError"], true);
    assert!(denied["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("not in scope"));
    assert_eq!(index.model_status().unwrap(), before);
    assert_eq!(
        responder.bulk_calls.load(Ordering::SeqCst),
        calls_before_rebuild + 1
    );

    let explicit = AgentSlot {
        slot_id: "explicit".into(),
        name: "Explicit".into(),
        persona: None,
        inherit_baseline: true,
        allowed_paths: vec![],
        forbidden_paths: vec![],
        tools: vec!["folder_index_rebuild".into()],
        process_mode: "supervised_pair".into(),
        transports: vec![],
        llm: None,
        runtime: None,
        triggers: Vec::new(),
    };
    AgentRegistry::create(tmp.path(), &explicit).unwrap();
    let request: RpcRequest = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "folder_index_rebuild",
            "arguments": {"folder_id": "manual", "dim": 32}
        }
    }))
    .unwrap();
    let allowed = crate::dispatch::AGENT_ID
        .scope(Some("explicit".into()), mcp.handle(request))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_ne!(allowed["isError"], true, "explicit grant was denied");
    assert!(allowed["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("rate_limited"));
}

#[derive(Clone)]
struct StrictUpsertResponder {
    batch_sizes: Arc<std::sync::Mutex<Vec<usize>>>,
    received_bytes: Arc<std::sync::Mutex<Vec<usize>>>,
}

impl Respond for StrictUpsertResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let arr = body
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let is_probe = arr.len() == 1 && arr.first().and_then(Value::as_str) == Some("probe");
        if arr.len() > 16 {
            return ResponseTemplate::new(400).set_body_json(json!({
                "error": format!("batch size {} > 16", arr.len())
            }));
        }
        let mut data = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let text = item.as_str().unwrap_or("");
            if text.len() > 2_000 {
                return ResponseTemplate::new(400).set_body_json(json!({
                    "error": format!("input bytes {} > 2000", text.len())
                }));
            }
            if !is_probe {
                self.received_bytes.lock().unwrap().push(text.len());
            }
            let page_idx = text
                .split(':')
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let mut vec = vec![0.0_f32; 32];
            vec[page_idx % 32] = 1.0;
            data.push(json!({ "index": i, "embedding": vec }));
        }
        if !is_probe {
            self.batch_sizes.lock().unwrap().push(arr.len());
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "model": "qwen3-strict",
            "data": data,
        }))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_folder_index_upsert_splits_over_16_chunks_and_truncates_utf8_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let responder = StrictUpsertResponder {
        batch_sizes: Arc::new(std::sync::Mutex::new(Vec::new())),
        received_bytes: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed-bulk"));
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

    // 20 single-chunk pages: exceeds 16 inputs per request, and each page's
    // 1,100 Polish/German characters exceed 2,000 UTF-8 bytes when >2 bytes/char
    // or with custom chunk_options max_chars=2000.
    let mut pages = Vec::new();
    for p in 1..=20usize {
        let prefix = format!("{p}:");
        let body: String = "Zażółć gęślą jaźń — Prüfung größere Maßstäbe. "
            .chars()
            .cycle()
            .take(1_800 - prefix.chars().count())
            .collect();
        let md = format!("{prefix}{body}");
        assert!(md.len() > 2_000, "page {p} must exceed 2,000 UTF-8 bytes");
        let sha = hex::encode(Sha256::digest(md.as_bytes()));
        pages.push(json!({
            "page_number": p,
            "unit": "page",
            "title": null,
            "markdown": md,
            "markdown_sha256": sha
        }));
    }

    let up = reg
        .call(
            "folder_index_upsert",
            json!({
                "folder_id": "multipage",
                "file_path": "report.pdf",
                "file_version": "v1",
                "dim": 32,
                "chunk_options": {
                    "min_chars": 1600,
                    "target_chars": 1800,
                    "max_chars": 2000,
                    "overlap_chars": 100
                },
                "file_engine_output": {
                    "contract": "v1",
                    "format": "pdf",
                    "pages": pages,
                    "warnings": []
                }
            }),
        )
        .await
        .unwrap();
    assert_eq!(up["chunks_written"], 20);
    assert_eq!(responder.batch_sizes.lock().unwrap().as_slice(), &[16, 4]);
    for &b in responder.received_bytes.lock().unwrap().iter() {
        assert!(b <= 2_000);
    }

    // Query for page 19's exact basis vector to prove vector ordering across the
    // 16 + 4 batch split landed on the right page.
    let mut qv = vec![0.0_f32; 32];
    qv[19] = 1.0;
    let hit = reg
        .call(
            "folder_index_search",
            json!({
                "folder_id": "multipage",
                "query_vector": qv,
                "k": 1,
                "dim": 32
            }),
        )
        .await
        .unwrap();
    let passages = hit["passages"].as_array().unwrap();
    assert_eq!(passages.len(), 1);
    assert_eq!(passages[0]["anchor"]["value"], "19");
}
