//! End-to-end guard tests proving every Brain and folder-index embedding path
//! (rebuild batches, top-up, folder index rebuild, Brain query, folder query)
//! enforces the 16-input batch cap and 2,000-UTF-8-byte character-boundary
//! truncation against a strict endpoint that returns HTTP 400 on any violation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::{EmbeddingClient, DEFAULT_EMBED_MAX_BATCH, DEFAULT_EMBED_MAX_INPUT_BYTES};
use crate::superbrain::folder_index::{
    Anchor, AnchorKind, ChunkInput, FolderIndexManager, FolderSearchRequest, IndexModelState,
    SearchFilter, SearchMode, SourceKind, TrustLevel, UpsertFileRequest, UpsertModelMeta,
};
use crate::superbrain::store::{unpack_vector, SuperbrainStore};
use crate::superbrain::vector_model::{
    rebuild_brain_vectors, search_brain_vectors_checked, BrainRebuildCoordinator,
};

#[derive(Clone)]
struct StrictEndpoint {
    model: Arc<Mutex<String>>,
    dim: usize,
    max_inputs: usize,
    max_bytes: usize,
    batch_sizes: Arc<Mutex<Vec<usize>>>,
    received_inputs: Arc<Mutex<Vec<String>>>,
}

impl StrictEndpoint {
    fn new(model: &str, dim: usize) -> Self {
        Self {
            model: Arc::new(Mutex::new(model.to_string())),
            dim,
            max_inputs: DEFAULT_EMBED_MAX_BATCH,
            max_bytes: DEFAULT_EMBED_MAX_INPUT_BYTES,
            batch_sizes: Arc::new(Mutex::new(Vec::new())),
            received_inputs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Respond for StrictEndpoint {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let arr = body
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if arr.len() > self.max_inputs {
            return ResponseTemplate::new(400).set_body_json(json!({
                "error": format!("batch size {} exceeds cap {}", arr.len(), self.max_inputs)
            }));
        }
        let is_probe = arr.len() == 1 && arr[0].as_str() == Some("probe");
        let mut data = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let text = item.as_str().unwrap_or("");
            if text.len() > self.max_bytes {
                return ResponseTemplate::new(400).set_body_json(json!({
                    "error": format!("input byte length {} exceeds cap {}", text.len(), self.max_bytes)
                }));
            }
            if !is_probe {
                self.received_inputs.lock().unwrap().push(text.to_string());
            }
            let mut vec = vec![0.0_f32; self.dim];
            // Encode leading `TAG:` integer when present so tests verify exact
            // doc-to-vector ordering across split batches while keeping cosine > 0.3.
            let tag = text.split(':').next().and_then(|s| s.parse::<usize>().ok());
            if let Some(slot) = tag {
                vec[0] = (slot as f32) + 1.0;
                vec[1] = 1.0;
                vec[(slot % (self.dim - 2)) + 2] = 0.5;
            } else {
                vec[0] = 1.0;
                vec[1] = 1.0;
            }
            data.push(json!({ "index": i, "embedding": vec }));
        }
        if !is_probe {
            self.batch_sizes.lock().unwrap().push(arr.len());
        }
        let model = self.model.lock().unwrap().clone();
        ResponseTemplate::new(200).set_body_json(json!({ "model": model, "data": data }))
    }
}

fn long_polish_2000_chars(tag: usize) -> String {
    let prefix = format!("{tag}:");
    let body: String = "Zażółć gęślą jaźń — ą ć ę ł ń ó ś ź ż. "
        .chars()
        .cycle()
        .take(2_000 - prefix.chars().count())
        .collect();
    let full = format!("{prefix}{body}");
    assert_eq!(full.chars().count(), 2_000);
    assert!(full.len() > DEFAULT_EMBED_MAX_INPUT_BYTES);
    full
}

fn long_german_2000_chars(tag: usize) -> String {
    let prefix = format!("{tag}:");
    let body: String = "Prüfung über größere Änderungen für Maßstäbe — Ärger, Öl, Übung. "
        .chars()
        .cycle()
        .take(2_000 - prefix.chars().count())
        .collect();
    let full = format!("{prefix}{body}");
    assert_eq!(full.chars().count(), 2_000);
    assert!(full.len() > DEFAULT_EMBED_MAX_INPUT_BYTES);
    full
}

fn stored_first_components(store: &SuperbrainStore) -> HashMap<String, f32> {
    let conn = store.conn_arc();
    let conn = conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT d.path, v.embedding
             FROM brain_vectors v
             JOIN brain_docs d ON v.doc_id = d.id",
        )
        .unwrap();
    stmt.query_map([], |r| {
        let path: String = r.get(0)?;
        let blob: Vec<u8> = r.get(1)?;
        let vec = unpack_vector(&blob);
        Ok((path, vec[0]))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brain_rebuild_caps_batch_at_16_and_truncates_polish_german_by_utf8_bytes() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    for i in 0..35 {
        let content = if i % 2 == 0 {
            long_polish_2000_chars(i)
        } else {
            long_german_2000_chars(i)
        };
        store
            .write_document(&format!("pages/doc-{i:02}.md"), &content, "page", json!([]))
            .unwrap();
    }

    let endpoint = StrictEndpoint::new("strict-qwen3", 8);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(endpoint.clone())
        .mount(&server)
        .await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed-bulk"));

    let written = rebuild_brain_vectors(Arc::clone(&store), bulk, None, None)
        .await
        .unwrap();
    assert_eq!(written, 35);
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );
    assert_eq!(
        endpoint.batch_sizes.lock().unwrap().as_slice(),
        &[16, 16, 3],
        "rebuild must send at most 16 inputs per request"
    );
    for sent in endpoint.received_inputs.lock().unwrap().iter() {
        assert!(sent.len() <= DEFAULT_EMBED_MAX_INPUT_BYTES);
    }

    let got = stored_first_components(&store);
    assert_eq!(got.len(), 35);
    for i in 0..35 {
        let key = format!("pages/doc-{i:02}.md");
        assert_eq!(
            got.get(&key).copied(),
            Some((i as f32) + 1.0),
            "vector for {key} out of order"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brain_top_up_splits_over_16_docs_and_truncates_utf8_bytes() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/seed.md", "0:seed doc", "page", json!([]))
        .unwrap();

    let endpoint = StrictEndpoint::new("strict-qwen3", 8);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(endpoint.clone())
        .mount(&server)
        .await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed-bulk"));

    rebuild_brain_vectors(Arc::clone(&store), Arc::clone(&bulk), None, None)
        .await
        .unwrap();
    endpoint.batch_sizes.lock().unwrap().clear();
    endpoint.received_inputs.lock().unwrap().clear();

    // Add 25 missing docs with 2,000-char Polish/German bodies (> 2,000 bytes).
    for i in 1..=25 {
        let content = if i % 2 == 0 {
            long_polish_2000_chars(i)
        } else {
            long_german_2000_chars(i)
        };
        store
            .write_document(
                &format!("pages/topup-{i:02}.md"),
                &content,
                "page",
                json!([]),
            )
            .unwrap();
    }

    let written = store.sync_vectors_with_embedder(&bulk, 32).await.unwrap();
    assert_eq!(written, 25);
    assert_eq!(
        endpoint.batch_sizes.lock().unwrap().as_slice(),
        &[16, 9],
        "top-up must split requests at 16 inputs"
    );
    for sent in endpoint.received_inputs.lock().unwrap().iter() {
        assert!(sent.len() <= DEFAULT_EMBED_MAX_INPUT_BYTES);
    }

    let got = stored_first_components(&store);
    for i in 1..=25 {
        let key = format!("pages/topup-{i:02}.md");
        assert_eq!(
            got.get(&key).copied(),
            Some((i as f32) + 1.0),
            "top-up vector for {key} out of order"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brain_query_truncates_polish_german_query_by_utf8_bytes() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/pl.md", "7:Zażółć gęślą jaźń", "page", json!([]))
        .unwrap();

    let endpoint = StrictEndpoint::new("strict-qwen3", 8);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(endpoint.clone())
        .mount(&server)
        .await;
    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed-bulk"));
    let coord = Arc::new(BrainRebuildCoordinator::new());

    rebuild_brain_vectors(
        Arc::clone(&store),
        Arc::clone(&bulk_emb),
        Some(Arc::clone(&short_emb)),
        None,
    )
    .await
    .unwrap();
    endpoint.received_inputs.lock().unwrap().clear();

    let query = long_polish_2000_chars(7);
    let hits = search_brain_vectors_checked(&store, &short_emb, Some(&bulk_emb), &coord, &query, 5)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].doc_id, "pages/pl.md");
    let sent = &endpoint.received_inputs.lock().unwrap()[0];
    assert!(sent.len() <= DEFAULT_EMBED_MAX_INPUT_BYTES);
    assert!(query.starts_with(sent.as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_index_rebuild_and_query_enforce_16_batch_and_2000_utf8_byte_cap() {
    let tmp = TempDir::new().unwrap();
    let endpoint = StrictEndpoint::new("model-v1", 32);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(endpoint.clone())
        .mount(&server)
        .await;

    let short_emb = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed"));
    let bulk_emb =
        Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed-bulk"));
    let mgr = Arc::new(
        FolderIndexManager::new(tmp.path())
            .with_embedders(Arc::clone(&short_emb), Arc::clone(&bulk_emb)),
    );

    let old_meta = UpsertModelMeta {
        model: "model-v1".into(),
        embed_dim: 32,
        bulk_alias: "ail-embed-bulk".into(),
        short_alias: Some("ail-embed".into()),
    };
    let mut expected_texts = Vec::new();
    for i in 0..25 {
        let text = if i % 2 == 0 {
            long_polish_2000_chars(i)
        } else {
            long_german_2000_chars(i)
        };
        let mut init_vec = vec![0.0_f32; 32];
        init_vec[0] = 1.0;
        mgr.upsert_folder_file(
            "eu-docs",
            32,
            &UpsertFileRequest {
                file_path: format!("file-{i:02}.md"),
                file_version: "v1".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "alice".into(),
                chunks: vec![ChunkInput {
                    anchor: Anchor {
                        kind: AnchorKind::Page,
                        value: (i + 1).to_string(),
                    },
                    char_start: 0,
                    char_end: text.chars().count(),
                    text: text.clone(),
                    vector: init_vec,
                }],
            },
            Some(&old_meta),
        )
        .unwrap();
        expected_texts.push((format!("file-{i:02}.md"), text));
    }

    // Switch model so search triggers a paged folder rebuild over all 25 chunks.
    *endpoint.model.lock().unwrap() = "model-v2".to_string();
    let long_query = long_german_2000_chars(1);
    let first = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "eu-docs".into(),
                dim: 32,
                k: 5,
                query: Some(long_query.clone()),
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
    assert_eq!(first.mode, SearchMode::Fts);
    assert!(first.stale);

    mgr.wait_for_rebuild("eu-docs").await;

    let idx = mgr.get_or_open("eu-docs", 32).unwrap();
    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-v2"));

    // First request was the short query (len 1), followed by rebuild batches [16, 9].
    let sizes = endpoint.batch_sizes.lock().unwrap().clone();
    assert_eq!(
        sizes.as_slice(),
        &[1, 16, 9],
        "folder rebuild must split 25 chunks into batches of 16 and 9"
    );
    for sent in endpoint.received_inputs.lock().unwrap().iter() {
        assert!(sent.len() <= DEFAULT_EMBED_MAX_INPUT_BYTES);
    }

    // Original untruncated UTF-8 text must stay intact in SQLite after swap-in.
    {
        let conn = idx.lock_conn().unwrap();
        for (file_path, orig_text) in &expected_texts {
            let stored: String = conn
                .query_row(
                    "SELECT text FROM chunks WHERE file_path = ?1",
                    [file_path],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(&stored, orig_text);
        }
    }

    // Now query the Ready folder index with a >2,000-byte German query: must
    // succeed in Vector mode (an un-truncated query gets HTTP 400 -> FTS fallback).
    let after = mgr
        .search_folder(
            FolderSearchRequest {
                folder_id: "eu-docs".into(),
                dim: 32,
                k: 5,
                query: Some(long_query),
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
    assert_eq!(
        after.mode,
        SearchMode::Vector,
        "folder query over 2,000 UTF-8 bytes must truncate and succeed in vector mode"
    );
    assert!(!after.stale);
    assert!(!after.passages.is_empty());
}
