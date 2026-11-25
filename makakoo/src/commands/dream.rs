//! `makakoo dream` — memory consolidation pass.
//!
//! Reuses [`DreamHandler`] from `makakoo-core::sancho::handlers` so the
//! CLI path matches whatever SANCHO runs on its schedule — one
//! implementation, one journal line.

use std::sync::Arc;

use makakoo_core::sancho::handlers::{DreamHandler, LlmCall};
use makakoo_core::sancho::registry::SanchoHandler;

use crate::context::CliContext;
use crate::output;

pub async fn run(ctx: &CliContext) -> anyhow::Result<i32> {
    let _sleep_guard = makakoo_platform::SleepGuard::acquire("makakoo dream consolidation");
    let sancho_ctx = ctx.build_sancho_context()?;
    let llm_for_dream: Arc<dyn LlmCall> = Arc::clone(&sancho_ctx.llm) as Arc<dyn LlmCall>;
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
