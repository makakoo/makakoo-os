//! Guard tests for cancel-safe rebuild claims (dropped caller futures) and
//! cross-connection `BEGIN IMMEDIATE` serialization in `store_vector_if_ready_for`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::json;
use tempfile::TempDir;

use crate::embeddings::EmbeddingClient;
use crate::superbrain::folder_index::IndexModelState;
use crate::superbrain::store::SuperbrainStore;
use crate::superbrain::vector_claim::{META_REBUILD_HEARTBEAT, META_REBUILD_OWNER};
use crate::superbrain::vector_model::{rebuild_brain_vectors, BrainRebuildCoordinator};

use super::timeout_tests::{read_meta, spawn_fake_embed_server, FakeServerMode};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_spawn_rebuild_future_after_claim_does_not_orphan_claim_or_heartbeat() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/a.md", "alpha", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/b.md", "beta", "page", json!([]))
        .unwrap();

    let srv = spawn_fake_embed_server("model-cancel-safe", FakeServerMode::Ok).await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(&srv.base_url).with_model("bulk"));
    let coord = Arc::new(BrainRebuildCoordinator::new());

    let claim_committed = Arc::new(AtomicBool::new(false));
    let release_claim = Arc::new(AtomicBool::new(false));
    let cc = Arc::clone(&claim_committed);
    let rc = Arc::clone(&release_claim);
    *coord.after_claim.lock().unwrap() = Some(Arc::new(move || {
        cc.store(true, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !rc.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }));

    let coord_caller = Arc::clone(&coord);
    let store_caller = Arc::clone(&store);
    let caller = tokio::spawn(async move {
        coord_caller
            .spawn_rebuild(store_caller, bulk, None, None)
            .await
    });

    // Wait until the DB claim has already committed inside `spawn_blocking`.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !claim_committed.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("after_claim hook never fired");
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Rebuilding
    );

    // Drop/abort the caller future while it is still awaiting `spawn_rebuild`.
    caller.abort();
    let _ = caller.await;

    // Release the claim hook: the detached task must keep owning the claim and
    // run the rebuild to Ready rather than leaving the DB stuck Rebuilding.
    release_claim.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), coord.wait(store.path()))
        .await
        .expect("detached rebuild never completed after caller cancellation");

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Ready);
    assert_eq!(status.embed_model.as_deref(), Some("model-cancel-safe"));
    assert_eq!(store.stats().unwrap().vector_count, 2);
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(!coord.is_active(store.path()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_inline_rebuild_future_releases_claim_with_rebuild_cancelled() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/a.md", "alpha", "page", json!([]))
        .unwrap();

    let srv = spawn_fake_embed_server("model-inline", FakeServerMode::HangBatch).await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(&srv.base_url).with_model("bulk"));
    let store_task = Arc::clone(&store);
    let task =
        tokio::spawn(async move { rebuild_brain_vectors(store_task, bulk, None, None).await });

    tokio::time::timeout(Duration::from_secs(5), async {
        while !srv.gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("inline rebuild never reached batch gate");
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Rebuilding
    );

    task.abort();
    let _ = task.await;

    tokio::time::timeout(Duration::from_secs(3), async {
        while store.vector_status().unwrap().index_state == IndexModelState::Rebuilding {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropped inline rebuild left DB stuck in Rebuilding");

    let status = store.vector_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(status.stale_reason.as_deref(), Some("rebuild_cancelled"));
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());
    srv.gate_release.notify_waiters();
}

#[test]
fn store_vector_if_ready_serializes_against_second_connection_claim_in_immediate_tx() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("sb.db");
    let store_topup = Arc::new(SuperbrainStore::open(&db_path).unwrap());
    store_topup
        .write_document("pages/seed.md", "seed", "page", json!([]))
        .unwrap();
    store_topup
        .store_vector_with_model("pages/seed.md", &[1.0, 0.0, 0.0, 0.0], "model-1")
        .unwrap();
    store_topup
        .set_vectors_ready("model-1", 4, "bulk", None)
        .unwrap();
    store_topup
        .write_document("pages/race.md", "racing top-up doc", "page", json!([]))
        .unwrap();

    // Second independent SQLite connection opens a BEGIN IMMEDIATE claim
    // transaction and holds the write lock uncommitted while connection 1 calls
    // `store_vector_if_ready_for`. Without BEGIN IMMEDIATE in
    // `store_vector_if_ready_for`, WAL mode lets connection 1 read the stale
    // `Ready` snapshot before commit and then write into a `Rebuilding` index.
    let mut conn_claim = Connection::open(&db_path).unwrap();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let tx = conn_claim
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute(
        "INSERT OR REPLACE INTO brain_vector_meta (key, value) VALUES ('index_state', 'rebuilding')",
        [],
    )
    .unwrap();
    tx.execute(
        "INSERT OR REPLACE INTO brain_vector_meta (key, value) VALUES ('rebuild_owner', 'other-proc')",
        [],
    )
    .unwrap();
    tx.execute(
        "INSERT OR REPLACE INTO brain_vector_meta (key, value) VALUES ('rebuild_heartbeat_ms', ?1)",
        params![now_ms.to_string()],
    )
    .unwrap();

    let store_t = Arc::clone(&store_topup);
    let writer = std::thread::spawn(move || {
        store_t.store_vector_if_ready_for("pages/race.md", &[0.0, 1.0, 0.0, 0.0], "model-1", 4)
    });

    // Give connection 1 plenty of time to enter `store_vector_if_ready_for` and
    // block on BEGIN IMMEDIATE (or, if unguarded, read the pre-commit snapshot).
    std::thread::sleep(Duration::from_millis(80));
    tx.commit().unwrap();

    let wrote = writer.join().unwrap().unwrap();
    assert!(
        !wrote,
        "top-up must not write when a second connection commits a rebuild claim"
    );
    assert_eq!(store_topup.stats().unwrap().vector_count, 1);
    assert_eq!(
        store_topup.vector_status().unwrap().index_state,
        IndexModelState::Rebuilding
    );
}
