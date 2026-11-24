//! Guard tests for concurrent rebuild heartbeat ticker, unfenced state writers,
//! wall-clock step-back lease window, stale-mark retry + warn, rebuild panic
//! watcher, and `spawn_blocking` claim execution.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use tempfile::TempDir;
use tracing::instrument::WithSubscriber;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;
use crate::error::MakakooError;
use crate::superbrain::folder_index::IndexModelState;
use crate::superbrain::store::SuperbrainStore;
use crate::superbrain::vector_claim::{
    mark_stale_for_owner_with_retry, META_REBUILD_HEARTBEAT, META_REBUILD_OWNER, REBUILD_LEASE_MS,
};
use crate::superbrain::vector_model::{rebuild_brain_vectors, BrainRebuildCoordinator};
use crate::superbrain::vector_rebuild::rebuild_brain_vectors_with_owner;

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

fn set_meta(store: &SuperbrainStore, key: &str, value: &str) {
    store
        .conn_arc()
        .lock()
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO brain_vector_meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .unwrap();
}

#[derive(Clone)]
struct GatedResponder {
    model: String,
    dim: usize,
    gate_batches: bool,
    gate_entered: Arc<AtomicBool>,
    gate_release: Arc<AtomicBool>,
}

impl Respond for GatedResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
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
        if self.gate_batches && !is_probe {
            self.gate_entered.store(true, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(15);
            while !self.gate_release.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let mut vec = vec![0.0_f32; self.dim];
        vec[0] = 1.0;
        let data: Vec<Value> = (0..inputs).map(|_| json!({ "embedding": vec })).collect();
        ResponseTemplate::new(200).set_body_json(json!({ "model": self.model, "data": data }))
    }
}

async fn start_gated_server(
    model: &str,
    gate_batches: bool,
) -> (MockServer, Arc<AtomicBool>, Arc<AtomicBool>) {
    let gate_entered = Arc::new(AtomicBool::new(false));
    let gate_release = Arc::new(AtomicBool::new(!gate_batches));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(GatedResponder {
            model: model.to_string(),
            dim: 4,
            gate_batches,
            gate_entered: Arc::clone(&gate_entered),
            gate_release: Arc::clone(&gate_release),
        })
        .mount(&server)
        .await;
    (server, gate_entered, gate_release)
}

