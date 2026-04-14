//! `makakoo search` — full-text search against the Brain.

use crate::context::CliContext;
use crate::output;

pub async fn run(ctx: &CliContext, query: &str, limit: usize) -> anyhow::Result<i32> {
    let store = ctx.store()?;
    let hits = store.search(query, limit)?;
    output::print_search_hits(&hits);
    Ok(0)
}
