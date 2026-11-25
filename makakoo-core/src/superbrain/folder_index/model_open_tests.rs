//! Guard-focused unit tests for folder index open/alias checks, incremental
//! batch model metadata, sidecar recovery metadata, and legacy DB backfill.

use std::sync::Arc;

use rusqlite::{params, Connection};
use serde_json::Value;
use tempfile::TempDir;

use crate::embeddings::EmbeddingClient;

use super::sqlite_repo::{open_folder_db, DB_FILE_NAME};
use super::*;

fn basis(dim: usize, slot: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; dim];
    v[slot % dim] = 1.0;
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_alias_change_marks_stale_and_unreachable_endpoint_preserves_state() {
    let tmp = TempDir::new().unwrap();
    let cfg_a = FolderIndexConfig::with_dim(32).with_embed_aliases(Some("alias-a".into()), None);
    {
        let idx = FolderIndex::open_with_config(tmp.path(), "alias-folder", cfg_a).unwrap();
        idx.upsert_file_with_model(
            &UpsertFileRequest {
                file_path: "a.md".into(),
                file_version: "v1".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "alice".into(),
                chunks: vec![ChunkInput {
                    anchor: Anchor {
                        kind: AnchorKind::Heading,
                        value: "Top".into(),
                    },
                    char_start: 0,
                    char_end: 18,
                    text: "Alias check sample".into(),
                    vector: basis(32, 0),
                }],
            },
            Some(&UpsertModelMeta {
                model: "model-a".into(),
                embed_dim: 32,
                bulk_alias: "alias-a".into(),
                short_alias: Some("alias-a".into()),
            }),
        )
        .unwrap();
        assert_eq!(
            idx.model_status().unwrap().index_state,
            IndexModelState::Ready
        );
    }

    // Reopen with same alias + unreachable endpoint -> stays Ready.
    let dead_emb =
        Arc::new(EmbeddingClient::with_base_url("http://127.0.0.1:9").with_model("alias-a"));
    let mgr_dead = FolderIndexManager::new(tmp.path())
        .with_embedders(Arc::clone(&dead_emb), Arc::clone(&dead_emb));
    let reopened_ok = mgr_dead.get_or_open("alias-folder", 32).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert_eq!(
        reopened_ok.model_status().unwrap().index_state,
        IndexModelState::Ready
    );
    drop(mgr_dead);

    // Reopen with changed alias -> marked Stale immediately on open.
    let cfg_b = FolderIndexConfig::with_dim(32).with_embed_aliases(Some("alias-b".into()), None);
    let reopened_changed =
        FolderIndex::open_with_config(tmp.path(), "alias-folder", cfg_b).unwrap();
    assert_eq!(
        reopened_changed.model_status().unwrap().index_state,
        IndexModelState::Stale
    );
    assert!(reopened_changed
        .search(&basis(32, 0), 5, &SearchFilter::default())
        .unwrap()
        .is_empty());
}

#[test]
fn legacy_index_without_fts_or_model_backfills_fts_and_marks_stale() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("legacy");
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join(DB_FILE_NAME);

    // Create a pre-C11 sqlite DB: chunks + folder_meta without chunks_fts or embed_model.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE folder_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE file_states (
                 file_path TEXT PRIMARY KEY, file_version TEXT NOT NULL, state TEXT NOT NULL,
                 done INTEGER, total INTEGER, skip_reason TEXT, updated_at INTEGER NOT NULL
             );
             CREATE TABLE chunks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 file_path TEXT NOT NULL, file_version TEXT NOT NULL,
                 anchor_kind TEXT NOT NULL, anchor_value TEXT NOT NULL,
                 char_start INTEGER NOT NULL, char_end INTEGER NOT NULL,
                 text TEXT NOT NULL, source_kind TEXT NOT NULL, trust TEXT NOT NULL,
                 owner TEXT NOT NULL, vector_i8 BLOB NOT NULL
             );
             INSERT INTO folder_meta (key, value) VALUES ('folder_id', 'legacy');
             INSERT INTO folder_meta (key, value) VALUES ('dim', '16');
             INSERT INTO folder_meta (key, value) VALUES ('generation', '1');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks (file_path, file_version, anchor_kind, anchor_value,
                char_start, char_end, text, source_kind, trust, owner, vector_i8)
             VALUES ('old.pdf', 'v1', 'page', '3', 0, 31, 'Legacy quarterly revenue report',
                     'file', 'trusted', 'alice', ?1)",
            params![vec![10u8; 16]],
        )
        .unwrap();
        drop(conn);
    }

    // Verify open_folder_db backfills chunks_fts and FolderIndex::open marks stale.
    let idx = FolderIndex::open(tmp.path(), "legacy", 16).unwrap();
    let status = idx.model_status().unwrap();
    assert_eq!(status.index_state, IndexModelState::Stale);
    assert_eq!(
        status.stale_reason.as_deref(),
        Some("missing_embed_model_metadata")
    );

    // Vector search is blocked while stale, while FTS search returns the backfilled chunk.
    assert!(idx
        .search(&basis(16, 0), 5, &SearchFilter::default())
        .unwrap()
        .is_empty());
    let fts_hits = idx
        .search_fts("quarterly revenue", 5, &SearchFilter::default())
        .unwrap();
    assert_eq!(fts_hits.len(), 1);
    assert_eq!(fts_hits[0].file_path, "old.pdf");
    assert_eq!(fts_hits[0].anchor.value, "3");

    idx.upsert_file_with_model(
        &UpsertFileRequest {
            file_path: "new.md".into(),
            file_version: "v1".into(),
            source_kind: SourceKind::File,
            trust: TrustLevel::Trusted,
            owner: "alice".into(),
            chunks: vec![ChunkInput {
                anchor: Anchor {
                    kind: AnchorKind::Heading,
                    value: "New".into(),
                },
                char_start: 0,
                char_end: 17,
                text: "New indexed text.".into(),
                vector: basis(16, 1),
            }],
        },
        Some(&UpsertModelMeta {
            model: "current-model".into(),
            embed_dim: 16,
            bulk_alias: "bulk-alias".into(),
            short_alias: None,
        }),
    )
    .unwrap();
    let after_upsert = idx.model_status().unwrap();
    assert_eq!(after_upsert.index_state, IndexModelState::Stale);
    assert_eq!(
        after_upsert.stale_reason.as_deref(),
        Some("mixed_model_upsert")
    );
    assert_eq!(after_upsert.embed_model, None);

    // Verify open_folder_db directly sees fts_version='1'.
    let conn = open_folder_db(&db_path).unwrap();
    let fts_ver: String = conn
        .query_row(
            "SELECT value FROM folder_meta WHERE key = 'fts_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts_ver, "1");
}

