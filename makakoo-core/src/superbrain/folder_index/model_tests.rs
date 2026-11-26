//! Guard-focused unit tests for per-folder embedding model tracking,
//! stale detection, background rebuilds, FTS fallback, and bulk/short split.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;

use super::model_meta::SHORT_BULK_MISMATCH_REASON;
use super::*;

static OBSERVED_BULK_SQL_TRACE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static MODEL_META_SQL_TRACE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn capture_observed_bulk_sql(sql: &str) {
    OBSERVED_BULK_SQL_TRACE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(sql.to_string());
}

fn capture_model_meta_sql(sql: &str) {
    MODEL_META_SQL_TRACE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(sql.to_string());
}

#[derive(Clone)]
struct DynamicEmbedResponder {
    short_alias: String,
    bulk_alias: String,
    short_model: Arc<std::sync::Mutex<String>>,
    bulk_model: Arc<std::sync::Mutex<String>>,
    bulk_embedding_response_model: Arc<std::sync::Mutex<Option<String>>>,
    short_dim: Arc<AtomicUsize>,
    bulk_dim: Arc<AtomicUsize>,
    short_vec_slot: Arc<AtomicUsize>,
    bulk_vec_slot: Arc<AtomicUsize>,
    gate_bulk: Arc<AtomicBool>,
    gate_after_bulk_calls: Arc<AtomicUsize>,
    gate_entered: Arc<AtomicBool>,
    gate_release: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    bulk_probe_calls: Arc<AtomicUsize>,
    bulk_calls: Arc<AtomicUsize>,
    fail_short_probes: Arc<AtomicBool>,
    fail_bulk_probes: Arc<AtomicBool>,
    fail_bulk_embeddings: Arc<AtomicBool>,
    vary_failure_body: Arc<AtomicBool>,
}

impl DynamicEmbedResponder {
    fn new(short_alias: &str, bulk_alias: &str, model: &str, dim: usize) -> Self {
        Self {
            short_alias: short_alias.to_string(),
            bulk_alias: bulk_alias.to_string(),
            short_model: Arc::new(std::sync::Mutex::new(model.to_string())),
            bulk_model: Arc::new(std::sync::Mutex::new(model.to_string())),
            bulk_embedding_response_model: Arc::new(std::sync::Mutex::new(None)),
            short_dim: Arc::new(AtomicUsize::new(dim)),
            bulk_dim: Arc::new(AtomicUsize::new(dim)),
            short_vec_slot: Arc::new(AtomicUsize::new(0)),
            bulk_vec_slot: Arc::new(AtomicUsize::new(0)),
            gate_bulk: Arc::new(AtomicBool::new(false)),
            gate_after_bulk_calls: Arc::new(AtomicUsize::new(0)),
            gate_entered: Arc::new(AtomicBool::new(false)),
            gate_release: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(AtomicUsize::new(0)),
            bulk_probe_calls: Arc::new(AtomicUsize::new(0)),
            bulk_calls: Arc::new(AtomicUsize::new(0)),
            fail_short_probes: Arc::new(AtomicBool::new(false)),
            fail_bulk_probes: Arc::new(AtomicBool::new(false)),
            fail_bulk_embeddings: Arc::new(AtomicBool::new(false)),
            vary_failure_body: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set_both_model(&self, model: &str, dim: usize, vec_slot: usize) {
        *self.short_model.lock().unwrap() = model.to_string();
        *self.bulk_model.lock().unwrap() = model.to_string();
        self.short_dim.store(dim, Ordering::SeqCst);
        self.bulk_dim.store(dim, Ordering::SeqCst);
        self.short_vec_slot.store(vec_slot, Ordering::SeqCst);
        self.bulk_vec_slot.store(vec_slot, Ordering::SeqCst);
    }

    fn set_bulk_model(&self, model: &str, dim: usize, vec_slot: usize) {
        *self.bulk_model.lock().unwrap() = model.to_string();
        self.bulk_dim.store(dim, Ordering::SeqCst);
        self.bulk_vec_slot.store(vec_slot, Ordering::SeqCst);
    }

    fn set_short_model(&self, model: &str, dim: usize, vec_slot: usize) {
        *self.short_model.lock().unwrap() = model.to_string();
        self.short_dim.store(dim, Ordering::SeqCst);
        self.short_vec_slot.store(vec_slot, Ordering::SeqCst);
    }

    fn set_bulk_embedding_response_model(&self, model: Option<&str>) {
        *self.bulk_embedding_response_model.lock().unwrap() = model.map(str::to_string);
    }
}

impl Respond for DynamicEmbedResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.calls.fetch_add(1, Ordering::SeqCst);
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

        if is_probe && req_model == self.bulk_alias {
            self.bulk_probe_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_bulk_probes.load(Ordering::SeqCst) {
                return ResponseTemplate::new(503);
            }
        }
        if is_probe
            && req_model == self.short_alias
            && req_model != self.bulk_alias
            && self.fail_short_probes.load(Ordering::SeqCst)
        {
            return ResponseTemplate::new(503);
        }

        if !is_probe && (is_bulk || inputs > 1) {
            let bulk_call = self.bulk_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_bulk_embeddings.load(Ordering::SeqCst) {
                let response = ResponseTemplate::new(503);
                return if self.vary_failure_body.load(Ordering::SeqCst) {
                    response.set_body_string(format!(
                        "transient failure {}",
                        self.bulk_calls.load(Ordering::SeqCst)
                    ))
                } else {
                    response
                };
            }
            if self.gate_bulk.load(Ordering::SeqCst)
                && bulk_call > self.gate_after_bulk_calls.load(Ordering::SeqCst)
            {
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

        let (mut model_name, dim, slot) = if is_bulk {
            (
                self.bulk_model.lock().unwrap().clone(),
                self.bulk_dim.load(Ordering::SeqCst),
                self.bulk_vec_slot.load(Ordering::SeqCst),
            )
        } else {
            (
                self.short_model.lock().unwrap().clone(),
                self.short_dim.load(Ordering::SeqCst),
                self.short_vec_slot.load(Ordering::SeqCst),
            )
        };
        if is_bulk && !is_probe {
            if let Some(override_model) = self.bulk_embedding_response_model.lock().unwrap().clone()
            {
                model_name = override_model;
            }
        }

        let mut vec = vec![0.0_f32; dim];
        vec[slot % dim] = 1.0;
        let data: Vec<Value> = (0..inputs).map(|_| json!({ "embedding": vec })).collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "model": model_name,
            "data": data,
        }))
    }
}

