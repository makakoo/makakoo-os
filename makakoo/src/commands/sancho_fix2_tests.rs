//! Fix round 2 guard tests for `makakoo sancho tick`:
//! - Decision 2: exit 2 only when this tick scheduled a rebuild (`spawned_count() > 0`)
//!   and it did not reach Ready; pre-existing stale state or a probe outcome alone
//!   exits 0 with one info line.
//! - Decision 3: hitting the wait bound writes owner-checked `rebuild_cancelled`
//!   before exiting, and the next tick claims immediately and resumes without
//!   re-embedding docs that already carry a `(model, dim)` vector.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use makakoo_core::superbrain::folder_index::IndexModelState;
use makakoo_core::superbrain::store::SuperbrainStore;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use super::*;

struct EnvRestore(Vec<(&'static str, Option<String>)>);

impl EnvRestore {
    fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in pairs {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        Self(saved)
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (k, v) in &self.0 {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

fn read_meta(store: &SuperbrainStore, key: &str) -> Option<String> {
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

async fn read_http_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let header_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_len = header_str
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
    let first_line = header_str.lines().next().unwrap_or("").to_string();
    let body = buf.get(header_end..).unwrap_or_default().to_vec();
    (first_line, body)
}

async fn spawn_mismatch_probe_mock(probe_delay_ms: u64) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (req_line, body_bytes) = read_http_request(&mut stream).await;
                let resp_body = if req_line.contains("/embeddings") {
                    if probe_delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(probe_delay_ms)).await;
                    }
                    let body: Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
                    let req_model = body.get("model").and_then(Value::as_str).unwrap_or("");
                    let reported = if req_model == "bulk-alias" {
                        "model-v2"
                    } else {
                        "model-v1"
                    };
                    let inputs = body
                        .get("input")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(1);
                    let data: Vec<Value> = (0..inputs)
                        .map(|_| json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                        .collect();
                    json!({ "model": reported, "data": data }).to_string()
                } else {
                    json!({ "choices": [{ "message": { "content": "NOOP" } }] }).to_string()
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

struct ResumableMock {
    base_url: String,
    batch_inputs: Arc<Mutex<Vec<usize>>>,
    hang_second_batch: Arc<AtomicBool>,
    gate_release: Arc<Notify>,
}

async fn spawn_resumable_mock() -> ResumableMock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let batch_inputs = Arc::new(Mutex::new(Vec::new()));
    let hang_second_batch = Arc::new(AtomicBool::new(true));
    let gate_release = Arc::new(Notify::new());
    let batches_seen = Arc::new(AtomicUsize::new(0));

    let bi_srv = Arc::clone(&batch_inputs);
    let hang_srv = Arc::clone(&hang_second_batch);
    let rel_srv = Arc::clone(&gate_release);
    let seen_srv = Arc::clone(&batches_seen);

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let bi = Arc::clone(&bi_srv);
            let hang = Arc::clone(&hang_srv);
            let rel = Arc::clone(&rel_srv);
            let seen = Arc::clone(&seen_srv);
            tokio::spawn(async move {
                let (req_line, body_bytes) = read_http_request(&mut stream).await;
                let resp_body = if req_line.contains("/embeddings") {
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
                        let n = seen.fetch_add(1, Ordering::SeqCst);
                        if n >= 1 && hang.load(Ordering::SeqCst) {
                            let notified = rel.notified();
                            tokio::pin!(notified);
                            notified.as_mut().enable();
                            notified.await;
                        }
                    }
                    let data: Vec<Value> = (0..inputs)
                        .map(|_| json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                        .collect();
                    json!({ "model": "model-v2", "data": data }).to_string()
                } else {
                    json!({ "choices": [{ "message": { "content": "NOOP" } }] }).to_string()
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });

    ResumableMock {
        base_url: format!("http://{addr}"),
        batch_inputs,
        hang_second_batch,
        gate_release,
    }
}

async fn spawn_embed_unavailable_mock(embed_delay_ms: u64) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (req_line, _) = read_http_request(&mut stream).await;
                let (status_line, resp_body) = if req_line.contains("/embeddings") {
                    if embed_delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(embed_delay_ms)).await;
                    }
                    (
                        "HTTP/1.1 503 Service Unavailable",
                        json!({ "error": "embeddings_unavailable" }).to_string(),
                    )
                } else {
                    (
                        "HTTP/1.1 200 OK",
                        json!({ "choices": [{ "message": { "content": "NOOP" } }] }).to_string(),
                    )
                };
                let reply = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

#[test]
fn one_shot_sancho_tick_exits_0_with_info_on_probe_mismatch_without_spawn() {
    let _guard = crate::test_support::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
    let db_path = dir.path().join("data/superbrain.db");
    {
        let seed = SuperbrainStore::open(&db_path).unwrap();
        seed.write_document("pages/old.md", "existing doc", "page", json!([]))
            .unwrap();
        seed.store_vector_with_model("pages/old.md", &[1.0, 0.0, 0.0, 0.0], "model-v1")
            .unwrap();
        seed.set_vectors_ready("model-v1", 4, "bulk-alias", Some("short-alias"))
            .unwrap();
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    // Delay probe responses slightly so `has_active_work()` is true after
    // `tick_once()`, exercising the exact r2 wait-without-spawn path.
    let base_url = rt.block_on(spawn_mismatch_probe_mock(120));
    let _env = EnvRestore::set(&[
        ("AIL_BASE_URL", Some(&base_url)),
        ("MAKAKOO_EMBED_MODEL", Some("short-alias")),
        ("MAKAKOO_BULK_EMBED_MODEL", Some("bulk-alias")),
        ("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", None),
        ("HARVEY_ENABLE_BACKGROUND_EMBED_SYNC", None),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_MS", None),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_SECS", None),
        ("MAKAKOO_REBUILD_CALL_TIMEOUT_MS", None),
        ("MAKAKOO_REBUILD_CALL_TIMEOUT_SECS", None),
    ]);

    rt.block_on(async move {
        let ctx = CliContext::for_home(dir.path().to_path_buf());
        let out = tokio::time::timeout(Duration::from_secs(10), run_tick_detailed(&ctx))
            .await
            .expect("sancho tick hung on mismatch probe")
            .unwrap();
        assert!(
            !out.rebuild_ran,
            "mismatch probe must not schedule a rebuild"
        );
        assert_eq!(
            out.exit_code, 0,
            "probe outcome without scheduled rebuild must exit 0"
        );
        let note = out
            .status_note
            .as_deref()
            .expect("non-ready index without rebuild must emit one info line");
        assert!(
            note.contains("not ready") && note.contains("short_bulk_model_mismatch"),
            "unexpected status info line: {note:?}"
        );
        let verify = SuperbrainStore::open(&db_path).unwrap();
        let status = verify.vector_status().unwrap();
        assert_eq!(status.index_state, IndexModelState::Stale);
        assert_eq!(
            status.stale_reason.as_deref(),
            Some("short_bulk_model_mismatch")
        );
    });
}

#[test]
fn one_shot_sancho_tick_exits_0_with_info_on_preexisting_stale_without_spawn() {
    let _guard = crate::test_support::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
    let db_path = dir.path().join("data/superbrain.db");
    {
        let seed = SuperbrainStore::open(&db_path).unwrap();
        seed.write_document("pages/stale.md", "stale doc", "page", json!([]))
            .unwrap();
        seed.store_vector_with_model("pages/stale.md", &[1.0, 0.0, 0.0, 0.0], "model-v1")
            .unwrap();
        seed.set_vectors_ready("model-v1", 4, "bulk-alias", Some("short-alias"))
            .unwrap();
        seed.mark_vectors_stale("previous_embed_error").unwrap();
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    // Chat completions succeed while `/embeddings` fails with 503 after a short
    // delay (so `has_active_work()` is true after `tick_once()`, leaving the
    // pre-existing stale state untouched and scheduling no rebuild).
    let base_url = rt.block_on(spawn_embed_unavailable_mock(120));
    let _env = EnvRestore::set(&[
        ("AIL_BASE_URL", Some(&base_url)),
        ("MAKAKOO_EMBED_MODEL", Some("short-alias")),
        ("MAKAKOO_BULK_EMBED_MODEL", Some("bulk-alias")),
        ("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", None),
        ("HARVEY_ENABLE_BACKGROUND_EMBED_SYNC", None),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_MS", None),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_SECS", None),
        ("MAKAKOO_REBUILD_CALL_TIMEOUT_MS", None),
        ("MAKAKOO_REBUILD_CALL_TIMEOUT_SECS", None),
    ]);

    rt.block_on(async move {
        let ctx = CliContext::for_home(dir.path().to_path_buf());
        let out = tokio::time::timeout(Duration::from_secs(10), run_tick_detailed(&ctx))
            .await
            .expect("sancho tick hung on pre-existing stale state")
            .unwrap();
        assert!(!out.rebuild_ran);
        assert_eq!(
            out.exit_code, 0,
            "pre-existing stale index without scheduled rebuild must exit 0"
        );
        let note = out
            .status_note
            .as_deref()
            .expect("pre-existing stale index must emit one info line");
        assert!(
            note.contains("not ready") && note.contains("previous_embed_error"),
            "unexpected status info line: {note:?}"
        );
    });
}

#[test]
fn one_shot_sancho_tick_timeout_cancels_claim_and_next_tick_resumes_skipping_done_docs() {
    let _guard = crate::test_support::ENV_MUTEX
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
    let db_path = dir.path().join("data/superbrain.db");
    {
        let seed = SuperbrainStore::open(&db_path).unwrap();
        // 35 docs > REBUILD_BATCH_SIZE (32): tick 1 embeds 32 docs, hangs on
        // the remaining 3, and hits the wait bound.
        for i in 0..35 {
            seed.write_document(
                &format!("pages/doc-{i:02}.md"),
                &format!("resumable doc {i}"),
                "page",
                json!([]),
            )
            .unwrap();
        }
        seed.mark_vectors_stale("model_changed").unwrap();
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let mock = rt.block_on(spawn_resumable_mock());
    let _env = EnvRestore::set(&[
        ("AIL_BASE_URL", Some(&mock.base_url)),
        ("MAKAKOO_EMBED_MODEL", Some("short-alias")),
        ("MAKAKOO_BULK_EMBED_MODEL", Some("bulk-alias")),
        ("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", Some("1")),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_MS", Some("350")),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_SECS", None),
        ("MAKAKOO_REBUILD_CALL_TIMEOUT_MS", None),
        ("MAKAKOO_REBUILD_CALL_TIMEOUT_SECS", None),
    ]);

    rt.block_on(async move {
        // Tick 1: completes batch 1 (32 docs), hangs on batch 2, hits 350ms bound.
        let ctx1 = CliContext::for_home(dir.path().to_path_buf());
        let out1 = tokio::time::timeout(Duration::from_secs(5), run_tick_detailed(&ctx1))
            .await
            .expect("tick 1 ignored bounded wait timeout")
            .unwrap();
        assert!(out1.rebuild_ran);
        assert_eq!(out1.exit_code, 2);

        let verify1 = SuperbrainStore::open(&db_path).unwrap();
        let status1 = verify1.vector_status().unwrap();
        assert_eq!(status1.index_state, IndexModelState::Stale);
        assert_eq!(
            status1.stale_reason.as_deref(),
            Some("rebuild_cancelled"),
            "timeout must write owner-checked rebuild_cancelled before exiting"
        );
        assert!(read_meta(&verify1, "rebuild_owner").is_none());
        assert!(read_meta(&verify1, "rebuild_heartbeat_ms").is_none());
        assert_eq!(verify1.stats().unwrap().vector_count, 32);

        // Unblock mock server and reset recorded batch inputs.
        mock.hang_second_batch.store(false, Ordering::SeqCst);
        mock.gate_release.notify_waiters();
        mock.batch_inputs.lock().unwrap().clear();

        // Tick 2 (fresh process/CliContext, no 120s wait): claims immediately
        // and embeds ONLY the remaining 3 docs (skips the 32 already-embedded).
        std::env::set_var("MAKAKOO_SANCHO_REBUILD_WAIT_MS", "5000");
        let ctx2 = CliContext::for_home(dir.path().to_path_buf());
        let out2 = tokio::time::timeout(Duration::from_secs(10), run_tick_detailed(&ctx2))
            .await
            .expect("tick 2 did not finish resumed rebuild")
            .unwrap();
        assert!(out2.rebuild_ran);
        assert_eq!(out2.exit_code, 0);

        let verify2 = SuperbrainStore::open(&db_path).unwrap();
        let status2 = verify2.vector_status().unwrap();
        assert_eq!(status2.index_state, IndexModelState::Ready);
        assert_eq!(status2.embed_model.as_deref(), Some("model-v2"));
        assert_eq!(verify2.stats().unwrap().vector_count, 35);
        assert_eq!(
            mock.batch_inputs.lock().unwrap().as_slice(),
            &[3],
            "resumed tick must embed only the 3 remaining docs, not re-embed the 32 done docs"
        );
    });
}
