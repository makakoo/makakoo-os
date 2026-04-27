//! Tier-A tool handlers — 20 read-mostly tools landed in T13 (Wave 4).
//!
//! Every handler in this tier is stateless against the Brain (no writes,
//! no mutations). They route through the `ToolContext` subsystems wired
//! in `main::build_context`. Handlers that need a subsystem not wired
//! return `RpcError::internal("subsystem not wired: <name>")`.
//!
//! # Coverage
//!
//! | # | Tool | Module |
//! |---|---|---|
//! | 1 | `brain_search` | `brain` |
//! | 2 | `brain_query` | `brain` |
//! | 3 | `brain_recent` | `brain` |
//! | 4 | `brain_entities` | `brain` |
//! | 5 | `brain_context` | `brain` |
//! | 6 | `harvey_brain_search` | `brain` |
//! | 7 | `harvey_superbrain_query` | `brain` |
//! | 8 | `harvey_superbrain_vector_search` | `brain` |
//! | 9 | `sancho_status` | `sancho` |
//! | 10 | `dream` | `sancho` |
//! | 11 | `skill_discover` | `skill` |
//! | 12 | `costs_summary` | `costs` |
//! | 13 | `nursery_status` | `nursery` |
//! | 14 | `buddy_status` | `nursery` |
//! | 15 | `wiki_lint` | `wiki` |
//! | 16 | `agent_list` | `agents` |
//! | 17 | `agent_info` | `agents` |
//! | 18 | `chat_status` | `chat` |
//! | 19 | `chat_history` | `chat` |
//! | 20 | `chat_stats` | `chat` |

pub mod agents;
pub mod brain;
pub mod chat;
pub mod costs;
pub mod nursery;
pub mod sancho;
pub mod skill;
pub mod wiki;

use crate::dispatch::{ToolContext, ToolRegistry};
use std::sync::Arc;

/// Register every Tier-A handler with the shared registry.
pub fn register_tier_a(registry: &mut ToolRegistry, ctx: Arc<ToolContext>) {
    // brain.rs — 8 tools
    registry.register(Arc::new(brain::BrainSearchHandler::new(ctx.clone())));
    registry.register(Arc::new(brain::BrainQueryHandler::new(ctx.clone())));
    registry.register(Arc::new(brain::BrainRecentHandler::new(ctx.clone())));
    registry.register(Arc::new(brain::BrainEntitiesHandler::new(ctx.clone())));
    registry.register(Arc::new(brain::BrainContextHandler::new(ctx.clone())));
    registry.register(Arc::new(brain::HarveyBrainSearchHandler::new(ctx.clone())));
    registry.register(Arc::new(brain::HarveySuperbrainQueryHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(brain::HarveySuperbrainVectorSearchHandler::new(
        ctx.clone(),
    )));

    // sancho.rs — 2 tools
    registry.register(Arc::new(sancho::SanchoStatusHandler::new(ctx.clone())));
    registry.register(Arc::new(sancho::DreamHandler::new(ctx.clone())));

    // skill.rs — 1 tool
    registry.register(Arc::new(skill::SkillDiscoverHandler::new(ctx.clone())));

    // costs.rs — 1 tool
    registry.register(Arc::new(costs::CostsSummaryHandler::new(ctx.clone())));

    // nursery.rs — 2 tools
    registry.register(Arc::new(nursery::NurseryStatusHandler::new(ctx.clone())));
    registry.register(Arc::new(nursery::BuddyStatusHandler::new(ctx.clone())));

    // wiki.rs — 1 tool
    registry.register(Arc::new(wiki::WikiLintHandler::new(ctx.clone())));

    // agents.rs — 2 tools
    registry.register(Arc::new(agents::AgentListHandler::new(ctx.clone())));
    registry.register(Arc::new(agents::AgentInfoHandler::new(ctx.clone())));

    // chat.rs — 3 tools
    registry.register(Arc::new(chat::ChatStatusHandler::new(ctx.clone())));
    registry.register(Arc::new(chat::ChatHistoryHandler::new(ctx.clone())));
    registry.register(Arc::new(chat::ChatStatsHandler::new(ctx)));
}
