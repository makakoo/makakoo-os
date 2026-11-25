//! Ignored 200,000-chunk benchmark measuring vector-index size, SQLite size,
//! peak RSS while indexing, and p95 search latency over an mmap view.

use std::time::Instant;

use tempfile::TempDir;

use super::tests::seeded_vec;
use super::*;

#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    if rc != 0 {
        return 0;
    }
    #[cfg(target_os = "macos")]
    {
        usage.ru_maxrss as u64
    }
    #[cfg(not(target_os = "macos"))]
    {
        (usage.ru_maxrss as u64).saturating_mul(1024)
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> u64 {
    0
}

#[test]
#[ignore = "heavy 200k-chunk benchmark; run manually with --release -- --ignored --nocapture"]
fn bench_200k_chunks_size_and_p95() {
    const TOTAL_FILES: usize = 4_000;
    const CHUNKS_PER_FILE: usize = 50;
    const TOTAL_CHUNKS: usize = TOTAL_FILES * CHUNKS_PER_FILE;
    const DIM: usize = 512;
    const QUERIES: usize = 100;

    let tmp = TempDir::new().unwrap();
    let t_start = Instant::now();
    let idx = FolderIndex::open(tmp.path(), "bench-200k", DIM).unwrap();
    let model_meta = UpsertModelMeta {
        model: "bench-model".into(),
        embed_dim: DIM,
        bulk_alias: crate::embeddings::resolve_bulk_model(),
        short_alias: Some(crate::embeddings::resolve_short_model()),
    };
    idx.begin_indexing_run().unwrap();

    for f in 0..TOTAL_FILES {
        let mut chunks = Vec::with_capacity(CHUNKS_PER_FILE);
        for c in 0..CHUNKS_PER_FILE {
            let gid = (f * CHUNKS_PER_FILE + c) as u64;
            chunks.push(ChunkInput {
                anchor: Anchor {
                    kind: AnchorKind::Page,
                    value: (c + 1).to_string(),
                },
                char_start: 0,
                char_end: 32,
                text: format!("Synthetic passage chunk {gid:08}"),
                vector: seeded_vec(DIM, gid + 1),
            });
        }
        idx.upsert_file_with_model(
            &UpsertFileRequest {
                file_path: format!("docs/file_{f:04}.pdf"),
                file_version: "v1".into(),
                source_kind: SourceKind::File,
                trust: TrustLevel::Trusted,
                owner: "bench".into(),
                chunks,
            },
            Some(&model_meta),
        )
        .unwrap();
    }

    let rss_indexing_peak = peak_rss_bytes();
    idx.finish_indexing_run().unwrap();
    let index_elapsed = t_start.elapsed();
    let sizes = idx.size_report().unwrap();
    drop(idx);

    let t_reopen = Instant::now();
    let reopened = FolderIndex::open(tmp.path(), "bench-200k", DIM).unwrap();
    let reopen_elapsed = t_reopen.elapsed();

    let mut latencies_us = Vec::with_capacity(QUERIES);
    for q in 0..QUERIES {
        let qv = seeded_vec(DIM, (q as u64) * 1999 + 7);
        let t0 = Instant::now();
        let hits = reopened.search(&qv, 10, &SearchFilter::default()).unwrap();
        latencies_us.push(t0.elapsed().as_micros());
        assert_eq!(hits.len(), 10);
    }
    latencies_us.sort_unstable();
    let p50_ms = latencies_us[QUERIES / 2] as f64 / 1000.0;
    let p95_ms = latencies_us[(QUERIES * 95) / 100 - 1] as f64 / 1000.0;
    let p99_ms = latencies_us[QUERIES - 1] as f64 / 1000.0;
    let rss_final_peak = peak_rss_bytes();

    println!("=== Folder Index 200k Chunk Benchmark ===");
    println!("chunks:             {}", sizes.chunk_count);
    println!("index_time_s:       {:.2}", index_elapsed.as_secs_f64());
    println!(
        "reopen_mmap_ms:     {:.2}",
        reopen_elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "vector_index_mb:    {:.2} ({} bytes)",
        sizes.vector_index_bytes as f64 / (1024.0 * 1024.0),
        sizes.vector_index_bytes
    );
    println!(
        "sqlite_mb:          {:.2} ({} bytes)",
        sizes.sqlite_bytes as f64 / (1024.0 * 1024.0),
        sizes.sqlite_bytes
    );
    println!(
        "peak_rss_indexing:  {:.2} MB ({} bytes)",
        rss_indexing_peak as f64 / (1024.0 * 1024.0),
        rss_indexing_peak
    );
    println!(
        "peak_rss_final:     {:.2} MB ({} bytes)",
        rss_final_peak as f64 / (1024.0 * 1024.0),
        rss_final_peak
    );
    println!("search_p50_ms:      {p50_ms:.2}");
    println!("search_p95_ms:      {p95_ms:.2}");
    println!("search_p99_ms:      {p99_ms:.2}");

    assert_eq!(sizes.chunk_count, TOTAL_CHUNKS as u64);
    assert!(
        p95_ms <= 300.0,
        "p95 search latency {p95_ms:.2} ms exceeded 300 ms"
    );
    if rss_indexing_peak > 0 {
        assert!(
            rss_indexing_peak <= 512 * 1024 * 1024,
            "peak RSS while indexing ({:.2} MB) exceeded 512 MB",
            rss_indexing_peak as f64 / (1024.0 * 1024.0)
        );
    }
}
