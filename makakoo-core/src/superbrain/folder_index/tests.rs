//! Unit tests (one per guard) for `superbrain::folder_index`.

use std::collections::BTreeSet;
use std::time::Duration;

use tempfile::TempDir;

use super::*;

pub(crate) fn seeded_vec(dim: usize, seed: u64) -> Vec<f32> {
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..dim)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let bits = ((x >> 40) as i32) - (1 << 23);
            (bits as f32) / ((1 << 23) as f32)
        })
        .collect()
}

fn unit_basis_vec(dim: usize, hot_idx: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; dim];
    v[hot_idx % dim] = 1.0;
    v
}

fn make_chunk(anchor: Anchor, start: usize, text: &str, vec: Vec<f32>) -> ChunkInput {
    ChunkInput {
        anchor,
        char_start: start,
        char_end: start + text.chars().count(),
        text: text.to_string(),
        vector: vec,
    }
}

fn test_model_meta(embed_dim: usize) -> UpsertModelMeta {
    UpsertModelMeta {
        model: "test-model".into(),
        embed_dim,
        bulk_alias: crate::embeddings::resolve_bulk_model(),
        short_alias: Some(crate::embeddings::resolve_short_model()),
    }
}

#[test]
fn rejects_path_traversal_folder_ids() {
    let tmp = TempDir::new().unwrap();
    for bad in [
        "",
        "../escape",
        "a/b",
        "a\\b",
        ".hidden",
        "foo..bar",
        "has space",
    ] {
        let err = FolderIndex::open(tmp.path(), bad, 64).err().unwrap();
        assert!(matches!(err, crate::error::MakakooError::InvalidInput(_)));
    }
}

#[test]
fn rejects_invalid_dimensions_on_open() {
    let tmp = TempDir::new().unwrap();
    assert!(FolderIndex::open(tmp.path(), "ok-folder", 0).is_err());
    assert!(FolderIndex::open(tmp.path(), "ok-folder", MAX_VECTOR_DIM + 1).is_err());
}