async fn wait_gate(flag: &AtomicBool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !flag.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("rebuild never reached gated embed batch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_heartbeat_keeps_lease_fresh_across_slow_batch_and_blocks_competitor() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("sb.db");
    let store = Arc::new(SuperbrainStore::open(&db_path).unwrap());
    // Short per-store test lease (200 ms -> ticker every 50 ms) so one gated
    // batch lasts longer than a full lease without sleeping 120 s.
    store.set_rebuild_lease_ms_for_test(200);
    store
        .write_document("pages/a.md", "alpha", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/b.md", "beta", "page", json!([]))
        .unwrap();

    // Independent DB handle simulating a competing process with the same lease.
    let competitor = SuperbrainStore::open(&db_path).unwrap();
    competitor.set_rebuild_lease_ms_for_test(200);

    let (server, gate_entered, gate_release) = start_gated_server("model-slow", true).await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-slow"));

    let store_task = Arc::clone(&store);
    let task =
        tokio::spawn(async move { rebuild_brain_vectors(store_task, bulk, None, None).await });
    wait_gate(&gate_entered).await;

    let hb_start: i64 = read_meta(&store, META_REBUILD_HEARTBEAT)
        .unwrap()
        .parse()
        .unwrap();

    // Hold the single batch open for > 1.75x the 200 ms lease.
    tokio::time::sleep(Duration::from_millis(360)).await;

    let hb_mid: i64 = read_meta(&store, META_REBUILD_HEARTBEAT)
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        hb_mid > hb_start,
        "concurrent heartbeat ticker must advance rebuild_heartbeat_ms during a long batch"
    );
    assert!(competitor.rebuild_lease_fresh().unwrap());
    assert!(
        competitor
            .try_mark_vectors_rebuilding("competitor")
            .unwrap()
            .is_none(),
        "competing claimant must be refused while concurrent heartbeat ticker is active"
    );

    gate_release.store(true, Ordering::SeqCst);
    let written = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("rebuild did not finish after gate release")
        .unwrap()
        .unwrap();
    assert_eq!(written, 2);

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-slow"));

    // Confirm ticker stopped after rebuild end and did not re-write claim meta.
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_heartbeat_cancels_in_flight_rebuild_on_claim_lost() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store.set_rebuild_lease_ms_for_test(160);
    store
        .write_document("pages/a.md", "alpha", "page", json!([]))
        .unwrap();

    let (server, gate_entered, gate_release) = start_gated_server("model-a", true).await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-a"));
    let owner = store
        .try_mark_vectors_rebuilding("owner-a")
        .unwrap()
        .unwrap();
    let store_task = Arc::clone(&store);
    let task = tokio::spawn(async move {
        rebuild_brain_vectors_with_owner(store_task, &owner, bulk, None, None).await
    });
    wait_gate(&gate_entered).await;

    // Evict ownership while the HTTP batch is still blocked on the gate.
    store.mark_vectors_stale("evicted_mid_batch").unwrap();

    // Without releasing the gate, the ticker detects RebuildClaimLost and cancels.
    let res = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("heartbeat ticker did not cancel rebuild on RebuildClaimLost")
        .unwrap();
    assert!(
        matches!(res, Err(MakakooError::RebuildClaimLost)),
        "expected RebuildClaimLost, got {res:?}"
    );
    gate_release.store(true, Ordering::SeqCst);
}

#[test]
fn unfenced_state_writers_clear_claim_meta_and_fence_old_owner() {
    let dir = TempDir::new().unwrap();
    let store = SuperbrainStore::open(&dir.path().join("sb.db")).unwrap();
    store
        .write_document("pages/0.md", "alpha", "page", json!([]))
        .unwrap();

    // 1. mark_vectors_stale deletes rebuild_owner and rebuild_heartbeat_ms.
    let owner_a = store
        .try_mark_vectors_rebuilding("owner-a")
        .unwrap()
        .unwrap();
    store.mark_vectors_stale("manual_stale").unwrap();
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());

    // Even if index_state flips back to rebuilding, old owner_a stays fenced.
    set_meta(&store, "index_state", "rebuilding");
    assert!(matches!(
        store.store_vector_for_rebuild(&owner_a, "pages/0.md", &[1.0, 0.0, 0.0, 0.0], "model-a"),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert!(matches!(
        store.touch_rebuild_heartbeat(&owner_a),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert!(matches!(
        store.finish_rebuild(&owner_a, "model-a", 4, "bulk-a", None),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert!(!store
        .mark_vectors_stale_for_owner(&owner_a, "late-stale")
        .unwrap());
    assert_eq!(store.stats().unwrap().vector_count, 0);

    // 2. set_vectors_ready clears claim meta atomically too.
    store.mark_vectors_stale("reset").unwrap();
    let owner_b = store
        .try_mark_vectors_rebuilding("owner-b")
        .unwrap()
        .unwrap();
    store
        .set_vectors_ready("model-ready", 4, "bulk", None)
        .unwrap();
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());
    set_meta(&store, "index_state", "rebuilding");
    assert!(matches!(
        store.store_vector_for_rebuild(&owner_b, "pages/0.md", &[1.0, 0.0, 0.0, 0.0], "model-b"),
        Err(MakakooError::RebuildClaimLost)
    ));

    // 3. check_brain_alias_on_open empty-store reset clears expired claim meta.
    store.mark_vectors_stale("reset2").unwrap();
    let owner_c = store
        .try_mark_vectors_rebuilding("owner-c")
        .unwrap()
        .unwrap();
    set_meta(&store, META_REBUILD_HEARTBEAT, "0");
    let status = store.check_brain_alias_on_open("bulk", None).unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());
    set_meta(&store, "index_state", "rebuilding");
    assert!(matches!(
        store.store_vector_for_rebuild(&owner_c, "pages/0.md", &[1.0, 0.0, 0.0, 0.0], "model-c"),
        Err(MakakooError::RebuildClaimLost)
    ));
}

#[test]
fn wall_clock_step_back_within_one_lease_stays_fresh_and_beyond_expires() {
    let dir = TempDir::new().unwrap();
    let store = SuperbrainStore::open(&dir.path().join("sb.db")).unwrap();
    let _owner = store
        .try_mark_vectors_rebuilding("clock-test")
        .unwrap()
        .unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    // Wall clock stepped back 30 s (age = -30_000, inside (-LEASE, LEASE)).
    set_meta(&store, META_REBUILD_HEARTBEAT, &(now + 30_000).to_string());
    assert!(store.rebuild_lease_fresh().unwrap());
    assert!(store
        .try_mark_vectors_rebuilding("steal")
        .unwrap()
        .is_none());

    // Wall clock stepped back beyond one full lease (age <= -LEASE): expired.
    set_meta(
        &store,
        META_REBUILD_HEARTBEAT,
        &(now + REBUILD_LEASE_MS + 1_000).to_string(),
    );
    assert!(!store.rebuild_lease_fresh().unwrap());
    assert!(store
        .try_mark_vectors_rebuilding("takeover")
        .unwrap()
        .is_some());
}

#[derive(Clone, Default)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_stale_mark_retries_once_and_logs_warn_on_double_failure() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("sb.db");
    let store = Arc::new(SuperbrainStore::open(&db_path).unwrap());
    store
        .conn_arc()
        .lock()
        .unwrap()
        .busy_timeout(Duration::from_millis(15))
        .unwrap();

    // Part 1: transient SQLITE_BUSY clears before retry -> succeeds on attempt 2.
    let owner1 = store
        .try_mark_vectors_rebuilding("retry-ok")
        .unwrap()
        .unwrap();
    let db_for_lock = db_path.clone();
    let lock_acquired = Arc::new(AtomicBool::new(false));
    let lock_acquired_bg = Arc::clone(&lock_acquired);
    let holder = std::thread::spawn(move || {
        let mut c2 = Connection::open(&db_for_lock).unwrap();
        let tx = c2
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        lock_acquired_bg.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(35));
        tx.commit().unwrap();
    });
    while !lock_acquired.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(2));
    }
    let marked = mark_stale_for_owner_with_retry(&store, &owner1, "retry_recovered").await;
    holder.join().unwrap();
    assert!(marked, "stale mark must succeed on retry after busy clears");
    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(status.stale_reason.as_deref(), Some("retry_recovered"));

    // Part 2: persistent SQLITE_BUSY across both attempts -> false + WARN log.
    let owner2 = store
        .try_mark_vectors_rebuilding("retry-fail")
        .unwrap()
        .unwrap();
    let mut c2 = Connection::open(&db_path).unwrap();
    let tx = c2
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let log_buf = BufWriter::default();
    let log_sink = log_buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer(move || log_sink.clone())
        .finish();
    let marked2 = mark_stale_for_owner_with_retry(&store, &owner2, "still_busy")
        .with_subscriber(subscriber)
        .await;
    drop(tx);
    assert!(!marked2);
    let logs = String::from_utf8(log_buf.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("WARN") && logs.contains("failed to mark brain vectors stale after retry"),
        "expected warn log after second stale-mark failure, got: {logs:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panic_in_rebuild_task_marks_index_stale_rebuild_panicked() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/p.md", "panic test", "page", json!([]))
        .unwrap();

    let coord = Arc::new(BrainRebuildCoordinator::new());
    *coord.before_rebuild.lock().unwrap() = Some(Box::new(|| {
        panic!("simulated rebuild worker panic");
    }));
    let bulk = Arc::new(EmbeddingClient::with_base_url("http://127.0.0.1:9").with_model("bulk"));

    assert!(
        coord
            .spawn_rebuild(Arc::clone(&store), bulk, None, None)
            .await
    );
    tokio::time::timeout(Duration::from_secs(5), coord.wait(store.path()))
        .await
        .expect("coord.wait did not unblock after rebuild panic");

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(status.stale_reason.as_deref(), Some("rebuild_panicked"));
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());
    assert!(!coord.is_active(store.path()));
}

