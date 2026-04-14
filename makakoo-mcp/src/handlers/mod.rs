//! Tool handler registration.
//!
//! Each MCP tool is a type that implements
//! [`crate::dispatch::ToolHandler`]. This module wires them into the
//! registry at server boot.
//!
//! Handler modules are added by Wave 4 parallel agents:
//!   - `tier_a` — 20 read tools (T13)
//!   - `tier_b` — 15 write tools (T14)
//!   - `tier_c` — 6 heavy tools + swarm gateway (T15)
//!
//! At T12 this module is intentionally empty — the server spine compiles
//! clean with zero tools registered, `--health` reports
//! `{"ok":true,"tools":0}`, and T13/T14/T15 add their registration calls
//! here without needing to touch anything else in the crate.

use crate::dispatch::{ToolContext, ToolRegistry};
use std::sync::Arc;

pub mod tier_a; // T13 — 20 read tools
pub mod tier_b; // T14 — 15 write + multimodal tools
pub mod tier_c; // T15 — 6 heavy + swarm tools

/// Register every tool handler with the shared registry.
///
/// Call once at boot, after `ToolContext` is fully constructed. Wave 4
/// agents append registrations here, one per tier:
///
/// ```ignore
/// tier_a::register_tier_a(registry, ctx.clone());
/// tier_b::register_tier_b(registry, ctx.clone());
/// tier_c::register_tier_c(registry, ctx);
/// ```
pub fn register_all(registry: &mut ToolRegistry, ctx: &Arc<ToolContext>) {
    // T13 — Tier-A: 20 read-only handlers.
    tier_a::register_tier_a(registry, Arc::clone(ctx));

    // T14 — Tier-B: 15 write / mutation + 4 multimodal handlers.
    tier_b::register_tier_b(registry, Arc::clone(ctx));

    // T15 — Tier-C: 6 heavy / swarm / comm handlers.
    tier_c::register_tier_c(registry, Arc::clone(ctx));
}