#[test]
fn places_survive_add_replace_reopen_mmap_and_search_with_exact_10_keys() {
    let tmp = TempDir::new().unwrap();
    let idx = FolderIndex::open(tmp.path(), "contracts", 512).unwrap();

    // Feed 1024-dim embedder vectors to verify truncation to 512 + i8 quantization.
    let v1 = unit_basis_vec(1024, 7);
    let v2 = unit_basis_vec(1024, 19);

    let out = idx
        .upsert_file_with_model(
            &UpsertFileRequest {
                file_path: "legal/msa.pdf".into(),
                file_version: "sha256:v1".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "owner-a".into(),
                chunks: vec![make_chunk(
                    Anchor {
                        kind: AnchorKind::Page,
                        value: "4".into(),
                    },
                    120,
                    "Net 30 payment terms apply.",
                    v1.clone(),
                )],
            },
            Some(&test_model_meta(1024)),
        )
        .unwrap();
    assert!(!out.replaced && !out.skipped_unchanged && out.chunks_written == 1);

    // Idempotent upsert with identical version is a no-op.
    let gen_before = idx.size_report().unwrap().generation;
    let idem = idx
        .upsert_file_with_model(
            &UpsertFileRequest {
                file_path: "legal/msa.pdf".into(),
                file_version: "sha256:v1".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "owner-a".into(),
                chunks: vec![make_chunk(
                    Anchor {
                        kind: AnchorKind::Page,
                        value: "99".into(),
                    },
                    0,
                    "Should not overwrite.",
                    v2.clone(),
                )],
            },
            Some(&test_model_meta(1024)),
        )
        .unwrap();
    assert!(idem.skipped_unchanged);
    assert_eq!(idx.size_report().unwrap().generation, gen_before);

    // Replace with new version removes old chunk and updates place.
    let rep = idx
        .upsert_file_with_model(
            &UpsertFileRequest {
                file_path: "legal/msa.pdf".into(),
                file_version: "sha256:v2".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "owner-a".into(),
                chunks: vec![make_chunk(
                    Anchor {
                        kind: AnchorKind::Page,
                        value: "6".into(),
                    },
                    240,
                    "Net 45 payment terms apply.",
                    v2.clone(),
                )],
            },
            Some(&test_model_meta(1024)),
        )
        .unwrap();
    assert!(rep.replaced && rep.chunks_written == 1);

    idx.flush_and_release().unwrap();
    drop(idx);

    // Reopen from disk — serves searches via mmap view without rebuild.
    let reopened = FolderIndex::open(tmp.path(), "contracts", 512).unwrap();
    let hits = reopened.search(&v2, 5, &SearchFilter::default()).unwrap();
    assert_eq!(hits.len(), 1);
    let p = &hits[0];
    assert_eq!(p.folder_id, "contracts");
    assert_eq!(p.file_path, "legal/msa.pdf");
    assert_eq!(p.file_version, "sha256:v2");
    assert_eq!(p.anchor.kind, AnchorKind::Page);
    assert_eq!(p.anchor.value, "6");
    assert_eq!(p.char_start, 240);
    assert_eq!(
        p.char_end,
        240 + "Net 45 payment terms apply.".chars().count()
    );
    assert_eq!(p.text, "Net 45 payment terms apply.");
    assert!(
        p.score > 0.99,
        "cosine score expected ~1.0, got {}",
        p.score
    );
    assert_eq!(p.source_kind, SourceKind::File);
    assert_eq!(p.trust, TrustLevel::Trusted);

    // Exact 10 passage keys (`owner` is filter-only).
    let val = serde_json::to_value(p).unwrap();
    let keys: BTreeSet<String> = val.as_object().unwrap().keys().cloned().collect();
    let expected: BTreeSet<String> = [
        "folder_id",
        "file_path",
        "file_version",
        "anchor",
        "char_start",
        "char_end",
        "text",
        "score",
        "source_kind",
        "trust",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(keys, expected);

    let sizes = reopened.size_report().unwrap();
    assert!(sizes.vector_index_bytes > 0);
    assert!(sizes.sqlite_bytes > 0);
    assert_eq!(sizes.chunk_count, 1);
}

#[test]
fn provenance_filters_exclude_non_matching_chunks() {
    let tmp = TempDir::new().unwrap();
    let idx = FolderIndex::open(tmp.path(), "mixed", 64).unwrap();
    let q = seeded_vec(64, 100);

    let entries = [
        ("doc.pdf", SourceKind::File, TrustLevel::Trusted, "alice"),
        ("note.md", SourceKind::Memory, TrustLevel::Trusted, "alice"),
        (
            "laptop.md",
            SourceKind::LaptopNote,
            TrustLevel::Untrusted,
            "alice",
        ),
        ("other.pdf", SourceKind::File, TrustLevel::Untrusted, "bob"),
    ];
    for (i, (path, sk, trust, owner)) in entries.into_iter().enumerate() {
        idx.upsert_file_with_model(
            &UpsertFileRequest {
                file_path: path.into(),
                file_version: format!("v{i}"),
                source_kind: sk,
                trust,
                owner: owner.into(),
                chunks: vec![make_chunk(
                    Anchor {
                        kind: AnchorKind::Heading,
                        value: "Intro".into(),
                    },
                    0,
                    path,
                    q.clone(),
                )],
            },
            Some(&test_model_meta(64)),
        )
        .unwrap();
    }
    idx.flush().unwrap();

    let laptop = idx
        .search(
            &q,
            10,
            &SearchFilter {
                source_kind: Some(SourceKind::LaptopNote),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(laptop.len(), 1);
    assert_eq!(laptop[0].file_path, "laptop.md");

    let trusted = idx
        .search(
            &q,
            10,
            &SearchFilter {
                trust: Some(TrustLevel::Trusted),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(trusted.len(), 2);
    assert!(trusted.iter().all(|p| p.trust == TrustLevel::Trusted));

    let bob = idx
        .search(
            &q,
            10,
            &SearchFilter {
                owner: Some("bob".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(bob.len(), 1);
    assert_eq!(bob[0].file_path, "other.pdf");

    let combined = idx
        .search(
            &q,
            10,
            &SearchFilter {
                source_kind: Some(SourceKind::File),
                trust: Some(TrustLevel::Trusted),
                owner: Some("alice".into()),
            },
        )
        .unwrap();
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].file_path, "doc.pdf");
}

#[test]
fn file_states_cover_indexing_indexed_skipped_and_remove() {
    let tmp = TempDir::new().unwrap();
    let idx = FolderIndex::open(tmp.path(), "states", 32).unwrap();
    let v = unit_basis_vec(32, 1);

    idx.begin_file_indexing("big.pdf", "v1", 40).unwrap();
    idx.put_file_chunks_batch(
        "big.pdf",
        "v1",
        SourceKind::File,
        TrustLevel::Trusted,
        "alice",
        &[make_chunk(
            Anchor {
                kind: AnchorKind::Page,
                value: "1".into(),
            },
            0,
            "Page one",
            v.clone(),
        )],
        Some(&UpsertModelMeta {
            model: "test-model".into(),
            embed_dim: 32,
            bulk_alias: "test-model".into(),
            short_alias: None,
        }),
        14,
        40,
    )
    .unwrap();
    let mid_json = serde_json::to_value(idx.file_state("big.pdf").unwrap().unwrap()).unwrap();
    assert_eq!(mid_json["state"], "indexing");
    assert_eq!(mid_json["done"], 14);
    assert_eq!(mid_json["total"], 40);

    idx.finish_file_indexing("big.pdf", "v1").unwrap();
    assert_eq!(
        idx.file_state("big.pdf").unwrap().unwrap().status,
        FileStatus::Indexed
    );

    // Skipped clears stale chunks and surfaces explicit reason.
    idx.mark_file_skipped("big.pdf", "v2", SkipReason::Password)
        .unwrap();
    assert!(idx
        .search(&v, 5, &SearchFilter::default())
        .unwrap()
        .is_empty());
    let skipped_json = serde_json::to_value(idx.file_state("big.pdf").unwrap().unwrap()).unwrap();
    assert_eq!(skipped_json["state"], "skipped");
    assert_eq!(skipped_json["reason"], SkipReason::Password.as_str());

    idx.mark_file_skipped("huge.pdf", "v1", SkipReason::TooBig)
        .unwrap();
    idx.mark_file_skipped("bad.pdf", "v1", SkipReason::Unreadable)
        .unwrap();
    assert_eq!(idx.file_states().unwrap().len(), 3);

    assert!(idx.remove_file("big.pdf").unwrap());
    assert!(idx.file_state("big.pdf").unwrap().is_none());
    assert!(!idx.remove_file("nonexistent.pdf").unwrap());
}

#[test]
fn skip_reason_wire_values_are_pinned_and_roundtrip() {
    assert_eq!(SkipReason::ALL.len(), 3);
    assert_eq!(SkipReason::ALL[0], SkipReason::Password);
    assert_eq!(SkipReason::ALL[1], SkipReason::TooBig);
    assert_eq!(SkipReason::ALL[2], SkipReason::Unreadable);

    assert_eq!(SkipReason::Password.as_str(), "password");
    assert_eq!(SkipReason::TooBig.as_str(), "too_big");
    assert_eq!(SkipReason::Unreadable.as_str(), "unreadable");

    for reason in SkipReason::ALL {
        let wire = reason.as_str();
        assert_eq!(SkipReason::parse(wire).unwrap(), reason);
        let serialized = serde_json::to_value(reason).unwrap();
        assert_eq!(serialized, serde_json::json!(wire));
        assert_eq!(
            serde_json::from_value::<SkipReason>(serialized).unwrap(),
            reason
        );
    }

    assert!(SkipReason::parse("other").is_err());
    assert!(serde_json::from_value::<SkipReason>(serde_json::json!("other")).is_err());
}

#[test]
fn missing_or_stale_usearch_file_rebuilds_from_sqlite_on_open() {
    let tmp = TempDir::new().unwrap();
    let v = unit_basis_vec(64, 5);
    let dir_path;
    {
        let idx = FolderIndex::open(tmp.path(), "recover", 64).unwrap();
        dir_path = idx.dir().to_path_buf();
        idx.upsert_file_with_model(
            &UpsertFileRequest {
                file_path: "a.pdf".into(),
                file_version: "v1".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "alice".into(),
                chunks: vec![make_chunk(
                    Anchor {
                        kind: AnchorKind::Page,
                        value: "1".into(),
                    },
                    0,
                    "Durable chunk in SQLite",
                    v.clone(),
                )],
            },
            Some(&test_model_meta(64)),
        )
        .unwrap();
        idx.flush_and_release().unwrap();
    }

    // Missing vectors.usearch -> rebuilds from SQLite without re-embedding.
    let usearch_path = dir_path.join("vectors.usearch");
    assert!(usearch_path.is_file());
    std::fs::remove_file(&usearch_path).unwrap();
    {
        let reopened = FolderIndex::open(tmp.path(), "recover", 64).unwrap();
        assert!(usearch_path.is_file());
        let hits = reopened.search(&v, 5, &SearchFilter::default()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "Durable chunk in SQLite");
    }

    // Stale generation in vectors.gen -> rebuilds from SQLite.
    std::fs::write(
        dir_path.join("vectors.gen"),
        r#"{"generation":0,"dim":64,"count":0}"#,
    )
    .unwrap();
    {
        let reopened = FolderIndex::open(tmp.path(), "recover", 64).unwrap();
        assert_eq!(
            reopened.vector_disk_generation(),
            Some(reopened.size_report().unwrap().generation)
        );
        let hits = reopened.search(&v, 5, &SearchFilter::default()).unwrap();
        assert_eq!(hits.len(), 1);
    }
}

#[test]
fn batched_flush_avoids_per_file_rewrites_and_uses_last_saved_view_during_batch() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = FolderIndexConfig::with_dim(32);
    cfg.flush_file_batch = 3;
    cfg.flush_interval = Duration::from_secs(3600);

    let idx = FolderIndex::open_with_config(tmp.path(), "batched", cfg).unwrap();
    idx.begin_indexing_run().unwrap();

    for i in 0..2 {
        idx.upsert_file_with_model(
            &UpsertFileRequest {
                file_path: format!("f{i}.txt"),
                file_version: "v1".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "alice".into(),
                chunks: vec![make_chunk(
                    Anchor {
                        kind: AnchorKind::Page,
                        value: "1".into(),
                    },
                    0,
                    &format!("file {i}"),
                    unit_basis_vec(32, i),
                )],
            },
            Some(&test_model_meta(32)),
        )
        .unwrap();
    }
    assert_eq!(idx.vector_disk_generation(), None);
    assert!(idx
        .search(&unit_basis_vec(32, 0), 5, &SearchFilter::default())
        .unwrap()
        .is_empty());

    // 3rd file triggers atomic batch flush.
    idx.upsert_file_with_model(
        &UpsertFileRequest {
            file_path: "f2.txt".into(),
            file_version: "v1".into(),
            source_kind: SourceKind::File,
            trust: TrustLevel::Trusted,
            owner: "alice".into(),
            chunks: vec![make_chunk(
                Anchor {
                    kind: AnchorKind::Page,
                    value: "1".into(),
                },
                0,
                "file 2",
                unit_basis_vec(32, 2),
            )],
        },
        Some(&test_model_meta(32)),
    )
    .unwrap();
    assert_eq!(idx.vector_disk_generation(), Some(3));
    assert_eq!(
        idx.search(&unit_basis_vec(32, 2), 1, &SearchFilter::default())
            .unwrap()
            .len(),
        1
    );

    idx.finish_indexing_run().unwrap();
}
