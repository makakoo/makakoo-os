//! Guard tests for `makakoo sancho tick`: shared process coordinator, bounded
//! rebuild waiting, failing/hung exit-2 reporting, and open-probe rebuild wait.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use makakoo_core::superbrain::folder_index::IndexModelState;
use makakoo_core::superbrain::store::SuperbrainStore;
use makakoo_core::superbrain::vector_model::REBUILD_LEASE_MS;
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

#[derive(Clone, Copy)]
enum EmbedBehavior {
    GatedOk,
    FailBatch500,
}

async fn spawn_sancho_mock(
    behavior: EmbedBehavior,
    gate_entered: Arc<AtomicBool>,
    gate_release: Arc<Notify>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let entered = Arc::clone(&gate_entered);
            let release = Arc::clone(&gate_release);
            tokio::spawn(async move {
                let (req_line, body_bytes) = read_http_request(&mut stream).await;
                let (status_line, resp_body) = if req_line.contains("/embeddings") {
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
                        match behavior {
                            EmbedBehavior::GatedOk => {
                                let notified = release.notified();
                                tokio::pin!(notified);
                                notified.as_mut().enable();
                                entered.store(true, Ordering::SeqCst);
                                notified.await;
                                let data: Vec<Value> = (0..inputs)
                                    .map(|_| json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                                    .collect();
                                (
                                    "HTTP/1.1 200 OK",
                                    json!({ "model": "model-v2", "data": data }).to_string(),
                                )
                            }
                            EmbedBehavior::FailBatch500 => {
                                entered.store(true, Ordering::SeqCst);
                                (
                                    "HTTP/1.1 500 Internal Server Error",
                                    json!({ "error": "embedder_boom" }).to_string(),
                                )
                            }
                        }
                    } else {
                        let data: Vec<Value> = (0..inputs)
                            .map(|_| json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                            .collect();
                        (
                            "HTTP/1.1 200 OK",
                            json!({ "model": "model-v2", "data": data }).to_string(),
                        )
                    }
                } else {
                    (
                        "HTTP/1.1 200 OK",
                        json!({
                            "choices": [{ "message": { "content": "NOOP" } }]
                        })
                        .to_string(),
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
fn cli_sancho_engine_shares_process_brain_rebuild_coordinator() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
    let ctx = CliContext::for_home(dir.path().to_path_buf());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let engine = rt.block_on(build_engine(&ctx)).unwrap();
    assert!(
        Arc::ptr_eq(&ctx.brain_rebuild(), &engine.context().brain_rebuild),
        "CLI SANCHO engine must share CliContext's process-wide BrainRebuildCoordinator"
    );
}

#[test]
fn resolve_rebuild_wait_timeout_defaults_to_four_leases_and_parses_env() {
    assert_eq!(
        resolve_rebuild_wait_timeout_with(|_| None),
        Duration::from_millis((REBUILD_LEASE_MS as u64) * 4)
    );
    assert_eq!(
        resolve_rebuild_wait_timeout_with(|k| {
            (k == "MAKAKOO_SANCHO_REBUILD_WAIT_SECS").then(|| "90".to_string())
        }),
        Duration::from_secs(90)
    );
    assert_eq!(
        resolve_rebuild_wait_timeout_with(|k| {
            (k == "MAKAKOO_SANCHO_REBUILD_WAIT_MS").then(|| "250".to_string())
        }),
        Duration::from_millis(250)
    );
}

#[test]
fn one_shot_sancho_tick_waits_for_scheduled_rebuild_to_finish() {
    let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
    let ctx = CliContext::for_home(dir.path().to_path_buf());
    let store = ctx.store().unwrap();
    store
        .write_document(
            "pages/note.md",
            "Pod one-shot sancho tick rebuild wait test.",
            "page",
            json!([]),
        )
        .unwrap();
    store.mark_vectors_stale("model_changed").unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let gate_entered = Arc::new(AtomicBool::new(false));
    let gate_release = Arc::new(Notify::new());
    let base_url = rt.block_on(spawn_sancho_mock(
        EmbedBehavior::GatedOk,
        Arc::clone(&gate_entered),
        Arc::clone(&gate_release),
    ));
    let _env = EnvRestore::set(&[
        ("AIL_BASE_URL", Some(&base_url)),
        ("MAKAKOO_EMBED_MODEL", Some("short-alias")),
        ("MAKAKOO_BULK_EMBED_MODEL", Some("bulk-alias")),
        ("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", Some("1")),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_MS", None),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_SECS", None),
    ]);

    rt.block_on(async move {
        let store_check = Arc::clone(&store);
        let tick_handle = tokio::spawn(async move { run_tick(&ctx).await });

        tokio::time::timeout(Duration::from_secs(10), async {
            while !gate_entered.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("scheduled rebuild never reached batch embed");

        // While the rebuild is in flight, one-shot `tick` must still be waiting.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !tick_handle.is_finished(),
            "one-shot sancho tick exited before its scheduled rebuild finished"
        );
        assert_eq!(
            store_check.vector_status().unwrap().index_state,
            IndexModelState::Rebuilding
        );

        gate_release.notify_waiters();
        let (code, rebuild_ran) = tokio::time::timeout(Duration::from_secs(10), tick_handle)
            .await
            .expect("one-shot sancho tick did not finish after gate release")
            .unwrap()
            .unwrap();
        assert_eq!(code, 0);
        assert!(rebuild_ran);

        let status = store_check.vector_status().unwrap();
        assert_eq!(status.index_state, IndexModelState::Ready);
        assert_eq!(status.embed_model.as_deref(), Some("model-v2"));
        assert_eq!(status.embed_dim, Some(4));
        assert_eq!(store_check.stats().unwrap().vector_count, 1);
    });
}

#[test]
fn one_shot_sancho_tick_exits_2_when_rebuild_fails() {
    let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
    let ctx = CliContext::for_home(dir.path().to_path_buf());
    let store = ctx.store().unwrap();
    store
        .write_document("pages/fail.md", "Failing rebuild doc.", "page", json!([]))
        .unwrap();
    store.mark_vectors_stale("model_changed").unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let gate_entered = Arc::new(AtomicBool::new(false));
    let gate_release = Arc::new(Notify::new());
    let base_url = rt.block_on(spawn_sancho_mock(
        EmbedBehavior::FailBatch500,
        Arc::clone(&gate_entered),
        Arc::clone(&gate_release),
    ));
    let _env = EnvRestore::set(&[
        ("AIL_BASE_URL", Some(&base_url)),
        ("MAKAKOO_EMBED_MODEL", Some("short-alias")),
        ("MAKAKOO_BULK_EMBED_MODEL", Some("bulk-alias")),
        ("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", Some("1")),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_MS", None),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_SECS", None),
    ]);

    rt.block_on(async move {
        let (code, rebuild_ran) = tokio::time::timeout(Duration::from_secs(10), run_tick(&ctx))
            .await
            .expect("one-shot sancho tick hung on failing rebuild")
            .unwrap();
        assert!(rebuild_ran);
        assert_eq!(
            code, 2,
            "failing background rebuild must make one-shot sancho tick exit 2"
        );
        let status = store.vector_status().unwrap();
        assert_eq!(status.index_state, IndexModelState::Stale);
        assert!(
            status
                .stale_reason
                .as_deref()
                .is_some_and(|r| r.starts_with("rebuild_failed:")),
            "unexpected stale_reason: {:?}",
            status.stale_reason
        );
    });
}

#[test]
fn one_shot_sancho_tick_exits_2_when_hung_embedder_hits_wait_bound() {
    let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
    let ctx = CliContext::for_home(dir.path().to_path_buf());
    let store = ctx.store().unwrap();
    store
        .write_document("pages/hang.md", "Hung embedder doc.", "page", json!([]))
        .unwrap();
    store.mark_vectors_stale("model_changed").unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let gate_entered = Arc::new(AtomicBool::new(false));
    let gate_release = Arc::new(Notify::new());
    let base_url = rt.block_on(spawn_sancho_mock(
        EmbedBehavior::GatedOk,
        Arc::clone(&gate_entered),
        Arc::clone(&gate_release),
    ));
    let _env = EnvRestore::set(&[
        ("AIL_BASE_URL", Some(&base_url)),
        ("MAKAKOO_EMBED_MODEL", Some("short-alias")),
        ("MAKAKOO_BULK_EMBED_MODEL", Some("bulk-alias")),
        ("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", Some("1")),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_MS", Some("150")),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_SECS", None),
    ]);

    rt.block_on(async move {
        let (code, rebuild_ran) = tokio::time::timeout(Duration::from_secs(5), run_tick(&ctx))
            .await
            .expect("one-shot sancho tick ignored the bounded wait timeout")
            .unwrap();
        assert!(rebuild_ran);
        assert_eq!(
            code, 2,
            "hung embedder hitting wait bound must make one-shot sancho tick exit 2"
        );
        assert!(gate_entered.load(Ordering::SeqCst));
        let status = store.vector_status().unwrap();
        assert_eq!(status.index_state, IndexModelState::Stale);
        assert_eq!(status.stale_reason.as_deref(), Some("rebuild_cancelled"));
        assert!(!ctx.brain_rebuild().is_active(store.path()));
        // Release the gated server task cleanly before runtime teardown.
        gate_release.notify_waiters();
    });
}

#[test]
fn one_shot_sancho_tick_waits_for_open_probe_started_rebuild_on_shared_coordinator() {
    let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
    // Seed an existing Ready Brain under model-v1 with 1 vector so the open
    // probe detects model-v2 and spawns a rebuild even with SANCHO bg sync OFF.
    {
        let seed = SuperbrainStore::open(&dir.path().join("data/superbrain.db")).unwrap();
        seed.write_document(
            "pages/old.md",
            "Existing doc under model-v1.",
            "page",
            json!([]),
        )
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

    let gate_entered = Arc::new(AtomicBool::new(false));
    let gate_release = Arc::new(Notify::new());
    let base_url = rt.block_on(spawn_sancho_mock(
        EmbedBehavior::GatedOk,
        Arc::clone(&gate_entered),
        Arc::clone(&gate_release),
    ));
    let _env = EnvRestore::set(&[
        ("AIL_BASE_URL", Some(&base_url)),
        ("MAKAKOO_EMBED_MODEL", Some("short-alias")),
        ("MAKAKOO_BULK_EMBED_MODEL", Some("bulk-alias")),
        ("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", None),
        ("HARVEY_ENABLE_BACKGROUND_EMBED_SYNC", None),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_MS", None),
        ("MAKAKOO_SANCHO_REBUILD_WAIT_SECS", None),
    ]);

    rt.block_on(async move {
        let ctx = CliContext::for_home(dir.path().to_path_buf());
        let db_path = dir.path().join("data/superbrain.db");
        let tick_handle = tokio::spawn(async move { run_tick(&ctx).await });

        tokio::time::timeout(Duration::from_secs(10), async {
            while !gate_entered.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("store-open probe never started gated rebuild");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !tick_handle.is_finished(),
            "one-shot sancho tick exited before open-probe rebuild finished"
        );

        gate_release.notify_waiters();
        let (code, rebuild_ran) = tokio::time::timeout(Duration::from_secs(10), tick_handle)
            .await
            .expect("one-shot sancho tick did not finish after open-probe gate release")
            .unwrap()
            .unwrap();
        assert_eq!(code, 0);
        assert!(rebuild_ran);

        let verify = SuperbrainStore::open(&db_path).unwrap();
        let status = verify.vector_status().unwrap();
        assert_eq!(status.index_state, IndexModelState::Ready);
        assert_eq!(status.embed_model.as_deref(), Some("model-v2"));
        assert_eq!(status.embed_dim, Some(4));
    });
}
