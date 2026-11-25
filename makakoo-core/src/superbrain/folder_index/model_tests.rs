//! Guard-focused unit tests for per-folder embedding model tracking,
//! stale detection, background rebuilds, FTS fallback, and bulk/short split.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;

use super::model_meta::SHORT_BULK_MISMATCH_REASON;
use super::*;

#[derive(Clone)]
struct DynamicEmbedResponder {
    short_alias: String,
    bulk_alias: String,
    short_model: Arc<std::sync::Mutex<String>>,
    bulk_model: Arc<std::sync::Mutex<String>>,
    short_dim: Arc<AtomicUsize>,
    bulk_dim: Arc<AtomicUsize>,
    short_vec_slot: Arc<AtomicUsize>,
    bulk_vec_slot: Arc<AtomicUsize>,
    gate_bulk: Arc<AtomicBool>,
    gate_entered: Arc<AtomicBool>,
    gate_release: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    bulk_probe_calls: Arc<AtomicUsize>,
    bulk_calls: Arc<AtomicUsize>,
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
            short_dim: Arc::new(AtomicUsize::new(dim)),
            bulk_dim: Arc::new(AtomicUsize::new(dim)),
            short_vec_slot: Arc::new(AtomicUsize::new(0)),
            bulk_vec_slot: Arc::new(AtomicUsize::new(0)),
            gate_bulk: Arc::new(AtomicBool::new(false)),
            gate_entered: Arc::new(AtomicBool::new(false)),
            gate_release: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(AtomicUsize::new(0)),
            bulk_probe_calls: Arc::new(AtomicUsize::new(0)),
            bulk_calls: Arc::new(AtomicUsize::new(0)),
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

        if !is_probe && (is_bulk || inputs > 1) {
            self.bulk_calls.fetch_add(1, Ordering::SeqCst);
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

        let (model_name, dim, slot) = if is_bulk {
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
    let responder = DynamicEmbedResponder::new("ail-embed", "ail-embed-bulk", "model-v1", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed-bulk"));
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
                bulk_alias: "ail-embed-bulk".into(),
                short_alias: Some("ail-embed".into()),
            }),
        )
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
    let responder = DynamicEmbedResponder::new("ail-embed", "ail-embed", "index-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let mgr =
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb));
    mgr.upsert_folder_file(
        "query-mismatch",
        32,
        &one_chunk("a.md", "Mismatch fallback text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "index-model".into(),
            embed_dim: 64,
            bulk_alias: "ail-embed".into(),
            short_alias: Some("ail-embed".into()),
        }),
    )
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
async fn permanent_rebuild_failure_is_bounded_over_a_simulated_day() {
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

    for _ in 2..(24 * 6) {
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
    assert!(status
        .stale_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("rebuild_suppressed:")
            && reason.contains("after 3 same-class failures")));

    responder.set_both_model("newer", 64, 3);
    mgr.advance_model_recovery_time_for_test(std::time::Duration::from_secs(10 * 60));
    let _ = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "failed".into(),
                dim: 32,
                k: 1,
                query: Some("Failure".into()),
                query_vector: Some(basis(64, 3)),
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
    mgr.probe_folder_model_for_test("failed", 32).await.unwrap();
    mgr.wait_for_rebuild("failed").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 4);

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
    assert_eq!(recovered.embed_model.as_deref(), Some("newer"));
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 5);
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
            .expect_err("foreign model metadata must be rejected");
        assert!(matches!(err, crate::error::MakakooError::InvalidInput(_)));

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
    .expect("the latest observed bulk pair must be accepted");
    mgr.wait_for_model_probe("observed-upsert").await;
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
        .expect_err("a pair that is neither stored nor observed must be rejected");
    assert!(matches!(err, crate::error::MakakooError::InvalidInput(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flapping_bulk_pair_needs_two_consecutive_observations() {
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
        "flapping",
        32,
        &one_chunk("a.md", "Flapping text.", basis(64, 1)),
        Some(&UpsertModelMeta {
            model: "model-a".into(),
            embed_dim: 64,
            bulk_alias: "bulk-alias".into(),
            short_alias: Some("short-alias".into()),
        }),
    )
    .unwrap();
    mgr.wait_for_model_probe("flapping").await;

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("flapping", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("flapping").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);

    responder.set_both_model("model-a", 64, 1);
    mgr.probe_folder_model_for_test("flapping", 32)
        .await
        .unwrap();
    assert_eq!(
        mgr.get_or_open_without_probe_for_test("flapping", 32)
            .unwrap()
            .model_status()
            .unwrap()
            .index_state,
        IndexModelState::Ready
    );
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);

    responder.set_both_model("model-b", 64, 2);
    mgr.probe_folder_model_for_test("flapping", 32)
        .await
        .unwrap();
    mgr.probe_folder_model_for_test("flapping", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("flapping").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        mgr.get_or_open_without_probe_for_test("flapping", 32)
            .unwrap()
            .model_status()
            .unwrap()
            .embed_model
            .as_deref(),
        Some("model-b")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn varying_failure_text_persists_backoff_and_halt_while_ingest_continues() {
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
    .unwrap();
    mgr.wait_for_model_probe("persisted-failure").await;
    responder.set_both_model("model-b", 64, 2);
    responder.fail_bulk_embeddings.store(true, Ordering::SeqCst);
    responder.vary_failure_body.store(true, Ordering::SeqCst);
    mgr.probe_folder_model_for_test("persisted-failure", 32)
        .await
        .unwrap();
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
    mgr.probe_folder_model_for_test("persisted-failure", 32)
        .await
        .unwrap();
    mgr.wait_for_rebuild("persisted-failure").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 2);
    tokio::time::sleep(std::time::Duration::from_millis(70)).await;
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
    .expect("a halted rebuild must not halt ingest from the observed bulk model");
    mgr.wait_for_model_probe("persisted-failure").await;
    mgr.wait_for_rebuild("persisted-failure").await;
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 3);
    let status = mgr
        .get_or_open_without_probe_for_test("persisted-failure", 32)
        .unwrap()
        .model_status()
        .unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert!(status
        .stale_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("rebuild_suppressed:")));
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
            bulk_alias: "ail-embed".into(),
            short_alias: Some("ail-embed".into()),
        }),
    )
    .unwrap();
    seed.upsert_folder_file(
        "interrupted",
        32,
        &one_chunk("b.md", "Second interrupted row.", basis(64, 2)),
        Some(&UpsertModelMeta {
            model: "stable-model".into(),
            embed_dim: 64,
            bulk_alias: "ail-embed".into(),
            short_alias: Some("ail-embed".into()),
        }),
    )
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
            .with_embed_aliases(Some("ail-embed".into()), Some("ail-embed".into())),
    )
    .unwrap();
    let reopened_status = reopened.model_status().unwrap();
    assert_eq!(reopened_status.index_state, IndexModelState::Stale);
    assert_eq!(
        reopened_status.stale_reason.as_deref(),
        Some("interrupted_rebuild")
    );
    drop(reopened);

    let responder = DynamicEmbedResponder::new("ail-embed", "ail-embed", "stable-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
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
    let responder = DynamicEmbedResponder::new("ail-embed", "ail-embed", "qwen3-0.6b", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
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
            bulk_alias: "ail-embed".into(),
            short_alias: Some("ail-embed".into()),
        }),
    )
    .unwrap();
    mgr.wait_for_model_probe("dim-test").await;
    mgr.upsert_folder_file(
        "dim-test",
        32,
        &one_chunk("b.md", "Second dimensionality row.", basis(64, 3)),
        Some(&UpsertModelMeta {
            model: "qwen3-0.6b".into(),
            embed_dim: 64,
            bulk_alias: "ail-embed".into(),
            short_alias: Some("ail-embed".into()),
        }),
    )
    .unwrap();
    // Same model name, changed pre-truncation dim (64 -> 48).
    responder.set_both_model("qwen3-0.6b", 48, 4);
    responder.gate_bulk.store(true, Ordering::SeqCst);
    mgr.probe_folder_model_for_test("dim-test", 32)
        .await
        .unwrap();
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
        let conn = idx.lock_conn().unwrap();
        model_meta::write_model_state(&conn, IndexModelState::Ready, None).unwrap();
    }
    assert!(idx
        .mark_stale_unless_rebuilding("model_changed_on_probe")
        .unwrap());
    let st = idx.model_status().unwrap();
    assert_eq!(st.index_state, IndexModelState::Stale);
    assert_eq!(st.stale_reason.as_deref(), Some("model_changed_on_probe"));
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
    let responder = DynamicEmbedResponder::new("ail-embed", "ail-embed", "old-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb)),
    );
    let old_meta = UpsertModelMeta {
        model: "old-model".into(),
        embed_dim: 64,
        bulk_alias: "ail-embed".into(),
        short_alias: Some("ail-embed".into()),
    };
    mgr.upsert_folder_file(
        "tail",
        32,
        &one_chunk("a.md", "Alpha text.", basis(64, 2)),
        Some(&old_meta),
    )
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

