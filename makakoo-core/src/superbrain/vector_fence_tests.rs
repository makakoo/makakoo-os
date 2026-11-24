//! Guard tests for owner-fenced Brain vector rebuild claims and per-batch /
//! pre-finish heartbeat refreshes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::embeddings::EmbeddingClient;
use crate::error::MakakooError;
use crate::superbrain::folder_index::IndexModelState;
use crate::superbrain::store::SuperbrainStore;
use crate::superbrain::vector_claim::{META_REBUILD_HEARTBEAT, META_REBUILD_OWNER};
use crate::superbrain::vector_model::rebuild_brain_vectors;
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

fn set_heartbeat(store: &SuperbrainStore, value: &str) {
    store
        .conn_arc()
        .lock()
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO brain_vector_meta (key, value) VALUES (?1, ?2)",
            params![META_REBUILD_HEARTBEAT, value],
        )
        .unwrap();
}

fn vector_rows(store: &SuperbrainStore) -> Vec<(String, String)> {
    let conn = store.conn_arc();
    let conn = conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT d.path, COALESCE(v.model, '')
             FROM brain_vectors v
             JOIN brain_docs d ON v.doc_id = d.id
             ORDER BY d.path",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn fenced_owner_write_heartbeat_and_finish_fail_with_claim_lost_and_change_nothing() {
    let dir = TempDir::new().unwrap();
    let store = SuperbrainStore::open(&dir.path().join("sb.db")).unwrap();
    store
        .write_document("pages/0.md", "alpha", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/1.md", "beta", "page", json!([]))
        .unwrap();

    // Owner A claims the rebuild.
    let owner_a = store
        .try_mark_vectors_rebuilding("owner-a")
        .unwrap()
        .expect("owner A must claim");
    assert_eq!(owner_a.len(), 32);
    assert_eq!(
        read_meta(&store, META_REBUILD_OWNER).as_deref(),
        Some(owner_a.as_str())
    );

    // Owner A's lease expires; owner B takes over with a distinct token.
    set_heartbeat(&store, "0");
    let owner_b = store
        .try_mark_vectors_rebuilding("owner-b")
        .unwrap()
        .expect("owner B must take over expired lease");
    assert_ne!(owner_a, owner_b);

    // Owner B writes doc 0.
    store
        .store_vector_for_rebuild(&owner_b, "pages/0.md", &[0.0, 1.0, 0.0, 0.0], "model-b")
        .unwrap();
    let hb_b = read_meta(&store, META_REBUILD_HEARTBEAT).unwrap();

    // Stalled owner A wakes up while B holds the claim: every fenced op fails
    // with RebuildClaimLost (or returns false) and mutates nothing.
    assert!(matches!(
        store.touch_rebuild_heartbeat(&owner_a),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert_eq!(read_meta(&store, META_REBUILD_HEARTBEAT), Some(hb_b));
    assert_eq!(
        read_meta(&store, META_REBUILD_OWNER).as_deref(),
        Some(owner_b.as_str())
    );

    assert!(matches!(
        store.store_vector_for_rebuild(&owner_a, "pages/0.md", &[1.0, 0.0, 0.0, 0.0], "model-a"),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert!(matches!(
        store.store_vector_for_rebuild(&owner_a, "pages/1.md", &[1.0, 0.0, 0.0, 0.0], "model-a"),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert_eq!(
        vector_rows(&store),
        vec![("pages/0.md".to_string(), "model-b".to_string())]
    );

    assert!(matches!(
        store.finish_rebuild(&owner_a, "model-a", 4, "bulk-a", None),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert!(!store
        .mark_vectors_stale_for_owner(&owner_a, "stale-from-a")
        .unwrap());
    let mid_status = store.vector_status().unwrap();
    assert_eq!(mid_status.index_state, IndexModelState::Rebuilding);
    assert_eq!(mid_status.stale_reason.as_deref(), Some("owner-b"));

    // Owner B finishes normally and clears claim metadata.
    store.touch_rebuild_heartbeat(&owner_b).unwrap();
    store
        .store_vector_for_rebuild(&owner_b, "pages/1.md", &[0.0, 1.0, 0.0, 0.0], "model-b")
        .unwrap();
    store
        .finish_rebuild(&owner_b, "model-b", 4, "bulk-b", None)
        .unwrap();

    let ready_status = store.vector_status().unwrap();
    assert_eq!(ready_status.index_state, IndexModelState::Ready);
    assert_eq!(ready_status.embed_model.as_deref(), Some("model-b"));
    assert_eq!(ready_status.embed_dim, Some(4));
    assert!(read_meta(&store, META_REBUILD_OWNER).is_none());
    assert!(read_meta(&store, META_REBUILD_HEARTBEAT).is_none());

    // Owner A waking up AFTER B finished still gets RebuildClaimLost and
    // changes nothing.
    assert!(matches!(
        store.touch_rebuild_heartbeat(&owner_a),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert!(matches!(
        store.store_vector_for_rebuild(&owner_a, "pages/0.md", &[1.0, 0.0, 0.0, 0.0], "model-a"),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert!(matches!(
        store.finish_rebuild(&owner_a, "model-a", 4, "bulk-a", None),
        Err(MakakooError::RebuildClaimLost)
    ));
    assert!(!store
        .mark_vectors_stale_for_owner(&owner_a, "late-stale-from-a")
        .unwrap());
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );
    assert_eq!(
        vector_rows(&store),
        vec![
            ("pages/0.md".to_string(), "model-b".to_string()),
            ("pages/1.md".to_string(), "model-b".to_string()),
        ]
    );
}

#[derive(Clone)]
struct GatedEmbedResponder {
    model: String,
    dim: usize,
    gate_batches: bool,
    gate_entered: Arc<AtomicBool>,
    gate_release: Arc<AtomicBool>,
}

impl Respond for GatedEmbedResponder {
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
            let deadline = Instant::now() + Duration::from_secs(30);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_rebuild_loop_aborts_with_claim_lost_and_preserves_new_owner() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    store
        .write_document("pages/0.md", "alpha", "page", json!([]))
        .unwrap();
    store
        .write_document("pages/1.md", "beta", "page", json!([]))
        .unwrap();

    let gate_entered = Arc::new(AtomicBool::new(false));
    let gate_release = Arc::new(AtomicBool::new(false));
    let server_a = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(GatedEmbedResponder {
            model: "model-a".to_string(),
            dim: 4,
            gate_batches: true,
            gate_entered: Arc::clone(&gate_entered),
            gate_release: Arc::clone(&gate_release),
        })
        .mount(&server_a)
        .await;
    let bulk_a = Arc::new(EmbeddingClient::with_base_url(server_a.uri()).with_model("bulk-a"));

    let server_b = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(GatedEmbedResponder {
            model: "model-b".to_string(),
            dim: 8,
            gate_batches: false,
            gate_entered: Arc::new(AtomicBool::new(false)),
            gate_release: Arc::new(AtomicBool::new(true)),
        })
        .mount(&server_b)
        .await;
    let bulk_b = Arc::new(EmbeddingClient::with_base_url(server_b.uri()).with_model("bulk-b"));

    // Owner A starts rebuilding and stalls inside the batch embed call.
    let owner_a = store
        .try_mark_vectors_rebuilding("owner-a")
        .unwrap()
        .unwrap();
    let store_a = Arc::clone(&store);
    let task_a = tokio::spawn(async move {
        rebuild_brain_vectors_with_owner(store_a, &owner_a, bulk_a, None, None).await
    });

    tokio::time::timeout(Duration::from_secs(10), async {
        while !gate_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owner A never reached gated batch embed");

    // Lease expires while A is stalled; owner B takes over and finishes Ready(model-b).
    set_heartbeat(&store, "0");
    let written_b = rebuild_brain_vectors(Arc::clone(&store), bulk_b, None, None)
        .await
        .unwrap();
    assert_eq!(written_b, 2);
    let status_b = store.vector_status().unwrap();
    assert_eq!(status_b.index_state, IndexModelState::Ready);
    assert_eq!(status_b.embed_model.as_deref(), Some("model-b"));
    assert_eq!(status_b.embed_dim, Some(8));

    // Unblock A: it must abort with RebuildClaimLost and preserve B's state & vectors.
    gate_release.store(true, Ordering::SeqCst);
    let res_a = tokio::time::timeout(Duration::from_secs(10), task_a)
        .await
        .expect("owner A did not finish after gate release")
        .unwrap();
    assert!(
        matches!(res_a, Err(MakakooError::RebuildClaimLost)),
        "expected RebuildClaimLost, got {res_a:?}"
    );

    let status_after = store.vector_status().unwrap();
    assert_eq!(status_after.index_state, IndexModelState::Ready);
    assert_eq!(status_after.embed_model.as_deref(), Some("model-b"));
    assert_eq!(status_after.embed_dim, Some(8));
    assert_eq!(
        vector_rows(&store),
        vec![
            ("pages/0.md".to_string(), "model-b".to_string()),
            ("pages/1.md".to_string(), "model-b".to_string()),
        ]
    );
}

#[derive(Clone)]
struct BackdatingBatchResponder {
    conn: Arc<Mutex<Connection>>,
    observed_batch_hbs: Arc<Mutex<Vec<i64>>>,
    backdated_to: Arc<Mutex<Vec<i64>>>,
}

impl Respond for BackdatingBatchResponder {
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
        if !is_probe {
            let conn = self.conn.lock().unwrap();
            let current: i64 = conn
                .query_row(
                    "SELECT value FROM brain_vector_meta WHERE key = ?1",
                    params![META_REBUILD_HEARTBEAT],
                    |r| r.get::<_, String>(0),
                )
                .unwrap()
                .parse()
                .unwrap();
            self.observed_batch_hbs.lock().unwrap().push(current);
            // Backdate by 40s (still well inside 120s lease) so the next
            // touch_rebuild_heartbeat observably advances the timestamp.
            let back = current - 40_000;
            conn.execute(
                "INSERT OR REPLACE INTO brain_vector_meta (key, value) VALUES (?1, ?2)",
                params![META_REBUILD_HEARTBEAT, back.to_string()],
            )
            .unwrap();
            self.backdated_to.lock().unwrap().push(back);
        }
        let data: Vec<Value> = (0..inputs)
            .map(|_| json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "model": "model-hb", "data": data }))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_refreshes_heartbeat_per_batch_and_before_finish() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
    // 35 docs > REBUILD_BATCH_SIZE (32) forces 2 batches (32 + 3).
    for i in 0..35 {
        store
            .write_document(
                &format!("pages/doc-{i:02}.md"),
                &format!("Document body {i}"),
                "page",
                json!([]),
            )
            .unwrap();
    }

    // Record every heartbeat write paired with the vector count at that moment.
    {
        let conn = store.conn_arc();
        let conn = conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE hb_audit (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 val INTEGER NOT NULL,
                 vec_count INTEGER NOT NULL
             );
             CREATE TEMP TRIGGER trg_hb_audit
             AFTER INSERT ON brain_vector_meta
             WHEN NEW.key = 'rebuild_heartbeat_ms'
             BEGIN
                 INSERT INTO hb_audit (val, vec_count)
                 VALUES (CAST(NEW.value AS INTEGER), (SELECT COUNT(*) FROM brain_vectors));
             END;",
        )
        .unwrap();
    }

    let observed_batch_hbs = Arc::new(Mutex::new(Vec::new()));
    let backdated_to = Arc::new(Mutex::new(Vec::new()));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(BackdatingBatchResponder {
            conn: store.conn_arc(),
            observed_batch_hbs: Arc::clone(&observed_batch_hbs),
            backdated_to: Arc::clone(&backdated_to),
        })
        .mount(&server)
        .await;
    let bulk = Arc::new(EmbeddingClient::with_base_url(server.uri()).with_model("bulk-hb"));

    let owner = store
        .try_mark_vectors_rebuilding("hb-test")
        .unwrap()
        .unwrap();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let initial_backdated = now_ms - 50_000;
    set_heartbeat(&store, &initial_backdated.to_string());

    let written = rebuild_brain_vectors_with_owner(Arc::clone(&store), &owner, bulk, None, None)
        .await
        .unwrap();
    assert_eq!(written, 35);
    assert_eq!(
        store.vector_status().unwrap().index_state,
        IndexModelState::Ready
    );

    let hbs = observed_batch_hbs.lock().unwrap().clone();
    let backs = backdated_to.lock().unwrap().clone();
    assert_eq!(hbs.len(), 2, "expected 2 batch requests for 35 docs");
    assert!(
        hbs[0] > initial_backdated,
        "heartbeat must advance before batch 1"
    );
    assert!(
        hbs[1] > backs[0],
        "heartbeat must advance between batch 1 and batch 2"
    );

    let audit_counts: Vec<i64> = {
        let conn = store.conn_arc();
        let conn = conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT vec_count FROM hb_audit ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(
        audit_counts.contains(&32),
        "expected heartbeat touch after batch 1 (vec_count=32); audit={audit_counts:?}"
    );
    assert!(
        audit_counts.contains(&35),
        "expected heartbeat touch after final batch and before finish_rebuild (vec_count=35); audit={audit_counts:?}"
    );
}