#[test]
fn incremental_batch_and_usearch_recovery_preserve_model_metadata_in_sidecar() {
    let tmp = TempDir::new().unwrap();
    let cfg = FolderIndexConfig::with_dim(16).with_embed_aliases(Some("test-embed".into()), None);
    let dir_path;
    {
        let idx = FolderIndex::open_with_config(tmp.path(), "inc-folder", cfg.clone()).unwrap();
        dir_path = idx.dir().to_path_buf();
        // The index already knows its real model; an unnamed batch must inherit it,
        // never record the alias as the model.
        {
            let conn = idx.lock_conn().unwrap();
            model_meta::write_ready_model_meta(&conn, "qwen3-real", 16, "test-embed", None)
                .unwrap();
        }
        idx.begin_file_indexing("doc.pdf", "v1", 1).unwrap();
        idx.put_file_chunks_batch(
            "doc.pdf",
            "v1",
            SourceKind::File,
            TrustLevel::Trusted,
            "alice",
            &[ChunkInput {
                anchor: Anchor {
                    kind: AnchorKind::Page,
                    value: "1".into(),
                },
                char_start: 0,
                char_end: 19,
                text: "Incremental batch.".into(),
                vector: basis(16, 0),
            }],
            1,
            1,
        )
        .unwrap();
        idx.finish_file_indexing("doc.pdf", "v1").unwrap();
        idx.flush_and_release().unwrap();

        let status = idx.model_status().unwrap();
        assert_eq!(status.index_state, IndexModelState::Ready);
        assert_eq!(status.embed_model.as_deref(), Some("qwen3-real"));
        assert_eq!(status.embed_dim, Some(16));
    }

    // Remove vectors.usearch so open() rebuilds it from SQLite and writes sidecar metadata.
    std::fs::remove_file(dir_path.join("vectors.usearch")).unwrap();
    let reopened = FolderIndex::open_with_config(tmp.path(), "inc-folder", cfg).unwrap();
    let sidecar_raw = std::fs::read_to_string(dir_path.join("vectors.gen")).unwrap();
    let sidecar: Value = serde_json::from_str(&sidecar_raw).unwrap();
    assert_eq!(sidecar["embed_model"], "qwen3-real");
    assert_eq!(sidecar["embed_dim"], 16);
    assert_eq!(
        reopened
            .search(&basis(16, 0), 5, &SearchFilter::default())
            .unwrap()
            .len(),
        1
    );
}

fn unnamed_chunk(text: &str, slot: usize) -> ChunkInput {
    ChunkInput {
        anchor: Anchor {
            kind: AnchorKind::Page,
            value: "1".into(),
        },
        char_start: 0,
        char_end: text.len(),
        text: text.into(),
        vector: basis(16, slot),
    }
}

#[test]
fn unnamed_rows_on_a_fresh_index_never_claim_the_alias_as_model() {
    let tmp = TempDir::new().unwrap();
    let cfg = FolderIndexConfig::with_dim(16).with_embed_aliases(Some("test-embed".into()), None);
    let idx = FolderIndex::open_with_config(tmp.path(), "fresh", cfg).unwrap();
    for (file, slot) in [("a.pdf", 0), ("b.pdf", 1)] {
        idx.begin_file_indexing(file, "v1", 1).unwrap();
        idx.put_file_chunks_batch(
            file,
            "v1",
            SourceKind::File,
            TrustLevel::Trusted,
            "alice",
            &[unnamed_chunk("Unnamed rows.", slot)],
            1,
            1,
        )
        .unwrap();
        idx.finish_file_indexing(file, "v1").unwrap();
        let st = idx.model_status().unwrap();
        // Model left unknown (a probe names it later); a second unnamed batch must
        // not turn this into `mixed_model_upsert`, and vector search keeps working.
        assert_eq!(st.embed_model, None, "{file}");
        assert_eq!(st.embed_dim, Some(16));
        assert_eq!(st.index_state, IndexModelState::Ready, "{file}");
        assert_eq!(st.stale_reason, None, "{file}");
        assert!(
            !idx.search(&basis(16, slot), 5, &SearchFilter::default())
                .unwrap()
                .is_empty(),
            "{file}: vector search must keep working"
        );
    }
}
