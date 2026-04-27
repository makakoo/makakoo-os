//! Tier-C tool handlers — 6 heavy / orchestration tools landed in T15
//! (Wave 4).
//!
//! Unlike Tier-A (read-mostly) and Tier-B (single-subsystem writes),
//! Tier-C handlers touch the swarm gateway + coordinator and the
//! outbound chat pipeline. The two hard rules that make this tier
//! interesting:
//!
//! 1. `harvey_telegram_send` NEVER sends unsolicited messages. The
//!    handler enforces allow-listing and existing-conversation checks,
//!    and returns a structured `{ok: false, reason: "..."}` rather
//!    than sending anything it cannot justify.
//! 2. Swarm dispatches are fire-and-forget from the tool caller's
//!    perspective — `harvey_swarm_run` returns an id and the caller
//!    polls `harvey_swarm_status`.
//!
//! # Coverage
//!
//! | # | Tool | Module |
//! |---|---|---|
//! | 1 | `harvey_swarm_run` | `swarm` |
//! | 2 | `harvey_swarm_status` | `swarm` |
//! | 3 | `swarm` (legacy alias) | `swarm` |
//! | 4 | `harvey_olibia_speak` | `olibia` |
//! | 5 | `harvey_telegram_send` | `comm` |
//! | 6 | `chat_send` | `comm` |

pub mod comm;
pub mod olibia;
pub mod swarm;

use crate::dispatch::{ToolContext, ToolRegistry};
use std::sync::Arc;

/// Register every Tier-C handler with the shared registry.
pub fn register_tier_c(registry: &mut ToolRegistry, ctx: Arc<ToolContext>) {
    // swarm.rs — 3 tools (swarm_run + swarm_status + legacy alias)
    registry.register(Arc::new(swarm::HarveySwarmRunHandler::new(ctx.clone())));
    registry.register(Arc::new(swarm::HarveySwarmStatusHandler::new(ctx.clone())));
    registry.register(Arc::new(swarm::SwarmLegacyHandler::new(ctx.clone())));

    // olibia.rs — 1 tool
    registry.register(Arc::new(olibia::HarveyOlibiaSpeakHandler::new(ctx.clone())));

    // comm.rs — 2 tools
    registry.register(Arc::new(comm::HarveyTelegramSendHandler::new(ctx.clone())));
    registry.register(Arc::new(comm::ChatSendHandler::new(ctx)));
}
