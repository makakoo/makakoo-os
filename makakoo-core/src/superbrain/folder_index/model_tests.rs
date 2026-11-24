//! Guard-focused unit tests for per-folder embedding model tracking,
//! stale detection, background rebuilds, FTS fallback, and bulk/short split.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;

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
    bulk_calls: Arc<AtomicUsize>,
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
            bulk_calls: Arc::new(AtomicUsize::new(0)),
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
}

impl Respond for DynamicEmbedResponder {
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
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path())
            .with_embedders(Arc::clone(&short_emb), Arc::clone(&bulk_emb)),
    );

    let vec_v1 = basis(64, 1);
    mgr.upsert_folder_file(
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

    // Baseline vector search succeeds under model-v1.
    let res1 = mgr
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

    // First search detects mismatch, triggers rebuild, and serves FTS only.
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

    // Wait until background rebuild hits the mock server so it's actively rebuilding.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !responder.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bulk embed call never reached the gate");

    // Direct vector search MUST return 0 vector results while rebuilding.
    let idx = mgr.get_or_open("docs", 32).unwrap();
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

    // Search invokes short-a -> gets model-a != stored model-b -> marks stale,
    // probes bulk-b -> sees short != bulk -> stays stale FTS-only without looping.
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
        Some("short_bulk_model_mismatch")
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
    assert_eq!(responder.bulk_calls.load(Ordering::SeqCst), 0);
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

    // Same model name, changed pre-truncation dim (64 -> 48).
    responder.set_both_model("qwen3-0.6b", 48, 4);
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
    reg.try_acquire("f").expect("free");
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

    // A writer with an old-model vector lands after every tail read, once,
    // right before the first commit attempt.
    let idx = mgr.get_or_open("tail", 32).unwrap();
    let fired = Arc::new(AtomicBool::new(false));
    let (f2, meta2) = (Arc::clone(&fired), old_meta.clone());
    *idx.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |i: &FolderIndex| {
        if !f2.swap(true, Ordering::SeqCst) {
            i.upsert_file_with_model(&one_chunk("b.md", "Beta text.", basis(64, 2)), Some(&meta2))
                .unwrap();
        }
    }));

    responder.set_both_model("new-model", 64, 5);
    let res = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "tail".into(),
                dim: 32,
                k: 5,
                query: Some("Alpha".into()),
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
    assert!(res.stale);
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

    responder.set_both_model("new-model", 64, 5);
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

/// S078 C11 review r2 HIGH: chunk ids are reused once the top rows are deleted.
/// A writer that replaces the last file while the rebuild is in flight must not
/// leave its new text holding the old text's vector.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reused_chunk_id_never_takes_the_old_chunks_vector() {
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
        "reuse",
        32,
        &one_chunk("a.md", "Head text.", basis(64, 2)),
        Some(&old_meta),
    )
    .unwrap();
    mgr.upsert_folder_file(
        "reuse",
        32,
        &one_chunk("last.md", "Old tail.", basis(64, 2)),
        Some(&old_meta),
    )
    .unwrap();

    let idx = mgr.get_or_open("reuse", 32).unwrap();
    let fired = Arc::new(AtomicBool::new(false));
    let (f2, meta2) = (Arc::clone(&fired), old_meta.clone());
    *idx.before_rebuild_commit.lock().unwrap() = Some(Box::new(move |i: &FolderIndex| {
        if !f2.swap(true, Ordering::SeqCst) {
            // Delete the top file's chunks, then add a new file: SQLite reuses the
            // freed top ids for the new chunk (an upsert alone inserts first, so
            // it would not reuse them).
            {
                let conn = i.lock_conn().unwrap();
                conn.execute("DELETE FROM chunks WHERE file_path = 'last.md'", [])
                    .unwrap();
            }
            i.upsert_file_with_model(
                &one_chunk("new.md", "New tail.", basis(64, 2)),
                Some(&meta2),
            )
            .unwrap();
        }
    }));

    responder.set_both_model("new-model", 64, 5);
    let _ = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "reuse".into(),
                dim: 32,
                k: 5,
                query: Some("tail".into()),
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
    mgr.wait_for_rebuild("reuse").await;
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
    assert_eq!(not_new, 0, "a row kept an old-model vector under Ready");
}
