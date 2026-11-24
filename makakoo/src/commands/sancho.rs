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
use makakoo_core::sancho::{default_registry, SanchoEngine, NATIVE_TASK_COUNT};
use makakoo_core::superbrain::folder_index::IndexModelState;
use makakoo_core::superbrain::vector_model::DEFAULT_REBUILD_WAIT_TIMEOUT;

use crate::cli::SanchoCmd;
use crate::context::CliContext;
use crate::output;

#[cfg(test)]
#[path = "sancho_tests.rs"]
mod tests;

pub async fn run(ctx: &CliContext, cmd: SanchoCmd) -> anyhow::Result<i32> {
    match cmd {
        SanchoCmd::Tick => tick(ctx).await,
        SanchoCmd::Status => status(ctx).await,
    }
}

/// Resolve the upper bound for one-shot `makakoo sancho tick` waiting on
/// background Brain vector rebuilds (`MAKAKOO_SANCHO_REBUILD_WAIT_MS` or
/// `MAKAKOO_SANCHO_REBUILD_WAIT_SECS`; default `4 × REBUILD_LEASE_MS`).
pub(crate) fn resolve_rebuild_wait_timeout() -> Duration {
    resolve_rebuild_wait_timeout_with(|k| std::env::var(k).ok())
}

fn resolve_rebuild_wait_timeout_with(get_env: impl Fn(&str) -> Option<String>) -> Duration {
    if let Some(ms) = get_env("MAKAKOO_SANCHO_REBUILD_WAIT_MS")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
    {
        return Duration::from_millis(ms);
    }
    if let Some(secs) = get_env("MAKAKOO_SANCHO_REBUILD_WAIT_SECS")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
    {
        return Duration::from_secs(secs);
    }
    DEFAULT_REBUILD_WAIT_TIMEOUT
}

async fn build_engine(ctx: &CliContext) -> anyhow::Result<SanchoEngine> {
    let sancho_ctx = ctx.build_sancho_context()?;
    let plugins = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let registry = default_registry(Arc::clone(&sancho_ctx), &plugins);
    Ok(SanchoEngine::new(
        registry,
        sancho_ctx,
        Duration::from_secs(60),
    ))
}

async fn tick(ctx: &CliContext) -> anyhow::Result<i32> {
    Ok(run_tick(ctx).await?.0)
}

async fn run_tick(ctx: &CliContext) -> anyhow::Result<(i32, bool)> {
    let engine = build_engine(ctx).await?;
    // Run the store-open probe on the same process-wide coordinator so SANCHO
    // and the open probe deduplicate in-process and wait_all covers both.
    ctx.probe_brain_on_open()?;
    output::print_info(format!(
        "sancho: ticking {} registered task(s)…",
        engine.task_count()
    ));
    let reports = engine.tick_once().await?;
    output::print_handler_reports(&reports);

    let coord = &engine.context().brain_rebuild;
    let waited = coord.spawned_count() > 0 || coord.has_active_work();
    let mut rebuild_ok = true;
    if waited {
        let wait_timeout = resolve_rebuild_wait_timeout();
        let completed = coord.wait_all_timeout(wait_timeout).await;
        let status = engine.context().store.vector_status()?;
        if !completed {
            let reason = status.stale_reason.as_deref().unwrap_or("wait_timeout");
            output::print_warn(format!(
                "sancho: brain vector rebuild timed out after {}s (state={}, stale_reason={reason})",
                wait_timeout.as_secs(),
                status.index_state.as_str(),
            ));
            rebuild_ok = false;
        } else if status.index_state == IndexModelState::Ready {
            if coord.spawned_count() > 0 {
                output::print_info("sancho: brain vector rebuild finished");
            }
        } else {
            let reason = status.stale_reason.as_deref().unwrap_or("unknown");
            output::print_warn(format!(
                "sancho: brain vector rebuild did not reach ready (state={}, stale_reason={reason})",
                status.index_state.as_str(),
            ));
            rebuild_ok = false;
        }
    }

    let rebuild_ran = coord.spawned_count() > 0;
    let failed = reports.iter().any(|r| !r.ok) || !rebuild_ok;
    Ok((if failed { 2 } else { 0 }, rebuild_ran))
}

async fn status(ctx: &CliContext) -> anyhow::Result<i32> {
    let engine = build_engine(ctx).await?;
    let state = engine.state();
    let guard = state.lock().await;
    let total = engine.task_count();
    let manifest = total.saturating_sub(NATIVE_TASK_COUNT);
    output::print_info(format!(
        "sancho: {total} registered task(s) ({native} native + {manifest} manifest)",
        native = NATIVE_TASK_COUNT,
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
