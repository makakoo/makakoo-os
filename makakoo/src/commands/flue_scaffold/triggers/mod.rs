//! Trigger renderers — one TS module per spec.triggers[] entry.
//!
//! Each renderer emits a module that exports:
//!
//! * `trigger()` — returns a Flue trigger definition. The runtime
//!   calls it at startup; the returned object includes a `run`
//!   callback that dispatches into the agent.

pub mod cron;
pub mod webhook;

use anyhow::{Context as _, Result};
use makakoo_core::agents::spec::TriggerSpec;

/// Relative path under the Flue project root, e.g.
/// `src/triggers/cron-0.ts`.
pub fn rel_path(i: usize, t: &TriggerSpec) -> String {
    format!("src/triggers/{}-{}.ts", kind_slug(t), i)
}

/// JS import alias for the assistant.ts static import. Prefixed
/// with `tr_` so channels and triggers can't collide.
pub fn import_alias(i: usize, t: &TriggerSpec) -> String {
    format!("tr_{}{}", kind_slug(t), i)
}

fn kind_slug(t: &TriggerSpec) -> &'static str {
    match t {
        TriggerSpec::Cron { .. } => "cron",
        TriggerSpec::Webhook { .. } => "webhook",
    }
}

/// Dispatch a trigger spec to its renderer. Returns the full TS
/// module body.
pub fn render(i: usize, t: &TriggerSpec) -> Result<String> {
    let _ = i; // index is encoded by the caller via rel_path/import_alias
    let body = match t {
        TriggerSpec::Cron { schedule, timezone } => cron::render(schedule, timezone),
        TriggerSpec::Webhook { path, secret_env } => webhook::render(path, secret_env),
    };
    body.with_context(|| format!("rendering trigger {:?}", t))
}
