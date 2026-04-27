//! `makakoo dream` — memory consolidation pass.
//!
//! Reuses [`DreamHandler`] from `makakoo-core::sancho::handlers` so the
//! CLI path matches whatever SANCHO runs on its schedule — one
//! implementation, one journal line.

use std::sync::Arc;

use makakoo_core::sancho::handlers::{DreamHandler, LlmCall};
use makakoo_core::sancho::registry::{SanchoContext, SanchoHandler};

use crate::context::CliContext;
use crate::output;

pub async fn run(ctx: &CliContext) -> anyhow::Result<i32> {
    let store = ctx.store()?;
    let bus = ctx.event_bus()?;
    let llm = ctx.llm();
    let emb = ctx.embeddings();
    let sancho_ctx = SanchoContext::new(store, bus, Arc::clone(&llm), emb, ctx.home().clone());
    let llm_for_dream: Arc<dyn LlmCall> = Arc::clone(&llm) as Arc<dyn LlmCall>;
    let handler = DreamHandler::new(llm_for_dream);
    output::print_info("dream: consolidating recent Brain docs...");
    let report = handler.run(&sancho_ctx).await?;
    if report.ok {
        output::print_info(format!("dream: {}", report.message));
        Ok(0)
    } else {
        output::print_warn(format!("dream failed: {}", report.message));
        Ok(2)
    }
}