#[tokio::test]
async fn spawn_rebuild_claim_runs_in_spawn_blocking_without_stalling_async_executor() {
    // Runs on a single-threaded current_thread runtime: an inline BEGIN IMMEDIATE
    // would block the executor so `releaser` could never run before SQLite's 5 s
    // busy_timeout failed the claim.
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("sb.db");
    let store = Arc::new(SuperbrainStore::open(&db_path).unwrap());
    store
        .write_document("pages/sb.md", "spawn_blocking test", "page", json!([]))
        .unwrap();

    let (server, _, _) = start_gated_server("model-sb", false).await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-sb"));

    let c2 = Connection::open(&db_path).unwrap();
    c2.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let releaser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c2.execute_batch("COMMIT;").unwrap();
    });

    let coord = Arc::new(BrainRebuildCoordinator::new());
    let claimed = tokio::time::timeout(
        Duration::from_secs(2),
        coord.spawn_rebuild(Arc::clone(&store), bulk, None, None),
    )
    .await
    .expect("spawn_rebuild stalled the current_thread async executor");
    releaser.await.unwrap();
    assert!(
        claimed,
        "claim must succeed once competing write lock drops"
    );

    tokio::time::timeout(Duration::from_secs(5), coord.wait(store.path()))
        .await
        .expect("rebuild did not finish");
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );
}
