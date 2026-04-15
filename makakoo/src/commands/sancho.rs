//! `makakoo sancho tick|status` — SANCHO proactive task engine.
//!
//! `tick` constructs the default registry and fires every eligible
//! handler exactly once. `status` prints registered tasks and their
//! last-run timestamps (best-effort introspection — we query gate
//! state directly since there's no persisted task ledger).

use std::sync::Arc;
use std::time::Duration;

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};

use makakoo_core::plugin::PluginRegistry;
use makakoo_core::sancho::{default_registry, SanchoContext, SanchoEngine};

use crate::cli::SanchoCmd;
use crate::context::CliContext;
use crate::output;

pub async fn run(ctx: &CliContext, cmd: SanchoCmd) -> anyhow::Result<i32> {
    match cmd {
        SanchoCmd::Tick => tick(ctx).await,
        SanchoCmd::Status => status(ctx).await,
    }
}

async fn build_engine(ctx: &CliContext) -> anyhow::Result<SanchoEngine> {
    let store = ctx.store()?;
    let bus = ctx.event_bus()?;
    let llm = ctx.llm();
    let emb = ctx.embeddings();
    let sancho_ctx = Arc::new(SanchoContext::new(
        store,
        bus,
        llm,
        emb,
        ctx.home().clone(),
    ));
    let plugins = PluginRegistry::load_default(ctx.home())
        .unwrap_or_default();
    let registry = default_registry(Arc::clone(&sancho_ctx), &plugins);
    Ok(SanchoEngine::new(
        registry,
        sancho_ctx,
        Duration::from_secs(60),
    ))
}

async fn tick(ctx: &CliContext) -> anyhow::Result<i32> {
    let engine = build_engine(ctx).await?;
    output::print_info(format!(
        "sancho: ticking {} registered task(s)…",
        engine.task_count()
    ));
    let reports = engine.tick_once().await?;
    output::print_handler_reports(&reports);
    let failed = reports.iter().any(|r| !r.ok);
    Ok(if failed { 2 } else { 0 })
}

async fn status(ctx: &CliContext) -> anyhow::Result<i32> {
    let engine = build_engine(ctx).await?;
    let state = engine.state();
    let guard = state.lock().await;
    output::print_info(format!(
        "sancho: {} registered task(s)",
        engine.task_count()
    ));
    if guard.last_run.is_empty() {
        output::print_info("(no task has run yet this process)");
    } else {
        let mut t = Table::new();
        t.load_preset(UTF8_FULL);
        t.set_header(vec![
            Cell::new("task").fg(TableColor::Cyan),
            Cell::new("last_run").fg(TableColor::Cyan),
            Cell::new("busy").fg(TableColor::Cyan),
        ]);
        for (task, ts) in guard.last_run.iter() {
            let busy = guard.locks.get(task).copied().unwrap_or(false);
            t.add_row(vec![
                Cell::new(task),
                Cell::new(ts.to_rfc3339()),
                Cell::new(busy.to_string()),
            ]);
        }
        println!("{t}");
    }
    Ok(0)
}
