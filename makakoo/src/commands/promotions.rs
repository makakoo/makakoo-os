//! `makakoo promotions` — memory promotion candidates.

use crate::context::CliContext;
use crate::output;

pub fn run(ctx: &CliContext, threshold: f32, limit: usize) -> anyhow::Result<i32> {
    let promoter = ctx.promoter()?;
    // Rank without mutating state — the CLI is a read-only surface by
    // default. SANCHO's MemoryPromotionHandler is where actual writes
    // to memory_promotions happen.
    let ranked = promoter.rank_candidates()?;
    let filtered: Vec<_> = ranked
        .into_iter()
        .filter(|p| p.score >= threshold)
        .take(limit)
        .collect();
    output::print_promotions(&filtered);
    Ok(0)
}
