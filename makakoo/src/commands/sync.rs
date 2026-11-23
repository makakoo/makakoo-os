//! `makakoo sync` — index the on-disk Brain into SQLite FTS5.

use std::path::PathBuf;
use std::sync::Arc;

use makakoo_core::superbrain::ingest::{IngestEngine, SyncOptions};

use crate::context::CliContext;
use crate::output;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    ctx: &CliContext,
    force: bool,
    embed: bool,
    no_auto_memory: bool,
    embed_limit: usize,
    file: Option<PathBuf>,
) -> anyhow::Result<i32> {
    let store = ctx.store()?;
    let graph = ctx.graph()?;
    let engine = IngestEngine::new(Arc::clone(&store), Arc::clone(&graph), ctx.home());

    if let Some(path) = file {
        let result = engine.sync_file(&path)?;
        println!("indexed {} as {:?}", path.display(), result);
        return Ok(0);
    }

    let opts = SyncOptions {
        force,
        include_auto_memory: !no_auto_memory,
    };
    let mut report = engine.sync(opts)?;

    if embed {
        let embedder = ctx.bulk_embeddings();
        match engine.embed_pending(&embedder, embed_limit).await {
            Ok(n) => report.vectors = n,
            Err(e) => output::print_warn(format!("embedding pass failed (continuing): {e}")),
        }
    }

    println!(
        "sync complete: {} pages, {} journals, {} memories, {} skipped, {} removed, {} errors, {} vectors ({} graph nodes / {} edges)",
        report.pages,
        report.journals,
        report.memories,
        report.skipped,
        report.removed,
        report.errors,
        report.vectors,
        report.graph_nodes,
        report.graph_edges,
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::superbrain::folder_index::IndexModelState;
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_embed_mock(
        expected_alias: &'static str,
        reported_model: &'static str,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let raw = String::from_utf8_lossy(&buf[..n]);
                    let body_str = raw.split("\r\n\r\n").nth(1).unwrap_or("{}");
                    let body: Value = serde_json::from_str(body_str).unwrap_or_default();
                    let req_model = body.get("model").and_then(Value::as_str).unwrap_or("");
                    assert_eq!(req_model, expected_alias);
                    let inputs = body
                        .get("input")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(1);
                    let data: Vec<Value> = (0..inputs)
                        .map(|_| json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                        .collect();
                    let resp_body = json!({
                        "model": reported_model,
                        "data": data,
                    })
                    .to_string();
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

    #[test]
    fn sync_embed_uses_bulk_env_and_records_reported_model() {
        let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();
        let prev_base = std::env::var("AIL_BASE_URL").ok();
        let prev_short = std::env::var("MAKAKOO_EMBED_MODEL").ok();
        let prev_bulk = std::env::var("MAKAKOO_BULK_EMBED_MODEL").ok();

        let dir = tempfile::tempdir().unwrap();
        let pages = dir.path().join("data").join("Brain").join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(
            pages.join("Alpha.md"),
            "# Alpha\n\nLong enough page content to index and embed during sync.",
        )
        .unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let base_url = rt.block_on(spawn_embed_mock("bulk-alias-cli", "Qwen3-Embedding-0.6B"));
        std::env::set_var("AIL_BASE_URL", base_url);
        std::env::set_var("MAKAKOO_EMBED_MODEL", "short-alias-cli");
        std::env::set_var("MAKAKOO_BULK_EMBED_MODEL", "bulk-alias-cli");

        let ctx = CliContext::for_home(dir.path().to_path_buf());
        let rc = rt.block_on(run(&ctx, false, true, true, 16, None)).unwrap();
        assert_eq!(rc, 0);

        let store = ctx.store().unwrap();
        let status = store.vector_status().unwrap();
        assert_eq!(status.index_state, IndexModelState::Ready);
        assert_eq!(status.embed_model.as_deref(), Some("Qwen3-Embedding-0.6B"));
        assert_eq!(status.embed_dim, Some(4));

        let stored_model: String = store
            .conn_arc()
            .lock()
            .unwrap()
            .query_row("SELECT model FROM brain_vectors LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored_model, "Qwen3-Embedding-0.6B");

        match prev_base {
            Some(v) => std::env::set_var("AIL_BASE_URL", v),
            None => std::env::remove_var("AIL_BASE_URL"),
        }
        match prev_short {
            Some(v) => std::env::set_var("MAKAKOO_EMBED_MODEL", v),
            None => std::env::remove_var("MAKAKOO_EMBED_MODEL"),
        }
        match prev_bulk {
            Some(v) => std::env::set_var("MAKAKOO_BULK_EMBED_MODEL", v),
            None => std::env::remove_var("MAKAKOO_BULK_EMBED_MODEL"),
        }
    }
}
