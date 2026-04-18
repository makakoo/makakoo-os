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
        let embedder = ctx.embeddings();
        match engine.embed_pending(&embedder, embed_limit).await {
            Ok(n) => report.vectors = n,
            Err(e) => output::print_warn(&format!(
                "embedding pass failed (continuing): {e}"
            )),
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