/// S078 C11 (review r1 HIGH): the rebuild pages through the folder, parks each
/// page's vectors on disk and swaps them in at commit. A folder larger than one
/// page must come out fully re-embedded, with nothing left parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_pages_across_more_than_one_page_and_leaves_nothing_parked() {
    let tmp = TempDir::new().unwrap();
    let responder = DynamicEmbedResponder::new("ail-embed", "ail-embed", "old-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb)),
    );
    let old_meta = UpsertModelMeta {
        model: "old-model".into(),
        embed_dim: 64,
        bulk_alias: "ail-embed".into(),
        short_alias: Some("ail-embed".into()),
    };
    let files = super::rebuild_pages::REBUILD_PAGE_ROWS + 40;
    for i in 0..files {
        mgr.upsert_folder_file(
            "paged",
            32,
            &one_chunk(&format!("f{i}.md"), &format!("Doc {i}."), basis(64, 2)),
            Some(&old_meta),
        )
        .unwrap();
    }
    mgr.wait_for_model_probe("paged").await;

    responder.set_both_model("new-model", 64, 5);
    mgr.probe_folder_model_for_test("paged", 32).await.unwrap();
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
    let responder = DynamicEmbedResponder::new("ail-embed", "ail-embed", "old-model", 64);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path()).with_embedders(Arc::clone(&emb), Arc::clone(&emb)),
    );
    let old_meta = UpsertModelMeta {
        model: "old-model".into(),
        embed_dim: 64,
        bulk_alias: "ail-embed".into(),
        short_alias: Some("ail-embed".into()),
    };
    mgr.upsert_folder_file(
        "untracked",
        32,
        &one_chunk("a.md", "Head text.", basis(64, 2)),
        Some(&old_meta),
    )
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
