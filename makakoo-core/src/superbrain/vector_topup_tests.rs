//! Guard tests for top-up single-document fallback when a batched chunk fails
//! (review r3 MED): one bad document must never block its 15 neighbours in the
//! same tick, and the bad document alone is skipped and logged once per tick.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tempfile::TempDir;
use tracing::instrument::WithSubscriber;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;
use crate::superbrain::folder_index::IndexModelState;
use crate::superbrain::store::{unpack_vector, SuperbrainStore};
use crate::superbrain::vector_model::{rebuild_brain_vectors, EMBED_MODEL_CHANGED_REASON};

#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct RefusingDocEndpoint {
    model: Arc<Mutex<String>>,
    dim: usize,
    refuse_marker: String,
    flip_model_on_marker: Option<(String, String)>,
    batch_sizes: Arc<Mutex<Vec<usize>>>,
}

impl Respond for RefusingDocEndpoint {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let arr = body
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let is_probe = arr.len() == 1 && arr[0].as_str() == Some("probe");
        if !is_probe {
            self.batch_sizes.lock().unwrap().push(arr.len());
        }
        if arr
            .iter()
            .filter_map(Value::as_str)
            .any(|s| s.contains(&self.refuse_marker))
        {
            return ResponseTemplate::new(400).set_body_json(json!({
                "error": "refused document payload"
            }));
        }
        let mut model = self.model.lock().unwrap().clone();
        if let Some((ref trigger, ref next_model)) = self.flip_model_on_marker {
            if arr
                .iter()
                .filter_map(Value::as_str)
                .any(|s| s.contains(trigger))
            {
                model = next_model.clone();
            }
        }
        let mut data = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let text = item.as_str().unwrap_or("");
            let tag = text
                .split(':')
                .next()
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0);
            let mut vec = vec![0.0_f32; self.dim];
            vec[0] = tag + 1.0;
            vec[1] = 1.0;
            data.push(json!({ "index": i, "embedding": vec }));
        }
        ResponseTemplate::new(200).set_body_json(json!({ "model": model, "data": data }))
    }
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
        Ok((path, unpack_vector(&blob)[0]))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// Review r3 MED: when a 16-doc top-up chunk fails because of one bad document,
