//! Guard tests for per-call rebuild timeouts (hung batch & probe, no
//! whole-rebuild ceiling) and coordinator cancel + resumable skip.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use crate::embeddings::{EmbeddingClient, DEFAULT_EMBED_TIMEOUT, PROBE_TIMEOUT};
use crate::superbrain::folder_index::IndexModelState;
use crate::superbrain::store::SuperbrainStore;
use crate::superbrain::vector_claim::{META_REBUILD_HEARTBEAT, META_REBUILD_OWNER};
use crate::superbrain::vector_model::{
    rebuild_brain_vectors, BrainRebuildCoordinator, REBUILD_CALL_TIMEOUT_MS_ENV,
    REBUILD_CALL_TIMEOUT_SECS_ENV,
};
use crate::superbrain::vector_rebuild_loop::resolve_rebuild_call_timeout_with;

pub(crate) fn read_meta(store: &SuperbrainStore, key: &str) -> Option<String> {
    store
        .conn_arc()
        .lock()
        .unwrap()
        .query_row(
            "SELECT value FROM brain_vector_meta WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
}

async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            return Vec::new();
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let content_len = headers
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_len {
        let n = stream.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    buf.get(header_end..).unwrap_or_default().to_vec()
}

#[derive(Clone, Copy)]
pub(crate) enum FakeServerMode {
    Ok,
    HangBatch,
    HangProbe,
    DelayBatchMs(u64),
    HangAfterFirstBatch,
}

pub(crate) struct FakeEmbedServer {
    pub(crate) base_url: String,
    pub(crate) batch_inputs: Arc<Mutex<Vec<usize>>>,
    pub(crate) gate_entered: Arc<AtomicBool>,
    pub(crate) gate_release: Arc<Notify>,
    pub(crate) hang_enabled: Arc<AtomicBool>,
}

pub(crate) async fn spawn_fake_embed_server(model: &str, mode: FakeServerMode) -> FakeEmbedServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let model = model.to_string();
    let batch_inputs = Arc::new(Mutex::new(Vec::new()));
    let gate_entered = Arc::new(AtomicBool::new(false));
    let gate_release = Arc::new(Notify::new());
    let hang_enabled = Arc::new(AtomicBool::new(true));
    let batch_counter = Arc::new(AtomicUsize::new(0));

    let bi_srv = Arc::clone(&batch_inputs);
    let ge_srv = Arc::clone(&gate_entered);
    let gr_srv = Arc::clone(&gate_release);
    let he_srv = Arc::clone(&hang_enabled);
    let bc_srv = Arc::clone(&batch_counter);

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let model = model.clone();
            let bi = Arc::clone(&bi_srv);
            let ge = Arc::clone(&ge_srv);
            let gr = Arc::clone(&gr_srv);
            let he = Arc::clone(&he_srv);
            let bc = Arc::clone(&bc_srv);
            tokio::spawn(async move {
                let body_bytes = read_http_body(&mut stream).await;
                let body: Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
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
                if !is_probe {
                    bi.lock().unwrap().push(inputs);
                }
                let should_hang = match mode {
                    FakeServerMode::Ok => false,
                    FakeServerMode::HangBatch => !is_probe && he.load(Ordering::SeqCst),
                    FakeServerMode::HangProbe => is_probe && he.load(Ordering::SeqCst),
                    FakeServerMode::DelayBatchMs(ms) => {
                        if !is_probe {
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                        }
                        false
                    }
                    FakeServerMode::HangAfterFirstBatch => {
                        !is_probe
                            && bc.fetch_add(1, Ordering::SeqCst) >= 1
                            && he.load(Ordering::SeqCst)
                    }
                };
                if should_hang {
                    let notified = gr.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    ge.store(true, Ordering::SeqCst);
                    notified.await;
                }
                let data: Vec<Value> = (0..inputs)
                    .map(|_| json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                    .collect();
                let resp_body = json!({ "model": model, "data": data }).to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });

    FakeEmbedServer {
        base_url: format!("http://{addr}"),
        batch_inputs,
        gate_entered,
        gate_release,
        hang_enabled,
    }
}

#[test]
fn resolve_rebuild_call_timeout_uses_existing_client_defaults_and_env_overrides() {
    assert_eq!(
        resolve_rebuild_call_timeout_with(None, false, |_| None),
        DEFAULT_EMBED_TIMEOUT
    );
    assert_eq!(
        resolve_rebuild_call_timeout_with(None, true, |_| None),
        PROBE_TIMEOUT
    );
    assert_eq!(
        resolve_rebuild_call_timeout_with(None, false, |k| {
            (k == REBUILD_CALL_TIMEOUT_SECS_ENV).then(|| "120".to_string())
        }),
        Duration::from_secs(120)
    );
    assert_eq!(
        resolve_rebuild_call_timeout_with(None, true, |k| {
            (k == REBUILD_CALL_TIMEOUT_MS_ENV).then(|| "250".to_string())
        }),
        Duration::from_millis(250)
    );
    assert_eq!(
        resolve_rebuild_call_timeout_with(Some(Duration::from_millis(90)), false, |k| {
            (k == REBUILD_CALL_TIMEOUT_SECS_ENV).then(|| "120".to_string())
        }),
        Duration::from_millis(90)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hung_embed_batch_times_out_releases_claim_and_second_claimant_succeeds() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    // Short lease (120 ms -> ticker every 30 ms) < call timeout (240 ms) proves
    // the per-call timeout fires even while the heartbeat ticker stays alive.
    store.set_rebuild_lease_ms_for_test(120);
    store
        .write_document("pages/a.md", "alpha", "page", json!([]))
        .unwrap();

    let hung = spawn_fake_embed_server("model-hung", FakeServerMode::HangBatch).await;
    let good = spawn_fake_embed_server("model-good", FakeServerMode::Ok).await;
    let bulk_hung = Arc::new(
        EmbeddingClient::with_base_url(&hung.base_url)
            .with_model("bulk")
            .with_timeout(Duration::from_millis(240)),
    );
    let bulk_good = Arc::new(EmbeddingClient::with_base_url(&good.base_url).with_model("bulk"));

    let coord = Arc::new(BrainRebuildCoordinator::new());
    assert!(
        coord
            .spawn_rebuild(Arc::clone(&store), bulk_hung, None, None)
            .await
    );
    tokio::time::timeout(Duration::from_secs(3), coord.wait(store.path()))
        .await
        .expect("hung embed batch did not fail within per-call timeout");

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(
        status.stale_reason.as_deref(),
        Some("rebuild_failed: timeout")
    );
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());
    assert!(!coord.is_active(store.path()));

    // Claim is free: a second claimant immediately succeeds.
    assert!(
        coord
            .spawn_rebuild(Arc::clone(&store), bulk_good, None, None)
            .await
    );
    tokio::time::timeout(Duration::from_secs(3), coord.wait(store.path()))
        .await
        .expect("second claimant did not finish");
    let after = store.vector_status().unwrap();
    assert_eq!(after.index_state, IndexModelState::Ready);
    assert_eq!(after.embed_model.as_deref(), Some("model-good"));
    assert_eq!(store.stats().unwrap().vector_count, 1);

    hung.gate_release.notify_waiters();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hung_probe_times_out_releases_claim_and_second_claimant_succeeds() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store.set_rebuild_lease_ms_for_test(120);
    store
        .write_document("pages/p.md", "probe doc", "page", json!([]))
        .unwrap();

    let hung = spawn_fake_embed_server("model-hung", FakeServerMode::HangProbe).await;
    let good = spawn_fake_embed_server("model-good", FakeServerMode::Ok).await;
    let bulk_hung = Arc::new(
        EmbeddingClient::with_base_url(&hung.base_url)
            .with_model("bulk")
            .with_timeout(Duration::from_millis(200)),
    );
    let bulk_good = Arc::new(EmbeddingClient::with_base_url(&good.base_url).with_model("bulk"));

    let coord = Arc::new(BrainRebuildCoordinator::new());
    assert!(
        coord
            .spawn_rebuild(Arc::clone(&store), bulk_hung, None, None)
            .await
    );
    tokio::time::timeout(Duration::from_secs(3), coord.wait(store.path()))
        .await
        .expect("hung probe did not fail within per-call timeout");

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(
        status.stale_reason.as_deref(),
        Some("rebuild_failed: timeout")
    );
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(!coord.is_active(store.path()));

    assert!(
        coord
            .spawn_rebuild(Arc::clone(&store), bulk_good, None, None)
            .await
    );
    tokio::time::timeout(Duration::from_secs(3), coord.wait(store.path()))
        .await
        .expect("second claimant did not finish after hung probe");
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );
    hung.gate_release.notify_waiters();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_batch_rebuild_has_no_whole_rebuild_ceiling() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    // 65 docs -> 5 batches (16 + 16 + 16 + 16 + 1). Each batch sleeps 90 ms
    // (< 220 ms per-call timeout), so total rebuild time (~450 ms) exceeds 1x call timeout.
    for i in 0..65 {
        store
            .write_document(
                &format!("pages/doc-{i:02}.md"),
                &format!("body {i}"),
                "page",
                json!([]),
            )
            .unwrap();
    }
    let slow = spawn_fake_embed_server("model-big", FakeServerMode::DelayBatchMs(90)).await;
    let bulk = Arc::new(
        EmbeddingClient::with_base_url(&slow.base_url)
            .with_model("bulk")
            .with_timeout(Duration::from_millis(220)),
    );
    let written = rebuild_brain_vectors(Arc::clone(&store), bulk, None, None)
        .await
        .unwrap();
    assert_eq!(written, 65);
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );
    assert_eq!(
        slow.batch_inputs.lock().unwrap().as_slice(),
        &[16, 16, 16, 16, 1]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_rebuild_releases_claim_and_resume_skips_already_embedded_docs() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    for i in 0..35 {
        store
            .write_document(
                &format!("pages/doc-{i:02}.md"),
                &format!("doc body {i}"),
                "page",
                json!([]),
            )
            .unwrap();
    }

    let srv = spawn_fake_embed_server("model-v2", FakeServerMode::HangAfterFirstBatch).await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(&srv.base_url).with_model("bulk"));
    let coord = Arc::new(BrainRebuildCoordinator::new());

    assert!(
        coord
            .spawn_rebuild(Arc::clone(&store), Arc::clone(&bulk), None, None)
            .await
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while !srv.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("rebuild never reached second (gated) batch");
    assert_eq!(store.stats().unwrap().vector_count, 16);

    // Cancel active rebuild: must write owner-checked `rebuild_cancelled` and free claim.
    coord.cancel_active_rebuilds().await;
    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(status.stale_reason.as_deref(), Some("rebuild_cancelled"));
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());
    assert!(!coord.is_active(store.path()));

    // Resume immediately without waiting 120 s lease; only remaining 19 docs get embedded.
    srv.hang_enabled.store(false, Ordering::SeqCst);
    srv.gate_release.notify_waiters();
    srv.batch_inputs.lock().unwrap().clear();

    assert!(
        coord
            .spawn_rebuild(Arc::clone(&store), bulk, None, None)
            .await
    );
    tokio::time::timeout(Duration::from_secs(5), coord.wait(store.path()))
        .await
        .expect("resumed rebuild did not finish");

    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );
    assert_eq!(store.stats().unwrap().vector_count, 35);
    assert_eq!(
        srv.batch_inputs.lock().unwrap().as_slice(),
        &[16, 3],
        "resumed rebuild must skip the 16 docs already embedded with (model-v2, 4)"
    );
}