fn basis(dim: usize, slot: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; dim];
    v[slot % dim] = 1.0;
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_switch_rebuilds_in_background_and_serves_fts_until_ready() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed-bulk", "model-v1", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed-bulk"));
    let initial_mgr = Arc::new(
        FolderIndexManager::new(tmp.path())
            .with_embedders(Arc::clone(&short_emb), Arc::clone(&bulk_emb)),
    );

    let vec_v1 = basis(64, 1);
    initial_mgr
        .upsert_folder_file(
            "docs",
            32,
            &UpsertFileRequest {
                file_path: "msa.pdf".into(),
                file_version: "v1".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "alice".into(),
                chunks: vec![ChunkInput {
                    anchor: Anchor {
                        kind: AnchorKind::Page,
                        value: "2".into(),
                    },
                    char_start: 0,
                    char_end: 44,
                    text: "Net 30 payment terms govern invoice timing.".into(),
                    vector: vec_v1,
                }],
            },
            Some(&UpsertModelMeta {
                model: "model-v1".into(),
                embed_dim: 64,
                bulk_alias: "test-embed-bulk".into(),
                short_alias: Some("test-embed".into()),
            }),
        )
        .await
        .unwrap();
    initial_mgr.wait_for_model_probe("docs").await;

    // Baseline vector search succeeds under model-v1.
    let res1 = initial_mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "docs".into(),
                dim: 32,
                k: 5,
                query: Some("payment terms".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(res1.mode, SearchMode::Vector);
    assert!(!res1.stale);
    assert_eq!(res1.passages.len(), 1);

    // Switch underlying model behind alias and hold bulk rebuild mid-flight.
    responder.set_both_model("model-v2", 48, 5);
    responder.gate_bulk.store(true, Ordering::SeqCst);

    // A fresh open probes the bulk embedder. That index-side observation, not
    // query metadata, starts the single rebuild.
    drop(initial_mgr);
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path())
            .with_embedders(Arc::clone(&short_emb), Arc::clone(&bulk_emb)),
    );
    let idx = mgr.get_or_open("docs", 32).unwrap();
    mgr.wait_for_model_probe("docs").await;
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("docs", 32).await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bulk probe did not start the rebuild");

    // Searches during the bulk-triggered rebuild serve FTS only.
    let during_fts = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "docs".into(),
                dim: 32,
                k: 5,
                query: Some("invoice timing".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(during_fts.mode, SearchMode::Fts);
    assert!(during_fts.stale);
    assert!(during_fts.rebuilding);
    assert_eq!(during_fts.passages.len(), 1);
    assert!(during_fts.passages[0].text.contains("Net 30"));

    // Direct vector search MUST return 0 vector results while rebuilding.
    assert!(idx
        .search(&basis(48, 5), 5, &SearchFilter::default())
        .unwrap()
        .is_empty());

    // query_vector-only while stale returns empty passages and stale=true.
    let during_vec_only = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "docs".into(),
                dim: 32,
                k: 5,
                query: None,
                query_vector: Some(basis(32, 5)),
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert!(during_vec_only.stale);
    assert!(during_vec_only.rebuilding);
    assert!(during_vec_only.passages.is_empty());

    // Unblock rebuild and wait for completion.
    responder.gate_bulk.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    mgr.wait_for_rebuild("docs").await;

    let status_after = idx.model_status().unwrap();
    assert_eq!(status_after.index_state, IndexModelState::Ready);
    assert_eq!(status_after.embed_model.as_deref(), Some("model-v2"));
    assert_eq!(status_after.embed_dim, Some(48));
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);

    // Vector search works again under model-v2.
    let after = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "docs".into(),
                dim: 32,
                k: 5,
                query: Some("payment".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(after.mode, SearchMode::Vector);
    assert!(!after.stale);
    assert!(!after.rebuilding);
    assert_eq!(after.passages.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_query_mismatch_records_fallback_without_reembedding() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "index-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    mgr.upsert_folder_file(
        "query-mismatch",
        32,
        &one_chunk("a.md", "Mismatch fallback text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "index-model".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("query-mismatch").await;
    let calls_after_open_probe = responder.calls.load(Ordering::SeqCst);
    let bulk_probes_after_open = responder.bulk_probe_calls.load(Ordering::SeqCst);
    assert_eq!(bulk_probes_after_open, 1);

    let mismatches = [
        ("client-resolved-model", 64, "model_mismatch:"),
        ("index-model", 48, "dim_mismatch:"),
        ("client-resolved-model", 48, "model_mismatch:"),
    ];
    for (query_model, query_dim, reason_prefix) in mismatches {
        let response = mgr
            .search_folder(
                FolderSearchRequest {
                    folder_id: "query-mismatch".into(),
                    dim: 32,
                    k: 5,
                    query: Some("fallback".into()),
                    query_vector: Some(basis(64, 2)),
                    query_model: Some(query_model.into()),
                    query_dim: Some(query_dim),
                    filter: SearchFilter::default(),
                },
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.mode, SearchMode::Fts);
        assert!(response.stale);
        assert!(!response.rebuilding);
        let status = mgr
            .get_or_open_without_probe_for_test("query-mismatch", 32)
            .unwrap()
            .model_status()
            .unwrap();
        assert_eq!(status.index_state, IndexModelState::Ready);
        assert!(status
            .query_mismatch_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with(reason_prefix)));
    }

    assert_eq!(
        responder.calls.load(Ordering::SeqCst),
        calls_after_open_probe
    );
    assert_eq!(
        responder.bulk_probe_calls.load(Ordering::SeqCst),
        bulk_probes_after_open
    );
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);

    let matched = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "query-mismatch".into(),
                dim: 32,
                k: 5,
                query: Some("fallback".into()),
                query_vector: Some(basis(64, 1)),
                query_model: Some("index-model".into()),
                query_dim: Some(64),
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(matched.mode, SearchMode::Vector);
    assert_eq!(
        mgr.get_or_open_without_probe_for_test("query-mismatch", 32)
            .unwrap()
            .model_status()
            .unwrap()
            .query_mismatch_reason,
        None
    );
    assert_eq!(
        responder.calls.load(Ordering::SeqCst),
        calls_after_open_probe
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn periodic_probe_rebuilds_once_after_alias_switch() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-v1", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(Arc::clone(&short), Arc::clone(&bulk))
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_millis(25),
            std::time::Duration::from_millis(5),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
            3,
        );
    mgr.upsert_folder_file(
        "switch",
        32,
        &one_chunk("a.md", "Alias switch text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-v1".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("switch").await;
    let probes_before = responder.bulk_probe_calls.load(Ordering::SeqCst);

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    responder.set_bulk_model("model-v2", 64, 3);
    let response = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "switch".into(),
                dim: 32,
                k: 5,
                query: Some("Alias".into()),
                query_vector: Some(basis(64, 2)),
                query_model: Some("client-claimed-model".into()),
                query_dim: Some(64),
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::Fts);
    mgr.wait_for_model_probe("switch").await;
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_millis(25));
    mgr.probe_folder_model_for_test("switch", 32).await.unwrap();
    mgr.wait_for_rebuild("switch").await;

    let status = mgr
        .get_or_open_without_probe_for_test("switch", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(status.embed_model.as_deref(), Some("model-v2"));
    assert_eq!(
        status.stale_reason.as_deref(),
        Some(SHORT_BULK_MISMATCH_REASON)
    );
    assert_eq!(
        responder.bulk_probe_calls.load(Ordering::SeqCst),
        probes_before + 2
    );
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);

    for _ in 0..20 {
        let _ = mgr
            .search_folder(
                FolderSearchRequest {
                    folder_id: "switch".into(),
                    dim: 32,
                    k: 1,
                    query: Some("Alias".into()),
                    query_vector: Some(basis(64, 2)),
                    query_model: Some("still-client-claimed".into()),
                    query_dim: Some(64),
                    filter: SearchFilter::default(),
                },
                None,
                None,
            )
            .await
            .unwrap();
    }
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_open_probe_retries_after_backoff() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "stable", 64);
    responder.fail_bulk_probes.store(true, Ordering::SeqCst);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(short, bulk)
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
            3,
        );
    mgr.upsert_folder_file(
        "retry",
        32,
        &one_chunk("a.md", "Retry text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "stable".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("retry").await;
    assert_eq!(responder.bulk_probe_calls.load(Ordering::SeqCst), 1);

    responder.fail_bulk_probes.store(false, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    let _ = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "retry".into(),
                dim: 32,
                k: 1,
                query: Some("Retry".into()),
                query_vector: Some(basis(64, 2)),
                query_model: Some("client-mismatch".into()),
                query_dim: Some(64),
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    mgr.wait_for_model_probe("retry").await;
    assert_eq!(responder.bulk_probe_calls.load(Ordering::SeqCst), 2);
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writer_race_backs_off_without_halting_and_recovers_after_writer_stops() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(short, bulk)
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(25),
            std::time::Duration::from_millis(100),
            1,
        );
    mgr.upsert_folder_file(
        "writer-race",
        32,
        &one_chunk("a.md", "Initial A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("writer-race").await;
    let index = mgr
        .get_or_open_without_probe_for_test("writer-race", 32)
        .unwrap();

    responder.set_both_model("model-b", 64, 2);
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_millis(10));
    mgr.probe_folder_model_for_test("writer-race", 32)
        .await
        .unwrap();

    let writes = Arc::new(AtomicUsize::new(0));
    let hook_writes = Arc::clone(&writes);
    *index.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |idx| {
        let attempt = hook_writes.fetch_add(1, Ordering::SeqCst) + 1;
        let text = format!("Concurrent B write {attempt}.");
        idx.upsert_file_with_model(
            &UpsertFileRequest {
                file_path: "writer.md".into(),
                file_version: format!("v{attempt}"),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "alice".into(),
                chunks: vec![ChunkInput {
                    anchor: Anchor {
                        kind: AnchorKind::Heading,
                        value: "Writer".into(),
                    },
                    char_start: 0,
                    char_end: text.len(),
                    text,
                    vector: basis(64, 2),
                }],
            },
            Some(&UpsertModelMeta {
                model: "model-b".into(),
                embed_dim: 64,
                bulk_alias: "bulk-alias".into(),
                short_alias: Some("short-alias".into()),
            }),
        )
        .unwrap();
    }));

    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_millis(10));
    mgr.probe_folder_model_for_test("writer-race", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("writer-race").await;
    assert_eq!(
        writes.load(Ordering::SeqCst),
        super::rebuild::MAX_REBUILD_COMMIT_ATTEMPTS
    );
    let raced = index.model_status().unwrap();
    assert_eq!(raced.index_state, IndexModelState::Stale);
    assert!(!raced.halted, "writer races must never count toward halt");
    assert!(raced
        .stale_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("rebuild_deferred:")));
    let first_retry = model_meta::read_persisted_rebuild_retry(&index.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    let first_attempt =
        model_meta::read_persisted_automatic_rebuild_success(&index.lock_conn().unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(first_retry.writer_races, 1);
    assert_eq!(first_retry.failures, 0);
    assert_eq!(first_retry.error_class, "writer_race");
    assert!(
        first_retry.not_before_unix_ms >= first_attempt.last_success_unix_ms + 25,
        "first writer-race delay was shorter than the configured backoff"
    );

    // The retry delay elapses, but the claimed attempt consumed the durable
    // one-hour automatic-rebuild window.
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_millis(30));
    mgr.probe_folder_model_for_test("writer-race", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("writer-race").await;
    assert_eq!(
        writes.load(Ordering::SeqCst),
        super::rebuild::MAX_REBUILD_COMMIT_ATTEMPTS,
        "writer race bypassed the hourly automatic-attempt cap"
    );

    // After the hourly cap, a second collision gets double the independent
    // writer-race delay while the genuine failure streak remains untouched.
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60 * 60 + 1));
    mgr.probe_folder_model_for_test("writer-race", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("writer-race").await;
    assert_eq!(
        writes.load(Ordering::SeqCst),
        super::rebuild::MAX_REBUILD_COMMIT_ATTEMPTS * 2
    );
    let second_retry = model_meta::read_persisted_rebuild_retry(&index.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    let second_attempt =
        model_meta::read_persisted_automatic_rebuild_success(&index.lock_conn().unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(second_retry.writer_races, 2);
    assert_eq!(second_retry.failures, 0);
    assert_eq!(second_retry.error_class, "writer_race");
    assert!(
        second_retry.not_before_unix_ms >= second_attempt.last_success_unix_ms + 50,
        "writer-race backoff did not double"
    );

    // Writer quiesces. A normal later query schedules recovery; no explicit
    // rebuild request or manual metadata repair is needed.
    *index.before_rebuild_commit.lock().unwrap() = None;
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60 * 60 + 1));
    let response = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "writer-race".into(),
                dim: 32,
                k: 1,
                query: Some("Initial".into()),
                query_vector: Some(basis(64, 2)),
                query_model: Some("model-b".into()),
                query_dim: Some(64),
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::Fts);
    mgr.wait_for_model_probe("writer-race").await;
    mgr.wait_for_rebuild("writer-race").await;
    let recovered = index.model_status().unwrap();
    assert_eq!(recovered.index_state, IndexModelState::Ready);
    assert_eq!(recovered.embed_model.as_deref(), Some("model-b"));
    assert_eq!(recovered.unproven_vectors_since_gen, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writer_race_preserves_a_same_class_failure_streak() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(short, bulk)
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(25),
            std::time::Duration::from_millis(100),
            3,
        );
    mgr.upsert_folder_file(
        "failure-race",
        32,
        &one_chunk("a.md", "Initial A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("failure-race").await;
    let index = mgr
        .get_or_open_without_probe_for_test("failure-race", 32)
        .unwrap();
    responder.set_both_model("model-b", 64, 2);
    responder.fail_bulk_embeddings.store(true, Ordering::SeqCst);
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_millis(10));
    mgr.probe_folder_model_for_test("failure-race", 32)
        .await
        .unwrap();

    for _ in 0..2 {
        mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60 * 60 + 1));
        mgr.probe_folder_model_for_test("failure-race", 32)
            .await
            .unwrap();
        mgr.wait_for_rebuild("failure-race").await;
    }
    let before_race = model_meta::read_persisted_rebuild_retry(&index.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(before_race.error_class, "http_503");
    assert_eq!(before_race.failures, 2);

    responder
        .fail_bulk_embeddings
        .store(false, Ordering::SeqCst);
    let writes = Arc::new(AtomicUsize::new(0));
    let hook_writes = Arc::clone(&writes);
    *index.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |idx| {
        let attempt = hook_writes.fetch_add(1, Ordering::SeqCst) + 1;
        idx.upsert_file_with_model(
            &UpsertFileRequest {
                file_path: "writer.md".into(),
                file_version: format!("v{attempt}"),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "alice".into(),
                chunks: vec![ChunkInput {
                    anchor: Anchor {
                        kind: AnchorKind::Heading,
                        value: "Writer".into(),
                    },
                    char_start: 0,
                    char_end: 6,
                    text: format!("Write {attempt}"),
                    vector: basis(64, 2),
                }],
            },
            Some(&UpsertModelMeta {
                model: "model-b".into(),
                embed_dim: 64,
                bulk_alias: "bulk-alias".into(),
                short_alias: Some("short-alias".into()),
            }),
        )
        .unwrap();
    }));
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60 * 60 + 1));
    mgr.probe_folder_model_for_test("failure-race", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("failure-race").await;
    assert_eq!(
        writes.load(Ordering::SeqCst),
        super::rebuild::MAX_REBUILD_COMMIT_ATTEMPTS
    );
    let after_race = model_meta::read_persisted_rebuild_retry(&index.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(after_race.error_class, "http_503");
    assert_eq!(after_race.failures, 2);
    assert!(!after_race.halted);
    assert_eq!(after_race.writer_races, 1);

    *index.before_rebuild_commit.lock().unwrap() = None;
    index
        .upsert_file_with_model(
            &one_chunk("after-race.md", "Requires a fresh embedding.", basis(64, 2)),
            Some(&UpsertModelMeta {
                model: "model-b".into(),
                embed_dim: 64,
                bulk_alias: "bulk-alias".into(),
                short_alias: Some("short-alias".into()),
            }),
        )
        .unwrap();
    responder.fail_bulk_embeddings.store(true, Ordering::SeqCst);
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60 * 60 + 1));
    mgr.probe_folder_model_for_test("failure-race", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("failure-race").await;
    let halted = {
        let conn = index.lock_conn().unwrap();
        model_meta::read_persisted_rebuild_retry(&conn).unwrap()
    }
    .unwrap_or_else(|| {
        panic!(
            "missing final retry: status={:?}, bulk_calls={}",
            index.model_status().unwrap(),
            responder.bulk_calls.load(Ordering::SeqCst)
        )
    });
    assert_eq!(halted.error_class, "http_503");
    assert_eq!(halted.failures, 3, "fail, fail, race, fail did not halt");
    assert!(halted.halted, "preserved failure streak did not re-halt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn genuine_same_class_failure_halts_with_backoff() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "old", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(short, bulk)
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_secs(10 * 60),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(6 * 60 * 60),
            3,
        );
    mgr.upsert_folder_file(
        "failed",
        32,
        &one_chunk("a.md", "Failure text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "old".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("failed").await;
    responder.set_both_model("new", 64, 2);
    responder.fail_bulk_embeddings.store(true, Ordering::SeqCst);

    for _ in 0..2 {
        mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(10 * 60));
        let _ = mgr
            .search_folder(
                FolderSearchRequest {
                    folder_id: "failed".into(),
                    dim: 32,
                    k: 1,
                    query: Some("Failure".into()),
                    query_vector: Some(basis(64, 2)),
                    query_model: Some("mismatch".into()),
                    query_dim: Some(64),
                    filter: SearchFilter::default(),
                },
                None,
                None,
            )
            .await
            .unwrap();
        mgr.wait_for_model_probe("failed").await;
        mgr.wait_for_rebuild("failed").await;
    }
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);

    for _ in 0..20 {
        if responder.bulk_calls.load(Ordering::SeqCst) == 3 {
            break;
        }
        mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(10 * 60));
        let _ = mgr
            .search_folder(
                FolderSearchRequest {
                    folder_id: "failed".into(),
                    dim: 32,
                    k: 1,
                    query: Some("Failure".into()),
                    query_vector: Some(basis(64, 2)),
                    query_model: Some("mismatch".into()),
                    query_dim: Some(64),
                    filter: SearchFilter::default(),
                },
                None,
                None,
            )
            .await
            .unwrap();
        mgr.wait_for_model_probe("failed").await;
        mgr.wait_for_rebuild("failed").await;
    }
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 3);
    let status = mgr
        .get_or_open_without_probe_for_test("failed", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert!(status.halted);
    assert!(status
        .stale_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("rebuild_suppressed:")
            && reason.contains("after 3 same-class failures")));
    let index = mgr
        .get_or_open_without_probe_for_test("failed", 32)
        .unwrap();
    let first_halt = model_meta::read_persisted_rebuild_retry(&index.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(first_halt.failures, 3);
    assert_eq!(first_halt.backoff_failures, 3);
    let first_attempt =
        model_meta::read_persisted_automatic_rebuild_success(&index.lock_conn().unwrap())
            .unwrap()
            .unwrap();
    assert!(first_halt.not_before_unix_ms >= first_attempt.last_success_unix_ms + 80 * 60 * 1_000);

    // A real halt still enforces its bounded backoff window.
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(10 * 60));
    let _ = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "failed".into(),
                dim: 32,
                k: 1,
                query: Some("Failure".into()),
                query_vector: Some(basis(64, 2)),
                query_model: Some("mismatch".into()),
                query_dim: Some(64),
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    mgr.wait_for_model_probe("failed").await;
    mgr.wait_for_rebuild("failed").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 3);

    // Expiry re-arms one attempt without resetting the same-class streak. The
    // next 503 re-halts immediately and doubles the bounded cooldown.
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(2 * 60 * 60));
    let _ = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "failed".into(),
                dim: 32,
                k: 1,
                query: Some("Failure".into()),
                query_vector: Some(basis(64, 2)),
                query_model: Some("mismatch".into()),
                query_dim: Some(64),
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    mgr.wait_for_model_probe("failed").await;
    mgr.wait_for_rebuild("failed").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 4);
    let second_halt = model_meta::read_persisted_rebuild_retry(&index.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    let second_attempt =
        model_meta::read_persisted_automatic_rebuild_success(&index.lock_conn().unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(second_halt.failures, 3);
    assert_eq!(second_halt.backoff_failures, 4);
    assert!(second_halt.halted);
    assert_eq!(
        second_halt.failure_generation,
        first_halt.failure_generation
    );
    assert!(
        second_halt.not_before_unix_ms >= second_attempt.last_success_unix_ms + 160 * 60 * 1_000,
        "re-halt cooldown did not double"
    );

    responder
        .fail_bulk_embeddings
        .store(false, Ordering::SeqCst);
    mgr.request_folder_rebuild("failed", 32).await.unwrap();
    mgr.wait_for_rebuild("failed").await;
    let recovered = mgr
        .get_or_open_without_probe_for_test("failed", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(recovered.index_state, IndexModelState::Ready);
    assert_eq!(recovered.embed_model.as_deref(), Some("new"));
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persisted_halt_clears_for_newer_flag_or_bounded_cooldown() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(Arc::clone(&emb), Arc::clone(&emb))
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(100),
            std::time::Duration::from_millis(100),
            3,
        );
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    };

    for folder in ["newer-flag", "cooldown"] {
        mgr.upsert_folder_file(
            folder,
            32,
            &one_chunk("a.md", "Initial A text.", basis(64, 1)),
            Some(&UpsertModelMeta {
                model: "model-a".into(),
                embed_dim: 64,
                bulk_alias: "test-embed".into(),
                short_alias: Some("test-embed".into()),
            }),
        )
        .await
        .unwrap();
        mgr.wait_for_model_probe(folder).await;
        let index = mgr.get_or_open_without_probe_for_test(folder, 32).unwrap();
        // Metadata-free bytes are deliberately unproven and must force a full
        // rebuild before this index can become Ready again.
        index
            .upsert_file_with_model(
                &one_chunk("unproven.md", "Unproven bytes.", basis(64, 2)),
                None,
            )
            .unwrap();
        let flag_generation = index
            .model_status()
            .unwrap()
            .unproven_vectors_since_gen
            .unwrap();
        let retry = model_meta::PersistedRebuildRetry {
            model: "model-a".into(),
            dim: 64,
            error_class: "http_503".into(),
            failures: 3,
            halted: true,
            not_before_unix_ms: if folder == "newer-flag" {
                now_ms() + 60_000
            } else {
                now_ms() + 10
            },
            failure_generation: if folder == "newer-flag" {
                flag_generation.saturating_sub(1)
            } else {
                flag_generation
            },
            backoff_failures: 3,
            writer_races: 0,
        };
        model_meta::write_persisted_rebuild_retry(&index.lock_conn().unwrap(), &retry).unwrap();
        if folder == "newer-flag" {
            assert!(retry.not_before_unix_ms > now_ms());
            assert!(
                !index.model_status().unwrap().halted,
                "a flag newer than the halt must invalidate it immediately"
            );
        } else {
            assert!(index.model_status().unwrap().halted);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(
                !index.model_status().unwrap().halted,
                "halt must expire after its bounded cooldown"
            );
        }

        mgr.advance_model_recovery_time_for_test(std::time::Duration::from_millis(20));
        let response = mgr
            .search_folder(
                FolderSearchRequest {
                    folder_id: folder.into(),
                    dim: 32,
                    k: 1,
                    query: Some("Initial".into()),
                    query_vector: Some(basis(64, 1)),
                    query_model: Some("model-a".into()),
                    query_dim: Some(64),
                    filter: SearchFilter::default(),
                },
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.mode, SearchMode::Fts);
        mgr.wait_for_model_probe(folder).await;
        mgr.wait_for_rebuild(folder).await;
        let recovered = index.model_status().unwrap();
        assert_eq!(recovered.index_state, IndexModelState::Ready);
        assert_eq!(recovered.unproven_vectors_since_gen, None);
        assert!(!recovered.halted);
    }
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_retry_without_failure_generation_reads_and_auto_clears() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(Arc::clone(&emb), Arc::clone(&emb))
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(25),
            std::time::Duration::from_millis(100),
            3,
        );
    mgr.upsert_folder_file(
        "legacy-retry",
        32,
        &one_chunk("a.md", "Initial A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("legacy-retry").await;
    let index = mgr
        .get_or_open_without_probe_for_test("legacy-retry", 32)
        .unwrap();
    index
        .upsert_file_with_model(
            &one_chunk("unproven.md", "Unproven bytes.", basis(64, 2)),
            None,
        )
        .unwrap();
    let flag_generation = index
        .model_status()
        .unwrap()
        .unproven_vectors_since_gen
        .unwrap();
    assert!(flag_generation > 0);
    let legacy = json!({
        "model": "model-a",
        "dim": 64,
        "error_class": "http_503",
        "failures": 3,
        "halted": true,
        "not_before_unix_ms": u64::MAX,
    })
    .to_string();
    index
        .lock_conn()
        .unwrap()
        .execute(
            "INSERT INTO folder_meta (key, value) VALUES ('rebuild_retry', ?1) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![legacy],
        )
        .unwrap();

    let status = index
        .model_status()
        .expect("legacy retry record must remain readable");
    assert!(
        !status.halted,
        "generation default did not expose auto-clear"
    );
    let retry = model_meta::read_persisted_rebuild_retry(&index.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(retry.failure_generation, 0);

    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_millis(10));
    mgr.probe_folder_model_for_test("legacy-retry", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("legacy-retry").await;
    let recovered = index.model_status().unwrap();
    assert_eq!(recovered.index_state, IndexModelState::Ready);
    assert_eq!(recovered.unproven_vectors_since_gen, None);
    assert!(
        model_meta::read_persisted_rebuild_retry(&index.lock_conn().unwrap())
            .unwrap()
            .is_none(),
        "legacy retry was not auto-cleared"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_upsert_cannot_rewrite_model_or_trigger_periodic_rebuilds() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "bulk-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(short, bulk)
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_secs(10 * 60),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(30 * 60),
            std::time::Duration::from_secs(6 * 60 * 60),
            3,
        );
    mgr.upsert_folder_file(
        "upsert-guard",
        32,
        &one_chunk("a.md", "Stable indexed text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "bulk-model".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("upsert-guard").await;

    for version in 2..=(24 * 6 + 1) {
        let mut foreign = one_chunk("a.md", "Foreign client text.", basis(64, 2));
        foreign.file_version = format!("v{version}");
        let err = mgr
            .upsert_folder_file(
                "upsert-guard",
                32,
                &foreign,
                Some(&UpsertModelMeta {
                    model: "client-model".into(),
                    embed_dim: 64,
                    bulk_alias: "bulk-alias".into(),
                    short_alias: Some("short-alias".into()),
                }),
            )
            .await
            .expect_err("foreign model metadata must be rejected");
        assert!(matches!(
            err,
            crate::error::MakakooError::RetryableFolderModelMismatch(_)
        ));

        mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(10 * 60));
        let _ = mgr
            .search_folder(
                FolderSearchRequest {
                    folder_id: "upsert-guard".into(),
                    dim: 32,
                    k: 1,
                    query: Some("Stable".into()),
                    query_vector: Some(basis(64, 1)),
                    query_model: Some("bulk-model".into()),
                    query_dim: Some(64),
                    filter: SearchFilter::default(),
                },
                None,
                None,
            )
            .await
            .unwrap();
        mgr.wait_for_model_probe("upsert-guard").await;
        mgr.wait_for_rebuild("upsert-guard").await;
    }

    let status = mgr
        .get_or_open_without_probe_for_test("upsert-guard", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("bulk-model"));
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ingest_only_bulk_switch_probes_synchronously_and_rebuilds_once() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let make_manager = || {
        FolderIndexManager::new(tmp.path())
            .with_embedders(Arc::clone(&emb), Arc::clone(&emb))
            .with_model_recovery_timings_for_test(
                std::time::Duration::from_secs(1),
                std::time::Duration::from_millis(50),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(10),
                3,
            )
    };

    let seed = make_manager();
    seed.upsert_folder_file(
        "ingest-only",
        32,
        &one_chunk("a.md", "Initial A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    seed.wait_for_model_probe("ingest-only").await;
    drop(seed);

    responder.set_both_model("model-b", 64, 2);
    let first_short_lived = make_manager();
    first_short_lived
        .upsert_folder_file(
            "ingest-only",
            32,
            &one_chunk("b1.md", "First B ingest.", basis(64, 2)),
            Some(&UpsertModelMeta {
                model: "model-b".into(),
                embed_dim: 64,
                bulk_alias: "test-embed".into(),
                short_alias: Some("test-embed".into()),
            }),
        )
        .await
        .expect("the rejected upsert must synchronously probe and retry once");
    first_short_lived.wait_for_model_probe("ingest-only").await;
    drop(first_short_lived);

    let second_short_lived = Arc::new(make_manager());
    second_short_lived.advance_model_recovery_time_for_test(std::time::Duration::from_secs(2));
    responder.gate_entered.store(false, Ordering::SeqCst);
    responder.gate_release.store(false, Ordering::SeqCst);
    responder.gate_bulk.store(true, Ordering::SeqCst);
    let ingest_mgr = Arc::clone(&second_short_lived);
    let ingest = tokio::spawn(async move {
        ingest_mgr
            .upsert_folder_file(
                "ingest-only",
                32,
                &one_chunk("b2.md", "Second B ingest.", basis(64, 2)),
                Some(&UpsertModelMeta {
                    model: "model-b".into(),
                    embed_dim: 64,
                    bulk_alias: "test-embed".into(),
                    short_alias: Some("test-embed".into()),
                }),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned repair never reached the bulk call");
    assert!(
        !ingest.is_finished(),
        "short-lived ingest returned while its repair was still running"
    );
    responder.gate_bulk.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    ingest.await.unwrap().unwrap();

    // No explicit wait: a short-lived caller may exit as soon as ingest
    // returns. The ingest path itself must keep the owned repair alive through
    // Ready publication.
    let idx = second_short_lived
        .get_or_open_without_probe_for_test("ingest-only", 32)
        .unwrap();
    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-b"));
    assert_eq!(status.unproven_vectors_since_gen, None);
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_bulk_switch_accepts_new_model_upsert_and_rebuilds_once() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(short, bulk)
        .with_model_recovery_timings_for_test(
            std::time::Duration::from_secs(10 * 60),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(30 * 60),
            std::time::Duration::from_secs(6 * 60 * 60),
            3,
        );
    mgr.upsert_folder_file(
        "observed-upsert",
        32,
        &one_chunk("a.md", "Original text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("observed-upsert").await;

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("observed-upsert", 32)
        .await
        .unwrap();
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);

    mgr.upsert_folder_file(
        "observed-upsert",
        32,
        &one_chunk("b.md", "New-model text.", basis(64, 2)),
        Some(&UpsertModelMeta {
            model: "model-b".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .expect("the latest observed bulk pair must be accepted");
    mgr.wait_for_model_probe("observed-upsert").await;
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("observed-upsert", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("observed-upsert").await;

    let status = mgr
        .get_or_open_without_probe_for_test("observed-upsert", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-b"));
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);

    let err = mgr
        .upsert_folder_file(
            "observed-upsert",
            32,
            &one_chunk("c.md", "Foreign text.", basis(64, 3)),
            Some(&UpsertModelMeta {
                model: "model-c".into(),
                embed_dim: 64,
                bulk_alias: "bulk-alias".into(),
                short_alias: Some("short-alias".into()),
            }),
        )
        .await
        .expect_err("a pair that is neither stored nor observed must be rejected");
    assert!(matches!(
        err,
        crate::error::MakakooError::RetryableFolderModelMismatch(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_change_confirmation_requires_elapsed_window() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "confirmation-window",
        32,
        &one_chunk("a.md", "Confirmation text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("confirmation-window").await;

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("confirmation-window", 32)
        .await
        .unwrap();
    mgr.probe_folder_model_for_test("confirmation-window", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("confirmation-window").await;
    assert_eq!(
        responder.bulk_calls.load(Ordering::SeqCst),
        0,
        "a count-only second sighting confirmed the change"
    );

    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(59));
    mgr.probe_folder_model_for_test("confirmation-window", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("confirmation-window").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);

    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(1));
    mgr.probe_folder_model_for_test("confirmation-window", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("confirmation-window").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        mgr.get_or_open_without_probe_for_test("confirmation-window", 32)
            .unwrap()
            .model_status()
            .unwrap()
            .embed_model
            .as_deref(),
        Some("model-b")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn varying_failure_text_persists_backoff_and_halt_until_new_ingest() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let make_manager = || {
        FolderIndexManager::new(tmp.path())
            .with_embedders(Arc::clone(&short), Arc::clone(&bulk))
            .with_model_recovery_timings_for_test(
                std::time::Duration::from_millis(10),
                std::time::Duration::from_millis(1),
                std::time::Duration::from_millis(30),
                std::time::Duration::from_secs(60),
                3,
            )
    };
    let mgr = make_manager();
    mgr.upsert_folder_file(
        "persisted-failure",
        32,
        &one_chunk("a.md", "Failure text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("persisted-failure").await;
    responder.set_both_model("model-b", 64, 2);
    responder.fail_bulk_embeddings.store(true, Ordering::SeqCst);
    responder.vary_failure_body.store(true, Ordering::SeqCst);
    mgr.probe_folder_model_for_test("persisted-failure", 32)
        .await
        .unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_millis(10));
    mgr.probe_folder_model_for_test("persisted-failure", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("persisted-failure").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
    let idx = mgr
        .get_or_open_without_probe_for_test("persisted-failure", 32)
        .unwrap();
    let mut retry = model_meta::read_persisted_rebuild_retry(&idx.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    retry.not_before_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    model_meta::write_persisted_rebuild_retry(&idx.lock_conn().unwrap(), &retry).unwrap();
    drop(idx);
    drop(mgr);

    let mgr = make_manager();
    mgr.get_or_open("persisted-failure", 32).unwrap();
    mgr.wait_for_model_probe("persisted-failure").await;
    mgr.wait_for_rebuild("persisted-failure").await;
    assert_eq!(
        responder.bulk_calls.load(Ordering::SeqCst),
        1,
        "restart ignored persisted backoff"
    );

    let idx = mgr
        .get_or_open_without_probe_for_test("persisted-failure", 32)
        .unwrap();
    let mut retry = model_meta::read_persisted_rebuild_retry(&idx.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    retry.not_before_unix_ms = 0;
    model_meta::write_persisted_rebuild_retry(&idx.lock_conn().unwrap(), &retry).unwrap();
    drop(idx);
    drop(mgr);

    let mgr = make_manager();
    // The persisted observation was recorded on the previous manager's
    // simulated clock. Advance the replacement manager past that confirmation
    // window instead of relying on host scheduling latency.
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60 * 60 + 1));
    mgr.probe_folder_model_for_test("persisted-failure", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("persisted-failure").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 2);
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60 * 60 + 1));
    mgr.probe_folder_model_for_test("persisted-failure", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("persisted-failure").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 3);
    let idx = mgr
        .get_or_open_without_probe_for_test("persisted-failure", 32)
        .unwrap();
    let retry = model_meta::read_persisted_rebuild_retry(&idx.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(retry.error_class, "http_503");
    assert_eq!(retry.failures, 3);
    assert!(retry.halted);
    assert!(idx
        .model_status()
        .unwrap()
        .stale_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("transient failure 3")));
    drop(idx);
    drop(mgr);

    let mgr = make_manager();
    mgr.upsert_folder_file(
        "persisted-failure",
        32,
        &one_chunk("b.md", "Ingest after halt.", basis(64, 2)),
        Some(&UpsertModelMeta {
            model: "model-b".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .expect("a halted rebuild must not halt ingest from the observed bulk model");
    mgr.wait_for_model_probe("persisted-failure").await;
    mgr.wait_for_rebuild("persisted-failure").await;
    assert_eq!(
        responder.bulk_calls.load(Ordering::SeqCst),
        4,
        "a newer provenance flag must clear the older halt and allow one retry"
    );
    let idx = mgr
        .get_or_open_without_probe_for_test("persisted-failure", 32)
        .unwrap();
    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert!(!status.halted);
    assert!(status.unproven_vectors_since_gen.is_some());
    let retry = model_meta::read_persisted_rebuild_retry(&idx.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(retry.failures, 1);
    assert!(!retry.halted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_query_claim_stale_reason_is_cleared_when_bulk_matches() {
    let tmp = TempDir::new().unwrap();
    let seed = FolderIndexManager::new(tmp.path());
    for (folder, reason) in [
        ("legacy-model", "model_mismatch: index=stable query=client"),
        ("legacy-dim", "dim_mismatch: index=64 query=48"),
    ] {
        seed.upsert_folder_file(
            folder,
            32,
            &one_chunk("a.md", "Legacy text.", basis(64, 1)),
            Some(&UpsertModelMeta {
                model: "stable".into(),
                embed_dim: 64,
                bulk_alias: "bulk-alias".into(),
                short_alias: Some("short-alias".into()),
            }),
        )
        .await
        .unwrap();
        seed.get_or_open(folder, 32)
            .unwrap()
            .mark_stale(reason)
            .unwrap();
    }
    drop(seed);

    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "stable", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    for folder in ["legacy-model", "legacy-dim"] {
        let idx = mgr.get_or_open(folder, 32).unwrap();
        mgr.wait_for_model_probe(folder).await;
        let status = idx.model_status().unwrap();
        assert_eq!(status.index_state, IndexModelState::Ready, "{folder}");
        assert_eq!(status.stale_reason, None, "{folder}");
    }
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_mismatch_diagnostic_is_bounded_and_never_persisted() {
    let tmp = TempDir::new().unwrap();
    let mgr = FolderIndexManager::new(tmp.path());
    mgr.upsert_folder_file(
        "diagnostic",
        32,
        &one_chunk("a.md", "Diagnostic text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "stable".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: None,
        }),
    )
    .await
    .unwrap();
    let long_model = "é".repeat(500);
    let _ = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "diagnostic".into(),
                dim: 32,
                k: 1,
                query: Some("Diagnostic".into()),
                query_vector: Some(basis(64, 2)),
                query_model: Some(long_model),
                query_dim: Some(64),
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    let idx = mgr
        .get_or_open_without_probe_for_test("diagnostic", 32)
        .unwrap();
    let reason = idx.model_status().unwrap().query_mismatch_reason.unwrap();
    assert!(reason.len() <= 256);
    let persisted: i64 = idx
        .lock_conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM folder_meta WHERE key = 'query_mismatch_reason'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_rebuild_is_marked_stale_and_resumes() {
    let tmp = TempDir::new().unwrap();
    let seed = FolderIndexManager::new(tmp.path());
    seed.upsert_folder_file(
        "interrupted",
        32,
        &one_chunk("a.md", "Interrupted rebuild text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "stable-model".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    seed.upsert_folder_file(
        "interrupted",
        32,
        &one_chunk("b.md", "Second interrupted row.", basis(64, 2)),
        Some(&UpsertModelMeta {
            model: "stable-model".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    let seeded = seed.get_or_open("interrupted", 32).unwrap();
    seeded.mark_stale("rebuild_started").unwrap();
    {
        let conn = seeded.lock_conn().unwrap();
        model_meta::write_model_state(&conn, IndexModelState::Rebuilding, Some("rebuild_started"))
            .unwrap();
    }
    drop(seeded);
    drop(seed);

    let reopened = FolderIndex::open_with_config(
        tmp.path(),
        "interrupted",
        FolderIndexConfig::with_dim(32)
            .with_embed_aliases(Some("test-embed".into()), Some("test-embed".into())),
    )
    .unwrap();
    let reopened_status = reopened.model_status().unwrap();
    assert_eq!(reopened_status.index_state, IndexModelState::Stale);
    assert_eq!(
        reopened_status.stale_reason.as_deref(),
        Some("interrupted_rebuild")
    );
    drop(reopened);

    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "stable-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    let response = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "interrupted".into(),
                dim: 32,
                k: 5,
                query: Some("Interrupted".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::Fts);
    assert!(response.stale);
    mgr.wait_for_model_probe("interrupted").await;
    mgr.wait_for_rebuild("interrupted").await;

    let status = mgr
        .get_or_open("interrupted", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn short_vs_bulk_mismatch_stays_stale_fts_only_without_rebuild_loop() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-a", "bulk-b", "model-a", 32);
    *responder.bulk_model.lock().unwrap() = "model-b".to_string();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-a"));
    let bulk_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-b"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path())
            .with_embedders(Arc::clone(&short_emb), Arc::clone(&bulk_emb)),
    );

    mgr.upsert_folder_file(
        "mismatch",
        32,
        &UpsertFileRequest {
            file_path: "guide.md".into(),
            file_version: "v1".into(),
            source_kind: SourceKind::File,
            trust: TrustLevel::Trusted,
            owner: "alice".into(),
            chunks: vec![ChunkInput {
                anchor: Anchor {
                    kind: AnchorKind::Heading,
                    value: "Setup".into(),
                },
                char_start: 0,
                char_end: 34,
                text: "Zeppelin deployment guide steps.".into(),
                vector: basis(32, 0),
            }],
        },
        Some(&UpsertModelMeta {
            model: "model-b".into(),
            embed_dim: 32,
            bulk_alias: "bulk-b".into(),
            short_alias: Some("short-a".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("mismatch").await;
    let probes_after_open = responder.bulk_probe_calls.load(Ordering::SeqCst);

    // The first-open probe records the endpoint split. Searches stay FTS-only
    // and do not pay another pair of probe calls inside the rate-limit window.
    let res1 = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "mismatch".into(),
                dim: 32,
                k: 5,
                query: Some("Zeppelin".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    mgr.wait_for_rebuild("mismatch").await;

    assert_eq!(res1.mode, SearchMode::Fts);
    assert!(res1.stale);
    assert_eq!(res1.passages.len(), 1);

    let idx = mgr.get_or_open("mismatch", 32).unwrap();
    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(
        status.stale_reason.as_deref(),
        Some(SHORT_BULK_MISMATCH_REASON)
    );

    let calls_before = responder.bulk_calls.load(Ordering::SeqCst);
    let res2 = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "mismatch".into(),
                dim: 32,
                k: 5,
                query: Some("Zeppelin".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    mgr.wait_for_rebuild("mismatch").await;
    assert_eq!(res2.mode, SearchMode::Fts);
    assert!(res2.stale);
    assert!(!res2.rebuilding);
    assert_eq!(
        responder.bulk_probe_calls.load(Ordering::SeqCst),
        probes_after_open
    );
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), calls_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_model_different_dim_marks_stale_and_rebuilds() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "qwen3-0.6b", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb)),
    );

    mgr.upsert_folder_file(
        "dim-test",
        32,
        &UpsertFileRequest {
            file_path: "a.md".into(),
            file_version: "v1".into(),
            source_kind: SourceKind::File,
            trust: TrustLevel::Trusted,
            owner: "alice".into(),
            chunks: vec![ChunkInput {
                anchor: Anchor {
                    kind: AnchorKind::Heading,
                    value: "Intro".into(),
                },
                char_start: 0,
                char_end: 20,
                text: "Dimensionality test.".into(),
                vector: basis(64, 2),
            }],
        },
        Some(&UpsertModelMeta {
            model: "qwen3-0.6b".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("dim-test").await;
    mgr.upsert_folder_file(
        "dim-test",
        32,
        &one_chunk("b.md", "Second dimensionality row.", basis(64, 3)),
        Some(&UpsertModelMeta {
            model: "qwen3-0.6b".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    // Same model name, changed pre-truncation dim (64 -> 48).
    responder.set_both_model("qwen3-0.6b", 48, 4);
    responder.gate_bulk.store(true, Ordering::SeqCst);
    mgr.probe_folder_model_for_test("dim-test", 32)
        .await
        .unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("dim-test", 32)
        .await
        .unwrap();
    let res = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "dim-test".into(),
                dim: 32,
                k: 5,
                query: Some("Dimensionality".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(res.mode, SearchMode::Fts);
    assert!(res.stale);

    responder.gate_bulk.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    mgr.wait_for_rebuild("dim-test").await;
    let status = mgr
        .get_or_open("dim-test", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_dim, Some(48));
}

#[test]
fn probe_stale_mark_never_hides_a_running_rebuild() {
    let tmp = TempDir::new().unwrap();
    let idx = FolderIndex::open(tmp.path(), "race", 8).unwrap();

    {
        let conn = idx.lock_conn().unwrap();
        model_meta::write_model_state(
            &conn,
            IndexModelState::Rebuilding,
            Some("model_changed_on_probe"),
        )
        .unwrap();
    }
    assert!(!idx
        .mark_stale_unless_rebuilding("model_changed_on_probe")
        .unwrap());
    assert_eq!(
        idx.model_status().unwrap().index_state,
        IndexModelState::Rebuilding
    );

    {
        let mut conn = idx.lock_conn().unwrap();
        let status = model_meta::read_model_status(&conn).unwrap();
        let generation = super::sqlite_repo::read_generation(&conn).unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        assert!(model_meta::publish_ready_model_meta_tx(
            &tx,
            &model_meta::ReadyPublication {
                expected_generation: generation,
                expected_state: status.index_state,
                expected_reason: status.stale_reason.as_deref(),
                model: status.embed_model.as_deref(),
                embed_dim: status.embed_dim,
                bulk_alias: status.embed_alias.as_deref(),
                short_alias: status.short_embed_alias.as_deref(),
                final_stale_reason: None,
                kind: model_meta::ReadyPublicationKind::Ordinary,
            }
        )
        .unwrap());
        tx.commit().unwrap();
    }
    assert!(idx
        .mark_stale_unless_rebuilding("model_changed_on_probe")
        .unwrap());
    let st = idx.model_status().unwrap();
    assert_eq!(st.index_state, IndexModelState::Stale);
    assert_eq!(st.stale_reason.as_deref(), Some("model_changed_on_probe"));
}

#[test]
fn ready_publication_door_refuses_sticky_unproven_vectors() {
    let tmp = TempDir::new().unwrap();
    let idx = FolderIndex::open(tmp.path(), "sticky-door", 32).unwrap();
    idx.upsert_file_with_model(
        &one_chunk("a.md", "Stored A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .unwrap();
    {
        let mut conn = idx.lock_conn().unwrap();
        model_meta::record_observed_bulk(&mut conn, "model-b", 64, 1).unwrap();
    }
    idx.upsert_file_with_model(
        &one_chunk("b.md", "Observed B text.", basis(64, 2)),
        Some(&UpsertModelMeta {
            model: "model-b".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .unwrap();
    {
        let conn = idx.lock_conn().unwrap();
        model_meta::write_model_state(
            &conn,
            IndexModelState::Stale,
            Some(SHORT_BULK_MISMATCH_REASON),
        )
        .unwrap();
    }
    let mut conn = idx.lock_conn().unwrap();
    let status = model_meta::read_model_status(&conn).unwrap();
    let generation = super::sqlite_repo::read_generation(&conn).unwrap();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(!model_meta::publish_ready_model_meta_tx(
        &tx,
        &model_meta::ReadyPublication {
            expected_generation: generation,
            expected_state: status.index_state,
            expected_reason: status.stale_reason.as_deref(),
            model: Some("model-a"),
            embed_dim: Some(64),
            bulk_alias: Some("bulk-alias"),
            short_alias: Some("short-alias"),
            final_stale_reason: None,
            kind: model_meta::ReadyPublicationKind::Ordinary,
        }
    )
    .unwrap());
    tx.commit().unwrap();
    drop(conn);
    let refused = idx.model_status().unwrap();
    assert_eq!(refused.index_state, IndexModelState::Stale);
    assert!(refused.unproven_vectors_since_gen.is_some());
}

#[test]
fn ready_publication_door_compare_and_sets_generation() {
    let tmp = TempDir::new().unwrap();
    let idx = FolderIndex::open(tmp.path(), "generation-door", 32).unwrap();
    let meta = UpsertModelMeta {
        model: "model-a".into(),
        embed_dim: 64,
        bulk_alias: "bulk-alias".into(),
        short_alias: Some("short-alias".into()),
    };
    idx.upsert_file_with_model(
        &one_chunk("a.md", "Stored A text.", basis(64, 1)),
        Some(&meta),
    )
    .unwrap();
    idx.mark_stale("bulk_model_change_pending").unwrap();
    let before = idx.model_status().unwrap();
    let old_generation = super::sqlite_repo::read_generation(&idx.lock_conn().unwrap()).unwrap();

    idx.upsert_file_with_model(
        &one_chunk("a2.md", "Another A text.", basis(64, 1)),
        Some(&meta),
    )
    .unwrap();
    let mut conn = idx.lock_conn().unwrap();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(!model_meta::publish_ready_model_meta_tx(
        &tx,
        &model_meta::ReadyPublication {
            expected_generation: old_generation,
            expected_state: before.index_state,
            expected_reason: before.stale_reason.as_deref(),
            model: Some("model-a"),
            embed_dim: Some(64),
            bulk_alias: Some("bulk-alias"),
            short_alias: Some("short-alias"),
            final_stale_reason: None,
            kind: model_meta::ReadyPublicationKind::Ordinary,
        }
    )
    .unwrap());
    tx.commit().unwrap();
    drop(conn);
    assert_eq!(
        idx.model_status().unwrap().index_state,
        IndexModelState::Stale
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_upsert_flap_back_rebuilds_stored_space_before_ready() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "flap-upsert",
        32,
        &one_chunk("a.md", "Original A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("flap-upsert").await;

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("flap-upsert", 32)
        .await
        .unwrap();
    mgr.upsert_folder_file(
        "flap-upsert",
        32,
        &one_chunk("b.md", "Observed B text.", basis(64, 2)),
        Some(&UpsertModelMeta {
            model: "model-b".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("flap-upsert").await;
    let idx = mgr
        .get_or_open_without_probe_for_test("flap-upsert", 32)
        .unwrap();
    let pending = idx.model_status().unwrap();
    assert_eq!(pending.index_state, IndexModelState::Stale);
    assert_eq!(
        pending.stale_reason.as_deref(),
        Some(model_meta::OBSERVED_BULK_UPSERT_REASON)
    );

    // Exact r5 sequence: bulk returns to A while short reports C. The
    // metadata-only short mismatch must not erase B-vector provenance.
    responder.set_bulk_model("model-a", 64, 1);
    responder.set_short_model("model-c", 64, 3);
    mgr.probe_folder_model_for_test("flap-upsert", 32)
        .await
        .unwrap();
    let after_short_c = idx.model_status().unwrap();
    assert_eq!(after_short_c.index_state, IndexModelState::Stale);
    assert_eq!(
        after_short_c.stale_reason.as_deref(),
        Some(model_meta::OBSERVED_BULK_UPSERT_REASON)
    );
    assert!(after_short_c.unproven_vectors_since_gen.is_some());

    responder.gate_entered.store(false, Ordering::SeqCst);
    responder.gate_release.store(false, Ordering::SeqCst);
    responder.gate_bulk.store(true, Ordering::SeqCst);
    responder.set_both_model("model-a", 64, 1);
    mgr.probe_folder_model_for_test("flap-upsert", 32)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rebuild did not reach the bulk responder gate");
    assert_ne!(
        idx.model_status().unwrap().index_state,
        IndexModelState::Ready,
        "the flap probe published Ready before removing model-b vectors"
    );
    responder.gate_bulk.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    mgr.wait_for_rebuild("flap-upsert").await;

    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-a"));
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
    let expected_quantized =
        crate::superbrain::folder_index::quantize::truncate_and_quantize_i8(&basis(64, 1), 32)
            .unwrap();
    let expected =
        crate::superbrain::folder_index::quantize::i8_slice_to_bytes(&expected_quantized);
    let non_a: i64 = idx
        .lock_conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE vector_i8 IS NOT ?1",
            [&expected],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(non_a, 0, "Ready(model-a) retained a model-b vector");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_promotion_cas_rejects_upsert_between_probe_read_and_write() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "promotion-cas",
        32,
        &one_chunk("a.md", "Original A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("promotion-cas").await;

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("promotion-cas", 32)
        .await
        .unwrap();
    let idx = mgr
        .get_or_open_without_probe_for_test("promotion-cas", 32)
        .unwrap();
    let fired = Arc::new(AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    *idx.before_ready_publish.lock().unwrap() = Some(Box::new(move |index: &FolderIndex| {
        if !hook_fired.swap(true, Ordering::SeqCst) {
            {
                let mut conn = index.lock_conn().unwrap();
                model_meta::record_observed_bulk(&mut conn, "model-b", 64, u64::MAX).unwrap();
            }
            index
                .upsert_file_with_model(
                    &one_chunk("b.md", "B landed during promotion.", basis(64, 2)),
                    Some(&UpsertModelMeta {
                        model: "model-b".into(),
                        embed_dim: 64,
                        bulk_alias: "bulk-alias".into(),
                        short_alias: Some("short-alias".into()),
                    }),
                )
                .unwrap();
        }
    }));

    responder.set_both_model("model-a", 64, 1);
    responder.gate_after_bulk_calls.store(0, Ordering::SeqCst);
    responder.gate_bulk.store(true, Ordering::SeqCst);
    mgr.probe_folder_model_for_test("promotion-cas", 32)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("CAS recovery rebuild never reached the bulk responder gate");
    assert!(fired.load(Ordering::SeqCst));
    let after_race = idx.model_status().unwrap();
    assert_ne!(after_race.index_state, IndexModelState::Ready);
    assert_eq!(
        after_race.stale_reason.as_deref(),
        Some(model_meta::OBSERVED_BULK_UPSERT_REASON)
    );
    assert!(after_race.unproven_vectors_since_gen.is_some());

    responder.gate_bulk.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    mgr.wait_for_rebuild("promotion-cas").await;
    let recovered = idx.model_status().unwrap();
    assert_eq!(recovered.index_state, IndexModelState::Ready);
    assert_eq!(recovered.embed_model.as_deref(), Some("model-a"));
    assert_eq!(recovered.unproven_vectors_since_gen, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unnamed_index_rejects_foreign_claim_before_bulk_naming() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder)
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    mgr.upsert_folder_file(
        "unnamed-laundering",
        32,
        &one_chunk("a.md", "Stored A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("unnamed-laundering").await;
    let idx = mgr
        .get_or_open_without_probe_for_test("unnamed-laundering", 32)
        .unwrap();

    // Simulate a legacy unnamed index that still has a known dimension and an
    // observed bulk pair. A client claim must not become provenance by fiat.
    idx.lock_conn()
        .unwrap()
        .execute("DELETE FROM folder_meta WHERE key = 'embed_model'", [])
        .unwrap();
    let err = idx
        .upsert_file_with_model(
            &one_chunk("x.md", "Foreign X text.", basis(64, 4)),
            Some(&UpsertModelMeta {
                model: "client-x".into(),
                embed_dim: 64,
                bulk_alias: "test-embed".into(),
                short_alias: Some("test-embed".into()),
            }),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        crate::error::MakakooError::RetryableFolderModelMismatch(_)
    ));
    let foreign_rows: i64 = idx
        .lock_conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE file_path = 'x.md'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(foreign_rows, 0);

    idx.upsert_file_with_model(
        &one_chunk("a2.md", "Observed A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .unwrap();
    let contaminated = idx.model_status().unwrap();
    assert_eq!(contaminated.index_state, IndexModelState::Stale);
    assert!(contaminated.unproven_vectors_since_gen.is_some());

    mgr.probe_folder_model_for_test("unnamed-laundering", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("unnamed-laundering").await;
    let named = idx.model_status().unwrap();
    assert_eq!(named.index_state, IndexModelState::Ready);
    assert_eq!(named.embed_model.as_deref(), Some("model-a"));
    assert_eq!(named.unproven_vectors_since_gen, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_pair_upsert_during_rebuilding_is_reembedded_before_ready() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&short), Arc::clone(&bulk));
    mgr.upsert_folder_file(
        "rebuilding-upsert",
        32,
        &one_chunk("a.md", "Original A text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("rebuilding-upsert").await;

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("rebuilding-upsert", 32)
        .await
        .unwrap();
    let idx = mgr
        .get_or_open_without_probe_for_test("rebuilding-upsert", 32)
        .unwrap();
    let fired = Arc::new(AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    let observed_b = UpsertModelMeta {
        model: "model-b".into(),
        embed_dim: 64,
        bulk_alias: "bulk-alias".into(),
        short_alias: Some("short-alias".into()),
    };
    *idx.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |index: &FolderIndex| {
        if !hook_fired.swap(true, Ordering::SeqCst) {
            index
                .upsert_file_with_model(
                    &one_chunk("b.md", "Observed B during rebuild.", basis(64, 2)),
                    Some(&observed_b),
                )
                .expect("the observed pair must remain ingestible while rebuilding");
        }
    }));
    responder.set_both_model("model-a", 64, 1);
    mgr.request_folder_rebuild("rebuilding-upsert", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("rebuilding-upsert").await;
    assert!(
        fired.load(Ordering::SeqCst),
        "rebuild commit hook never ran"
    );

    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-a"));
    let expected_quantized =
        crate::superbrain::folder_index::quantize::truncate_and_quantize_i8(&basis(64, 1), 32)
            .unwrap();
    let expected =
        crate::superbrain::folder_index::quantize::i8_slice_to_bytes(&expected_quantized);
    let non_a: i64 = idx
        .lock_conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE vector_i8 IS NOT ?1",
            [&expected],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(non_a, 0, "rebuild committed over a model-b vector");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_pair_upserts_expedite_only_one_probe() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "one-expedite",
        32,
        &one_chunk("a.md", "Original text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("one-expedite").await;
    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("one-expedite", 32)
        .await
        .unwrap();
    let before_upserts = responder.bulk_probe_calls.load(Ordering::SeqCst);

    mgr.upsert_folder_file(
        "one-expedite",
        32,
        &one_chunk("b0.md", "First B text.", basis(64, 2)),
        Some(&UpsertModelMeta {
            model: "model-b".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("one-expedite").await;
    assert_eq!(
        responder.bulk_probe_calls.load(Ordering::SeqCst),
        before_upserts + 1
    );

    for i in 1..=5 {
        mgr.upsert_folder_file(
            "one-expedite",
            32,
            &one_chunk(&format!("b{i}.md"), "More B text.", basis(64, 2)),
            Some(&UpsertModelMeta {
                model: "model-b".into(),
                embed_dim: 64,
                bulk_alias: "bulk-alias".into(),
                short_alias: Some("short-alias".into()),
            }),
        )
        .await
        .unwrap();
        mgr.wait_for_model_probe("one-expedite").await;
    }
    assert_eq!(
        responder.bulk_probe_calls.load(Ordering::SeqCst),
        before_upserts + 1,
        "pending upserts repeatedly expedited the same observed pair"
    );
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_observed_bulk_json_self_heals_with_defaulted_fields() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    mgr.upsert_folder_file(
        "corrupt-observed",
        32,
        &one_chunk("a.md", "Corrupt metadata text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("corrupt-observed").await;
    let idx = mgr
        .get_or_open_without_probe_for_test("corrupt-observed", 32)
        .unwrap();
    idx.lock_conn()
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO folder_meta (key, value) VALUES ('observed_bulk', '{bad json')",
            [],
        )
        .unwrap();
    assert_eq!(
        model_meta::read_observed_bulk(&idx.lock_conn().unwrap()).unwrap(),
        None
    );

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("corrupt-observed", 32)
        .await
        .unwrap();
    let observed = model_meta::read_observed_bulk(&idx.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(observed.model, "model-b");
    assert_eq!(observed.dim, 64);
    assert_eq!(observed.consecutive, 1);
    assert!(observed.first_seen_unix_ms > 0);
    assert_eq!(observed.last_seen_unix_ms, observed.first_seen_unix_ms);

    let old_shape: model_meta::ObservedBulkModel =
        serde_json::from_str(r#"{"model":"legacy","dim":12,"consecutive":1}"#).unwrap();
    assert_eq!(old_shape.first_seen_unix_ms, 0);
    assert_eq!(old_shape.last_seen_unix_ms, 0);
    assert!(!old_shape.probe_expedited);

    let old_retry: model_meta::PersistedRebuildRetry =
        serde_json::from_str(r#"{"model":"legacy","dim":12}"#).unwrap();
    assert_eq!(old_retry.error_class, "");
    assert_eq!(old_retry.failures, 0);
    assert!(!old_retry.halted);
    assert_eq!(old_retry.not_before_unix_ms, 0);
}

#[test]
fn observed_bulk_record_uses_immediate_transaction() {
    let tmp = TempDir::new().unwrap();
    let idx = FolderIndex::open(tmp.path(), "immediate-observation", 8).unwrap();
    let trace = OBSERVED_BULK_SQL_TRACE.get_or_init(|| Mutex::new(Vec::new()));
    trace.lock().unwrap_or_else(|e| e.into_inner()).clear();
    let mut conn = idx.lock_conn().unwrap();
    conn.trace(Some(capture_observed_bulk_sql));
    model_meta::record_observed_bulk(&mut conn, "model-a", 64, 1).unwrap();
    conn.trace(None);
    let statements = trace.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        statements
            .iter()
            .any(|statement| statement.eq_ignore_ascii_case("BEGIN IMMEDIATE")),
        "observed-pair read/write did not start with BEGIN IMMEDIATE: {statements:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_call_override_never_records_observed_bulk() {
    let tmp = TempDir::new().unwrap();
    let configured = DynamicEmbedResponder::new("configured", "configured", "model-a", 64);
    let configured_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(configured.clone())
        .mount(&configured_server)
        .await;
    let override_responder = DynamicEmbedResponder::new("override", "override", "model-b", 64);
    let override_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(override_responder.clone())
        .mount(&override_server)
        .await;
    let configured_emb =
        Arc::new(EmbeddingClient::with_base_url(configured_server.uri()).with_model("configured"));
    let override_emb =
        Arc::new(EmbeddingClient::with_base_url(override_server.uri()).with_model("override"));
    let mgr = FolderIndexManager::new(tmp.path())
        .with_embedders(Arc::clone(&configured_emb), Arc::clone(&configured_emb));
    mgr.upsert_folder_file(
        "override-probe",
        32,
        &one_chunk("a.md", "Configured model text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "configured".into(),
            short_alias: Some("configured".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("override-probe").await;
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(10 * 60));
    mgr.search_folder(
        FolderSearchRequest {
            folder_id: "override-probe".into(),
            dim: 32,
            k: 1,
            query: Some("Configured".into()),
            query_vector: None,
            query_model: None,
            query_dim: None,
            filter: SearchFilter::default(),
        },
        Some(&override_emb),
        Some(&override_emb),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("override-probe").await;

    assert_eq!(
        override_responder.bulk_probe_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(configured.bulk_probe_calls.load(Ordering::SeqCst), 2);
    let idx = mgr
        .get_or_open_without_probe_for_test("override-probe", 32)
        .unwrap();
    let observed = model_meta::read_observed_bulk(&idx.lock_conn().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(observed.model, "model-a");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_rebuild_success_cap_survives_manager_restart() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let make_manager = || {
        FolderIndexManager::new(tmp.path())
            .with_embedders(Arc::clone(&short), Arc::clone(&bulk))
            .with_model_recovery_timings_for_test(
                std::time::Duration::from_secs(10 * 60),
                std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(30 * 60),
                std::time::Duration::from_secs(6 * 60 * 60),
                3,
            )
    };
    let mgr = make_manager();
    mgr.upsert_folder_file(
        "success-cap",
        32,
        &one_chunk("a.md", "Cap text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("success-cap").await;
    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("success-cap", 32)
        .await
        .unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("success-cap", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("success-cap").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
    drop(mgr);

    let mgr = make_manager();
    responder.set_both_model("model-c", 64, 3);
    mgr.probe_folder_model_for_test("success-cap", 32)
        .await
        .unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("success-cap", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("success-cap").await;
    assert_eq!(
        responder.bulk_calls.load(Ordering::SeqCst),
        1,
        "restart bypassed the persisted one-hour success cap"
    );
    let capped = mgr
        .get_or_open_without_probe_for_test("success-cap", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(capped.index_state, IndexModelState::Stale);
    assert_eq!(capped.embed_model.as_deref(), Some("model-b"));

    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60 * 60 + 1));
    mgr.probe_folder_model_for_test("success-cap", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("success-cap").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 2);
    let recovered = mgr
        .get_or_open_without_probe_for_test("success-cap", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(recovered.index_state, IndexModelState::Ready);
    assert_eq!(recovered.embed_model.as_deref(), Some("model-c"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn future_automatic_success_timestamp_is_expired_and_status_exposes_real_cap() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "future-cap",
        32,
        &one_chunk("a.md", "Future cap text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("future-cap").await;
    let idx = mgr
        .get_or_open_without_probe_for_test("future-cap", 32)
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    {
        let conn = idx.lock_conn().unwrap();
        model_meta::write_persisted_automatic_rebuild_success(
            &conn,
            &model_meta::PersistedAutomaticRebuildSuccess {
                last_success_unix_ms: now,
            },
        )
        .unwrap();
    }
    assert!(idx.model_status().unwrap().rebuild_deferred_until.is_some());
    {
        let conn = idx.lock_conn().unwrap();
        model_meta::write_persisted_automatic_rebuild_success(
            &conn,
            &model_meta::PersistedAutomaticRebuildSuccess {
                last_success_unix_ms: u64::MAX,
            },
        )
        .unwrap();
    }
    assert_eq!(
        idx.model_status().unwrap().rebuild_deferred_until,
        None,
        "a future wall-clock timestamp must not defer rebuilds"
    );

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("future-cap", 32)
        .await
        .unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("future-cap", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("future-cap").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        idx.model_status().unwrap().embed_model.as_deref(),
        Some("model-b")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_short_probe_preserves_short_bulk_stale_reason() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "bulk-model", 64);
    *responder.short_model.lock().unwrap() = "short-model".to_string();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "short-error",
        32,
        &one_chunk("a.md", "Short error text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "bulk-model".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("short-error").await;
    let idx = mgr
        .get_or_open_without_probe_for_test("short-error", 32)
        .unwrap();
    assert_eq!(
        idx.model_status().unwrap().stale_reason.as_deref(),
        Some(SHORT_BULK_MISMATCH_REASON)
    );

    responder.fail_short_probes.store(true, Ordering::SeqCst);
    mgr.probe_folder_model_for_test("short-error", 32)
        .await
        .unwrap();
    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(
        status.stale_reason.as_deref(),
        Some(SHORT_BULK_MISMATCH_REASON)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn short_bulk_state_is_written_inside_rebuild_commit_transaction() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "short-commit",
        32,
        &one_chunk("a.md", "Short commit text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("short-commit").await;
    let idx = mgr
        .get_or_open_without_probe_for_test("short-commit", 32)
        .unwrap();
    let trace = MODEL_META_SQL_TRACE.get_or_init(|| Mutex::new(Vec::new()));
    trace.lock().unwrap_or_else(|e| e.into_inner()).clear();
    idx.lock_conn().unwrap().trace(Some(capture_model_meta_sql));

    responder.set_bulk_model("model-b", 64, 2);
    responder.set_short_model("model-c", 64, 3);
    mgr.probe_folder_model_for_test("short-commit", 32)
        .await
        .unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("short-commit", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("short-commit").await;
    idx.lock_conn().unwrap().trace(None);
    assert_eq!(
        idx.model_status().unwrap().stale_reason.as_deref(),
        Some(SHORT_BULK_MISMATCH_REASON)
    );

    let statements = trace.lock().unwrap_or_else(|e| e.into_inner());
    let reason_pos = statements
        .iter()
        .position(|statement| statement.contains(SHORT_BULK_MISMATCH_REASON))
        .expect("trace did not contain the short/bulk stale write");
    let begin_pos = statements[..reason_pos]
        .iter()
        .rposition(|statement| statement.eq_ignore_ascii_case("BEGIN IMMEDIATE"))
        .expect("short/bulk stale write had no preceding IMMEDIATE transaction");
    let commit_pos = statements[reason_pos + 1..]
        .iter()
        .position(|statement| statement.eq_ignore_ascii_case("COMMIT"))
        .map(|offset| reason_pos + 1 + offset)
        .expect("short/bulk stale write had no following commit");
    assert!(begin_pos < reason_pos && reason_pos < commit_pos);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_rebuild_request_reports_already_running() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "already-running",
        32,
        &one_chunk("a.md", "Already running text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("already-running").await;

    responder.set_both_model("model-b", 64, 2);
    responder.gate_entered.store(false, Ordering::SeqCst);
    responder.gate_release.store(false, Ordering::SeqCst);
    responder.gate_bulk.store(true, Ordering::SeqCst);
    assert_eq!(
        mgr.request_folder_rebuild("already-running", 32)
            .await
            .unwrap(),
        FolderRebuildRequestOutcome::Started
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rebuild did not reach the bulk gate");
    assert_eq!(
        mgr.request_folder_rebuild("already-running", 32)
            .await
            .unwrap(),
        FolderRebuildRequestOutcome::AlreadyRunning
    );
    responder.gate_bulk.store(false, Ordering::SeqCst);
    responder.gate_release.store(true, Ordering::SeqCst);
    mgr.wait_for_rebuild("already-running").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panicking_rebuild_task_drop_guard_releases_active_entry() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    mgr.upsert_folder_file(
        "panic-rebuild",
        32,
        &one_chunk("a.md", "Panic rebuild text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mgr.wait_for_model_probe("panic-rebuild"),
    )
    .await
    .expect("initial rebuild-test probe leaked its active entry");
    let idx = mgr
        .get_or_open_without_probe_for_test("panic-rebuild", 32)
        .unwrap();
    idx.panic_in_rebuild.store(true, Ordering::SeqCst);
    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("panic-rebuild", 32)
        .await
        .unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("panic-rebuild", 32)
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mgr.wait_for_rebuild("panic-rebuild"),
    )
    .await
    .expect("panicking rebuild leaked the active entry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panicking_probe_task_drop_guard_releases_active_entry() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder)
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    mgr.upsert_folder_file(
        "panic-probe",
        32,
        &one_chunk("a.md", "Panic probe text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mgr.wait_for_model_probe("panic-probe"),
    )
    .await
    .expect("initial probe-test probe leaked its active entry");
    let idx = mgr
        .get_or_open_without_probe_for_test("panic-probe", 32)
        .unwrap();
    idx.panic_in_probe.store(true, Ordering::SeqCst);
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(10 * 60));
    mgr.get_or_open("panic-probe", 32).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mgr.wait_for_model_probe("panic-probe"),
    )
    .await
    .expect("panicking probe leaked the active entry");
}

#[tokio::test]
async fn rebuild_wait_returns_when_finish_lands_between_lookup_and_await() {
    let reg = Arc::new(rebuild::FolderRebuildRegistry::new());
    reg.try_acquire_for_test("f").expect("free");
    let r2 = Arc::clone(&reg);
    // The rebuild finishes exactly after wait() looked the folder up.
    *reg.after_lookup.lock().unwrap() = Some(Box::new(move || r2.finish("f")));
    tokio::time::timeout(std::time::Duration::from_secs(5), reg.wait("f"))
        .await
        .expect("wait() missed a finish between lookup and await");
}

fn one_chunk(file: &str, text: &str, vector: Vec<f32>) -> UpsertFileRequest {
    UpsertFileRequest {
        file_path: file.into(),
        file_version: "v1".into(),
        source_kind: SourceKind::File,
        trust: TrustLevel::Trusted,
        owner: "alice".into(),
        chunks: vec![ChunkInput {
            anchor: Anchor {
                kind: AnchorKind::Heading,
                value: "Intro".into(),
            },
            char_start: 0,
            char_end: text.len(),
            text: text.into(),
            vector,
        }],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_written_after_the_rebuild_tail_are_re_embedded_before_ready() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "old-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb)),
    );
    let old_meta = UpsertModelMeta {
        model: "old-model".into(),
        embed_dim: 64,
        bulk_alias: "test-embed".into(),
        short_alias: Some("test-embed".into()),
    };
    mgr.upsert_folder_file(
        "tail",
        32,
        &one_chunk("a.md", "Alpha text.", basis(64, 2)),
        Some(&old_meta),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("tail").await;

    // A writer with an old-model vector lands after every tail read, once,
    // right before the first commit attempt.
    let idx = mgr.get_or_open_without_probe_for_test("tail", 32).unwrap();
    let fired = Arc::new(AtomicBool::new(false));
    let (f2, meta2) = (Arc::clone(&fired), old_meta.clone());
    *idx.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |i: &FolderIndex| {
        if !f2.swap(true, Ordering::SeqCst) {
            i.upsert_file_with_model(&one_chunk("b.md", "Beta text.", basis(64, 2)), Some(&meta2))
                .unwrap();
        }
    }));

    responder.set_both_model("new-model", 64, 5);
    mgr.probe_folder_model_for_test("tail", 32).await.unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("tail", 32).await.unwrap();
    mgr.wait_for_rebuild("tail").await;
    assert!(fired.load(Ordering::SeqCst), "hook never ran");

    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("new-model"));
    let conn = idx.lock_conn().unwrap();
    let vec_of = |f: &str| -> Vec<u8> {
        conn.query_row(
            "SELECT vector_i8 FROM chunks WHERE file_path = ?1",
            [f],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        vec_of("b.md"),
        vec_of("a.md"),
        "b.md kept its old-model vector under Ready"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_rejects_page_response_from_a_different_model() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "page-response-model",
        32,
        &one_chunk("a.md", "Original text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("page-response-model").await;

    responder.set_both_model("model-b", 64, 2);
    responder.set_bulk_embedding_response_model(Some("model-c"));
    assert_eq!(
        mgr.request_folder_rebuild("page-response-model", 32)
            .await
            .unwrap(),
        FolderRebuildRequestOutcome::Started
    );
    mgr.wait_for_rebuild("page-response-model").await;

    let idx = mgr
        .get_or_open_without_probe_for_test("page-response-model", 32)
        .unwrap();
    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(status.embed_model.as_deref(), Some("model-a"));
    assert!(
        status
            .stale_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("model changed mid-rebuild")),
        "wrong-model page response must fail the rebuild: {status:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_commit_is_fenced_when_claim_owner_changes() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "commit-fence",
        32,
        &one_chunk("a.md", "Original text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("commit-fence").await;

    let idx = mgr
        .get_or_open_without_probe_for_test("commit-fence", 32)
        .unwrap();
    let fired = Arc::new(AtomicBool::new(false));
    let fired_hook = Arc::clone(&fired);
    *idx.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |index| {
        if !fired_hook.swap(true, Ordering::SeqCst) {
            let conn = index.lock_conn().unwrap();
            let raw: String = conn
                .query_row(
                    "SELECT value FROM folder_meta WHERE key = 'folder_rebuild_claim'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let mut claim: serde_json::Value = serde_json::from_str(&raw).unwrap();
            claim["owner"] = json!("replacement-owner");
            claim["lease_expires_unix_ms"] = json!(u64::MAX);
            conn.execute(
                "UPDATE folder_meta SET value = ?1 WHERE key = 'folder_rebuild_claim'",
                [serde_json::to_string(&claim).unwrap()],
            )
            .unwrap();
        }
    }));

    responder.set_both_model("model-b", 64, 2);
    assert_eq!(
        mgr.request_folder_rebuild("commit-fence", 32)
            .await
            .unwrap(),
        FolderRebuildRequestOutcome::Started
    );
    mgr.wait_for_rebuild("commit-fence").await;
    assert!(fired.load(Ordering::SeqCst), "commit hook never ran");

    let status = idx.model_status().unwrap();
    assert_ne!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-a"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_commit_requires_matching_claim_token_for_coverage_and_swap() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr = FolderIndexManager::new(tmp.path()).with_embedders(short, bulk);
    mgr.upsert_folder_file(
        "token-coverage",
        32,
        &one_chunk("a.md", "Original text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("token-coverage").await;

    let idx = mgr
        .get_or_open_without_probe_for_test("token-coverage", 32)
        .unwrap();
    let fired = Arc::new(AtomicBool::new(false));
    let fired_hook = Arc::clone(&fired);
    let old_q =
        crate::superbrain::folder_index::quantize::truncate_and_quantize_i8(&basis(64, 1), 32)
            .unwrap();
    let old_bytes = crate::superbrain::folder_index::quantize::i8_slice_to_bytes(&old_q).to_vec();
    *idx.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |index| {
        if !fired_hook.swap(true, Ordering::SeqCst) {
            index
                .lock_conn()
                .unwrap()
                .execute(
                    "UPDATE rebuild_vectors
                        SET claim_token = 'foreign-claim', vector_i8 = ?1",
                    [&old_bytes],
                )
                .unwrap();
        }
    }));

    responder.set_both_model("model-b", 64, 2);
    assert_eq!(
        mgr.request_folder_rebuild("token-coverage", 32)
            .await
            .unwrap(),
        FolderRebuildRequestOutcome::Started
    );
    mgr.wait_for_rebuild("token-coverage").await;
    assert!(fired.load(Ordering::SeqCst), "commit hook never ran");

    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-b"));
    let new_q =
        crate::superbrain::folder_index::quantize::truncate_and_quantize_i8(&basis(64, 2), 32)
            .unwrap();
    let new_bytes = crate::superbrain::folder_index::quantize::i8_slice_to_bytes(&new_q);
    let stored: Vec<u8> = idx
        .lock_conn()
        .unwrap()
        .query_row("SELECT vector_i8 FROM chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, new_bytes, "foreign-token vector was committed");
}

/// S078 C11 (review r1 HIGH): the rebuild pages through the folder, parks each
/// page's vectors on disk and swaps them in at commit. A folder larger than one
/// page must come out fully re-embedded, with nothing left parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_pages_across_more_than_one_page_and_leaves_nothing_parked() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "old-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb)),
    );
    let old_meta = UpsertModelMeta {
        model: "old-model".into(),
        embed_dim: 64,
        bulk_alias: "test-embed".into(),
        short_alias: Some("test-embed".into()),
    };
    let files = super::rebuild_pages::REBUILD_PAGE_ROWS + 40;
    for i in 0..files {
        mgr.upsert_folder_file(
            "paged",
            32,
            &one_chunk(&format!("f{i}.md"), &format!("Doc {i}."), basis(64, 2)),
            Some(&old_meta),
        )
        .await
        .unwrap();
    }
    mgr.wait_for_model_probe("paged").await;

    responder.set_both_model("new-model", 64, 5);
    mgr.probe_folder_model_for_test("paged", 32).await.unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("paged", 32).await.unwrap();
    let _ = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "paged".into(),
                dim: 32,
                k: 5,
                query: Some("Doc".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    mgr.wait_for_rebuild("paged").await;

    let idx = mgr.get_or_open("paged", 32).unwrap();
    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("new-model"));
    let conn = idx.lock_conn().unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, files as i64);
    // Every row must hold the NEW model's vector (basis slot 5), not the old one.
    let new_q =
        crate::superbrain::folder_index::quantize::truncate_and_quantize_i8(&basis(64, 5), 32)
            .unwrap();
    let new_vec = crate::superbrain::folder_index::quantize::i8_slice_to_bytes(&new_q);
    let stale_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE vector_i8 IS NOT ?1",
            [&new_vec],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        stale_rows, 0,
        "rows still hold a vector that is not the new model's"
    );
    assert_eq!(
        super::rebuild_pages::parked_count(&conn).unwrap(),
        0,
        "parked vectors left behind after commit"
    );
}

/// S078 C11 review r3 HIGH: a writer that changes a chunk's text WITHOUT bumping
/// the generation (an untracked in-place UPDATE) must not let the commit go
/// through with that row still holding its old vector. The commit detects the
/// uncovered row, and the rescan re-embeds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untracked_text_change_is_caught_before_the_commit() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "old-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb)),
    );
    let old_meta = UpsertModelMeta {
        model: "old-model".into(),
        embed_dim: 64,
        bulk_alias: "test-embed".into(),
        short_alias: Some("test-embed".into()),
    };
    mgr.upsert_folder_file(
        "untracked",
        32,
        &one_chunk("a.md", "Head text.", basis(64, 2)),
        Some(&old_meta),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("untracked").await;

    let idx = mgr
        .get_or_open_without_probe_for_test("untracked", 32)
        .unwrap();
    let fired = Arc::new(AtomicBool::new(false));
    let f2 = Arc::clone(&fired);
    *idx.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |i: &FolderIndex| {
        if !f2.swap(true, Ordering::SeqCst) {
            i.lock_conn()
                .unwrap()
                .execute(
                    "UPDATE chunks SET text = 'Changed text.' WHERE file_path = 'a.md'",
                    [],
                )
                .unwrap();
        }
    }));

    responder.set_both_model("new-model", 64, 5);
    mgr.probe_folder_model_for_test("untracked", 32)
        .await
        .unwrap();
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(60));
    mgr.probe_folder_model_for_test("untracked", 32)
        .await
        .unwrap();
    let _ = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "untracked".into(),
                dim: 32,
                k: 5,
                query: Some("text".into()),
                query_vector: None,
                query_model: None,
                query_dim: None,
                filter: SearchFilter::default(),
            },
            None,
            None,
        )
        .await
        .unwrap();
    mgr.wait_for_rebuild("untracked").await;
    assert!(fired.load(Ordering::SeqCst), "hook never ran");

    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    let new_q =
        crate::superbrain::folder_index::quantize::truncate_and_quantize_i8(&basis(64, 5), 32)
            .unwrap();
    let new_vec = crate::superbrain::folder_index::quantize::i8_slice_to_bytes(&new_q);
    let conn = idx.lock_conn().unwrap();
    let not_new: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE vector_i8 IS NOT ?1",
            [&new_vec],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        not_new, 0,
        "the changed row kept its old vector under Ready"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_free_batch_during_pending_is_sticky_and_rebuilt_before_ready() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("test-embed", "test-embed", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("test-embed"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    mgr.upsert_folder_file(
        "none-batch",
        32,
        &one_chunk("a.md", "Stored A.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "test-embed".into(),
            short_alias: Some("test-embed".into()),
        }),
    )
    .await
    .unwrap();
    mgr.wait_for_model_probe("none-batch").await;

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("none-batch", 32)
        .await
        .unwrap();
    let idx = mgr
        .get_or_open_without_probe_for_test("none-batch", 32)
        .unwrap();
    idx.begin_file_indexing("b.md", "v1", 1).unwrap();
    idx.put_file_chunks_batch(
        "b.md",
        "v1",
        SourceKind::File,
        TrustLevel::Trusted,
        "alice",
        &one_chunk("b.md", "Foreign B.", basis(64, 2)).chunks,
        None,
        1,
        1,
    )
    .unwrap();
    idx.finish_file_indexing("b.md", "v1").unwrap();
    let contaminated = idx.model_status().unwrap();
    assert_eq!(contaminated.index_state, IndexModelState::Stale);
    assert!(contaminated.unproven_vectors_since_gen.is_some());

    responder.set_both_model("model-a", 64, 1);
    mgr.probe_folder_model_for_test("none-batch", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("none-batch").await;
    assert_ready_vectors_match_stored_pair(&idx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn db_claim_excludes_second_manager_and_stale_owner_resumes_parked_rows() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let mgr1 = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&short), Arc::clone(&bulk)),
    );
    let mgr2 = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&short), Arc::clone(&bulk)),
    );

    let chunks = (0..257)
        .map(|i| ChunkInput {
            anchor: Anchor {
                kind: AnchorKind::Heading,
                value: format!("H{i}"),
            },
            char_start: 0,
            char_end: 6,
            text: format!("Doc {i}."),
            vector: basis(64, 1),
        })
        .collect();
    mgr1.upsert_folder_file(
        "claimed",
        32,
        &UpsertFileRequest {
            file_path: "all.md".into(),
            file_version: "v1".into(),
            source_kind: SourceKind::File,
            trust: TrustLevel::Trusted,
            owner: "alice".into(),
            chunks,
        },
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .await
    .unwrap();
    mgr1.wait_for_model_probe("claimed").await;

    responder.set_both_model("model-b", 64, 2);
    responder.gate_after_bulk_calls.store(16, Ordering::SeqCst);
    responder.gate_bulk.store(true, Ordering::SeqCst);
    let (first, second) = tokio::join!(
        mgr1.request_folder_rebuild("claimed", 32),
        mgr2.request_folder_rebuild("claimed", 32)
    );
    let outcomes = [first.unwrap(), second.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == FolderRebuildRequestOutcome::Started)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == FolderRebuildRequestOutcome::AlreadyRunning)
            .count(),
        1,
        "second process started a concurrent rebuild"
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first owner never parked its first page");

    let idx1 = mgr1
        .get_or_open_without_probe_for_test("claimed", 32)
        .unwrap();
    let claim1 = {
        let conn = idx1.lock_conn().unwrap();
        assert_eq!(super::rebuild_pages::parked_count(&conn).unwrap(), 256);
        model_meta::read_folder_rebuild_claim(&conn)
            .unwrap()
            .expect("missing durable claim")
    };
    {
        let mut conn = idx1.lock_conn().unwrap();
        assert!(model_meta::expire_folder_rebuild_claim(
            &mut conn,
            &claim1,
            "simulated_process_crash"
        )
        .unwrap());
    }
    // Let the dead owner return from HTTP. Its fenced park must fail, leaving
    // the first page durable for the replacement.
    responder.gate_release.store(true, Ordering::SeqCst);
    mgr1.wait_for_rebuild("claimed").await;
    mgr2.wait_for_rebuild("claimed").await;
    responder.gate_entered.store(false, Ordering::SeqCst);
    responder.gate_release.store(false, Ordering::SeqCst);
    assert_eq!(
        mgr2.request_folder_rebuild("claimed", 32).await.unwrap(),
        FolderRebuildRequestOutcome::Started,
        "stale lease was not reclaimable"
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replacement did not resume at the unparked tail");
    let idx2 = mgr2
        .get_or_open_without_probe_for_test("claimed", 32)
        .unwrap();
    let claim2 = model_meta::read_folder_rebuild_claim(&idx2.lock_conn().unwrap())
        .unwrap()
        .expect("replacement owner did not claim");
    assert_eq!(claim2.token, claim1.token, "replacement discarded progress");
    assert_ne!(claim2.owner, claim1.owner, "replacement was not fenced");

    responder.gate_release.store(true, Ordering::SeqCst);
    mgr2.wait_for_rebuild("claimed").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 18);
    assert_ready_vectors_match_stored_pair(&idx2);
    assert_eq!(
        super::rebuild_pages::parked_count(&idx2.lock_conn().unwrap()).unwrap(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unnamed_naming_requires_ready_and_retries_benign_generation_cas_loss() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("alias-a", "alias-a", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let cfg = FolderIndexConfig::with_dim(32).with_embed_aliases(Some("alias-a".into()), None);
    {
        let idx = FolderIndex::open_with_config(tmp.path(), "unnamed-cas", cfg).unwrap();
        idx.upsert_file_with_model(
            &one_chunk("a.md", "Stored A.", basis(64, 1)),
            Some(&UpsertModelMeta {
                model: "model-a".into(),
                embed_dim: 64,
                bulk_alias: "alias-a".into(),
                short_alias: Some("alias-a".into()),
            }),
        )
        .unwrap();
        idx.lock_conn()
            .unwrap()
            .execute("DELETE FROM folder_meta WHERE key = 'embed_model'", [])
            .unwrap();
    }
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("alias-a"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    let idx = mgr
        .get_or_open_without_probe_for_test("unnamed-cas", 32)
        .unwrap();
    let bumped = Arc::new(AtomicBool::new(false));
    let bumped_hook = Arc::clone(&bumped);
    *idx.before_ready_publish.lock().unwrap() = Some(Box::new(move |index| {
        if !bumped_hook.swap(true, Ordering::SeqCst) {
            let conn = index.lock_conn().unwrap();
            super::sqlite_repo::bump_generation_tx(&conn).unwrap();
        }
    }));
    let calls_before_naming = responder.calls.load(Ordering::SeqCst);
    mgr.probe_folder_model_for_test("unnamed-cas", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("unnamed-cas").await;
    assert!(bumped.load(Ordering::SeqCst));
    assert_eq!(
        idx.model_status().unwrap().index_state,
        IndexModelState::Ready
    );
    assert_eq!(
        idx.model_status().unwrap().embed_model.as_deref(),
        Some("model-a")
    );
    assert_eq!(
        responder.calls.load(Ordering::SeqCst) - calls_before_naming,
        1,
        "benign naming CAS loss fell back to a re-embed"
    );

    // Recreate the unnamed fixture, then change the alias. Stale(alias-change)
    // must never be promoted in place; only RebuildCommit may name it.
    {
        let conn = idx.lock_conn().unwrap();
        conn.execute("DELETE FROM folder_meta WHERE key = 'embed_model'", [])
            .unwrap();
        conn.execute(
            "UPDATE folder_meta SET value = 'alias-a' WHERE key = 'embed_alias'",
            [],
        )
        .unwrap();
    }
    drop(idx);
    drop(mgr);
    responder.set_both_model("model-b", 64, 2);
    let emb_b = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("alias-b"));
    let mgr_b =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb_b), Arc::clone(&emb_b));
    let reopened = mgr_b
        .get_or_open_without_probe_for_test("unnamed-cas", 32)
        .unwrap();
    assert_eq!(
        reopened.model_status().unwrap().index_state,
        IndexModelState::Stale
    );
    let calls_before_alias_probe = responder.calls.load(Ordering::SeqCst);
    mgr_b
        .probe_folder_model_for_test("unnamed-cas", 32)
        .await
        .unwrap();
    mgr_b.wait_for_rebuild("unnamed-cas").await;
    assert_eq!(
        reopened.model_status().unwrap().index_state,
        IndexModelState::Ready
    );
    assert_eq!(
        reopened.model_status().unwrap().embed_model.as_deref(),
        Some("model-b")
    );
    assert_eq!(
        responder.calls.load(Ordering::SeqCst) - calls_before_alias_probe,
        2,
        "stale unnamed alias change was named without a rebuild"
    );
}

fn assert_ready_vectors_match_stored_pair(idx: &FolderIndex) {
    let mut conn = idx.lock_conn().unwrap();
    let tx = conn.transaction().unwrap();
    let status = model_meta::read_model_status(&tx).unwrap();
    if status.index_state != IndexModelState::Ready {
        tx.commit().unwrap();
        return;
    }
    let slot = match status.embed_model.as_deref() {
        Some("model-a") => 1,
        Some("model-b") => 2,
        Some("model-c") => 3,
        other => panic!("unexpected Ready model in interleaving test: {other:?}"),
    };
    let expected_quantized =
        crate::superbrain::folder_index::quantize::truncate_and_quantize_i8(&basis(64, slot), 32)
            .unwrap();
    let expected =
        crate::superbrain::folder_index::quantize::i8_slice_to_bytes(&expected_quantized);
    let foreign: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE vector_i8 IS NOT ?1",
            [&expected],
            |row| row.get(0),
        )
        .unwrap();
    tx.commit().unwrap();
    drop(conn);
    assert_eq!(
        status.unproven_vectors_since_gen, None,
        "Ready retained the unproven-vector flag"
    );
    assert_eq!(status.embed_dim, Some(64));
    assert_eq!(
        foreign, 0,
        "Ready vectors did not all match the stored model pair"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn random_probe_upsert_restart_and_rebuild_interleavings_preserve_ready_invariant() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("short-alias", "bulk-alias", "model-a", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let short = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("short-alias"));
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-alias"));
    let make_manager = || {
        Arc::new(
            FolderIndexManager::new(tmp.path())
                .with_embedders(Arc::clone(&short), Arc::clone(&bulk))
                .with_model_recovery_timings_for_test(
                    std::time::Duration::from_secs(1),
                    std::time::Duration::from_millis(20),
                    std::time::Duration::from_secs(2),
                    std::time::Duration::from_secs(10),
                    3,
                ),
        )
    };
    let models = [("model-a", 1_usize), ("model-b", 2), ("model-c", 3)];

    for seed in [0x5eed_f1a5_u64, 0xc0ffee, 0xdecafbad] {
        responder.set_both_model("model-a", 64, 1);
        responder.gate_bulk.store(false, Ordering::SeqCst);
        responder.gate_release.store(false, Ordering::SeqCst);
        let folder = format!("interleavings-{seed:x}");
        let mut manager_a = make_manager();
        let mut manager_b = make_manager();
        manager_a
            .upsert_folder_file(
                &folder,
                32,
                &one_chunk("seed.md", "Seed A text.", basis(64, 1)),
                Some(&UpsertModelMeta {
                    model: "model-a".into(),
                    embed_dim: 64,
                    bulk_alias: "bulk-alias".into(),
                    short_alias: Some("short-alias".into()),
                }),
            )
            .await
            .unwrap();
        manager_a.wait_for_model_probe(&folder).await;

        let reader_idx = manager_b
            .get_or_open_without_probe_for_test(&folder, 32)
            .unwrap();
        let reader_stop = Arc::new(AtomicBool::new(false));
        let stop_task = Arc::clone(&reader_stop);
        let reader = tokio::spawn(async move {
            while !stop_task.load(Ordering::SeqCst) {
                assert_ready_vectors_match_stored_pair(&reader_idx);
                tokio::task::yield_now().await;
            }
        });

        let mut rng = StdRng::seed_from_u64(seed);
        for step in 0..48 {
            match rng.gen_range(0..8) {
                0 => {
                    let (bulk_model, bulk_slot) = models[rng.gen_range(0..models.len())];
                    let (short_model, short_slot) = models[rng.gen_range(0..models.len())];
                    responder.set_bulk_model(bulk_model, 64, bulk_slot);
                    responder.set_short_model(short_model, 64, short_slot);
                    let (left, right) = tokio::join!(
                        manager_a.probe_folder_model_for_test(&folder, 32),
                        manager_b.probe_folder_model_for_test(&folder, 32)
                    );
                    left.unwrap();
                    right.unwrap();
                }
                1 => {
                    let (model, slot) = models[rng.gen_range(0..models.len())];
                    let manager = if rng.gen_bool(0.5) {
                        Arc::clone(&manager_a)
                    } else {
                        Arc::clone(&manager_b)
                    };
                    let result = manager
                        .upsert_folder_file(
                            &folder,
                            32,
                            &one_chunk(
                                &format!("step-{step}.md"),
                                &format!("Random step {step}."),
                                basis(64, slot),
                            ),
                            Some(&UpsertModelMeta {
                                model: model.into(),
                                embed_dim: 64,
                                bulk_alias: "bulk-alias".into(),
                                short_alias: Some("short-alias".into()),
                            }),
                        )
                        .await;
                    assert!(
                        result.is_ok()
                            || matches!(
                                result,
                                Err(crate::error::MakakooError::RetryableFolderModelMismatch(_))
                            )
                    );
                }
                2 => {
                    // Windows cannot replace an mmap-backed sidecar while the
                    // other process fixture still has it open. Unix runs the
                    // real manager restart; Windows keeps exercising the two
                    // live managers and the durable crash/reclaim branch.
                    #[cfg(not(windows))]
                    {
                        if rng.gen_bool(0.5) {
                            manager_a.flush_all().unwrap();
                            manager_a = make_manager();
                        } else {
                            manager_b.flush_all().unwrap();
                            manager_b = make_manager();
                        }
                    }
                }
                3 => {
                    let (left, right) = tokio::join!(
                        manager_a.request_folder_rebuild(&folder, 32),
                        manager_b.request_folder_rebuild(&folder, 32)
                    );
                    for outcome in [left.unwrap(), right.unwrap()] {
                        assert!(matches!(
                            outcome,
                            FolderRebuildRequestOutcome::Started
                                | FolderRebuildRequestOutcome::AlreadyRunning
                                | FolderRebuildRequestOutcome::AlreadyCurrent
                        ));
                    }
                }
                4 => {
                    manager_a
                        .advance_model_recovery_time_for_test(std::time::Duration::from_secs(2));
                    manager_b
                        .advance_model_recovery_time_for_test(std::time::Duration::from_secs(2));
                    let (left, right) = tokio::join!(
                        manager_a.probe_folder_model_for_test(&folder, 32),
                        manager_b.probe_folder_model_for_test(&folder, 32)
                    );
                    left.unwrap();
                    right.unwrap();
                }
                5 => {
                    let idx = manager_a
                        .get_or_open_without_probe_for_test(&folder, 32)
                        .unwrap();
                    let _ = idx.remove_file(&format!("step-{}.md", rng.gen_range(0..=step)));
                }
                6 => {
                    let idx = manager_a
                        .get_or_open_without_probe_for_test(&folder, 32)
                        .unwrap();
                    let file = format!("none-{step}.md");
                    idx.begin_file_indexing(&file, "v1", 1).unwrap();
                    idx.put_file_chunks_batch(
                        &file,
                        "v1",
                        SourceKind::File,
                        TrustLevel::Trusted,
                        "alice",
                        &one_chunk(&file, "Unnamed batch.", basis(64, 3)).chunks,
                        None,
                        1,
                        1,
                    )
                    .unwrap();
                    idx.finish_file_indexing(&file, "v1").unwrap();
                }
                _ => {
                    // Crash between claim and first page. The DB must remain
                    // non-Ready, and a replacement process must be able to reclaim.
                    let idx = manager_a
                        .get_or_open_without_probe_for_test(&folder, 32)
                        .unwrap();
                    let model = responder.bulk_model.lock().unwrap().clone();
                    let dim = responder.bulk_dim.load(Ordering::SeqCst);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;
                    let claim = {
                        let mut conn = idx.lock_conn().unwrap();
                        model_meta::try_claim_folder_rebuild(&mut conn, &model, dim, now, None)
                            .unwrap()
                    };
                    if let Some(claim) = claim {
                        let mut conn = idx.lock_conn().unwrap();
                        model_meta::expire_folder_rebuild_claim(
                            &mut conn,
                            &claim,
                            "simulated_process_crash",
                        )
                        .unwrap();
                    }
                    #[cfg(not(windows))]
                    {
                        manager_a = make_manager();
                    }
                }
            }

            manager_a.wait_for_model_probe(&folder).await;
            manager_b.wait_for_model_probe(&folder).await;
            manager_a.wait_for_rebuild(&folder).await;
            manager_b.wait_for_rebuild(&folder).await;
            let idx = manager_a
                .get_or_open_without_probe_for_test(&folder, 32)
                .unwrap();
            assert_ready_vectors_match_stored_pair(&idx);
        }

        reader_stop.store(true, Ordering::SeqCst);
        reader.await.unwrap();
    }
}