/// top-up retries that chunk one document at a time so the other 15 get vectors
/// in the same tick, while the bad document alone is skipped and logged once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_up_retries_failed_16_doc_chunk_one_by_one_and_skips_only_bad_doc() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/seed.md", "0:seed doc", "page", json!([]))
        .unwrap();

    let endpoint = RefusingDocEndpoint {
        model: Arc::new(Mutex::new("model-1".to_string())),
        dim: 8,
        refuse_marker: "POISON_DOC".to_string(),
        flip_model_on_marker: None,
        batch_sizes: Arc::new(Mutex::new(Vec::new())),
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(endpoint.clone())
        .mount(&server)
        .await;
    let bulk = Arc::new(
        EmbeddingClient::with_base_url(server.uri())
            .with_model("ail-embed-bulk")
            .with_max_batch(16)
            .with_max_input_bytes(2_000),
    );

    rebuild_brain_vectors(Arc::clone(&store), Arc::clone(&bulk), None, None)
        .await
        .unwrap();
    endpoint.batch_sizes.lock().unwrap().clear();

    // Add one 16-doc chunk where doc 7 is refused by the server.
    for i in 1..=16usize {
        let content = if i == 7 {
            format!("{i}:POISON_DOC rejected payload")
        } else {
            format!("{i}:healthy doc {i}")
        };
        store
            .write_document(&format!("pages/doc-{i:02}.md"), &content, "page", json!([]))
            .unwrap();
    }

    let log_buf = LogBuf::default();
    let log_sink = log_buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer(move || log_sink.clone())
        .finish();
    let written = store
        .sync_vectors_with_embedder(&bulk, 16)
        .with_subscriber(subscriber)
        .await
        .unwrap();

    assert_eq!(
        written, 15,
        "all 15 healthy neighbours must be embedded in the same tick"
    );
    let mut expected_sizes = vec![16];
    expected_sizes.extend(std::iter::repeat_n(1, 16));
    assert_eq!(
        endpoint.batch_sizes.lock().unwrap().as_slice(),
        expected_sizes.as_slice()
    );

    let stored = stored_first_components(&store);
    assert!(
        !stored.contains_key("pages/doc-07.md"),
        "bad doc must be skipped"
    );
    for i in 1..=16usize {
        if i == 7 {
            continue;
        }
        let id = format!("pages/doc-{i:02}.md");
        assert_eq!(
            stored.get(&id).copied(),
            Some((i as f32) + 1.0),
            "missing or misaligned vector for {id}"
        );
    }
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );

    let logs = String::from_utf8(log_buf.0.lock().unwrap().clone()).unwrap();
    assert_eq!(
        logs.matches("pages/doc-07.md").count(),
        1,
        "bad doc must be logged exactly once per tick, got: {logs:?}"
    );
    assert!(logs.contains("WARN"), "expected WARN log, got: {logs:?}");
    assert!(
        !logs.contains("pages/doc-01.md"),
        "healthy docs must not be logged as skipped"
    );

    // Next tick sees only the 1 remaining bad doc: sends 1 request (no duplicate
    // retry) and logs once.
    endpoint.batch_sizes.lock().unwrap().clear();
    let log_buf2 = LogBuf::default();
    let log_sink2 = log_buf2.clone();
    let subscriber2 = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer(move || log_sink2.clone())
        .finish();
    let written2 = store
        .sync_vectors_with_embedder(&bulk, 16)
        .with_subscriber(subscriber2)
        .await
        .unwrap();
    assert_eq!(written2, 0);
    assert_eq!(endpoint.batch_sizes.lock().unwrap().as_slice(), &[1]);
    let logs2 = String::from_utf8(log_buf2.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs2.matches("pages/doc-07.md").count(), 1);
}

/// If a model change occurs during the single-doc fallback loop, top-up marks
/// the index stale and stops immediately without embedding remaining documents.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_up_single_doc_fallback_stops_immediately_on_model_change() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/seed.md", "0:seed doc", "page", json!([]))
        .unwrap();

    let endpoint = RefusingDocEndpoint {
        model: Arc::new(Mutex::new("model-1".to_string())),
        dim: 8,
        refuse_marker: "POISON_DOC".to_string(),
        flip_model_on_marker: Some(("FLIP_MODEL".to_string(), "model-2".to_string())),
        batch_sizes: Arc::new(Mutex::new(Vec::new())),
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(endpoint.clone())
        .mount(&server)
        .await;
    let bulk = Arc::new(
        EmbeddingClient::with_base_url(server.uri())
            .with_model("ail-embed-bulk")
            .with_max_batch(16)
            .with_max_input_bytes(2_000),
    );

    rebuild_brain_vectors(Arc::clone(&store), Arc::clone(&bulk), None, None)
        .await
        .unwrap();
    endpoint.batch_sizes.lock().unwrap().clear();

    // 4-doc chunk: doc 1 ok, doc 2 poison, doc 3 flips model, doc 4 must never run.
    store
        .write_document("pages/a.md", "1:good", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/b.md", "2:POISON_DOC", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/c.md", "3:FLIP_MODEL", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/d.md", "4:should not embed", "page", json!([]))
        .unwrap();

    let written = store.sync_vectors_with_embedder(&bulk, 16).await.unwrap();
    assert_eq!(written, 1);
    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(
        status.stale_reason.as_deref(),
        Some(EMBED_MODEL_CHANGED_REASON)
    );
    // Batch of 4 failed, then doc 1 (ok), doc 2 (poison), doc 3 (model change -> stop).
    assert_eq!(
        endpoint.batch_sizes.lock().unwrap().as_slice(),
        &[4, 1, 1, 1]
    );
}
