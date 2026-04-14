//! Tier-B tool handlers — 15 write / mutation tools + the 4 omni
//! multimodal tools. See `tier_b/<submodule>.rs` for implementation.
//!
//! Registration happens in [`register_tier_b`], which is called from
//! `handlers::register_all` at server boot after the `ToolContext` is
//! fully constructed.

pub mod agents;
pub mod journal;
pub mod multimodal;
pub mod nursery;
pub mod outbound;
pub mod sancho;
pub mod wiki;

use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolRegistry};

/// Register every Tier-B handler with the shared registry. Call once,
/// after the context is wired.
pub fn register_tier_b(registry: &mut ToolRegistry, ctx: Arc<ToolContext>) {
    // Journal writes — 3 tools that all append to today's Brain journal.
    registry.register(Arc::new(journal::BrainWriteJournalHandler::new(ctx.clone())));
    registry.register(Arc::new(journal::HarveyBrainWriteHandler::new(ctx.clone())));
    registry.register(Arc::new(journal::HarveyJournalEntryHandler::new(ctx.clone())));

    // Wiki compile + save.
    registry.register(Arc::new(wiki::WikiCompileHandler::new(ctx.clone())));
    registry.register(Arc::new(wiki::WikiSaveHandler::new(ctx.clone())));

    // SANCHO tick (stub until T17 daemonizes the engine).
    registry.register(Arc::new(sancho::SanchoTickHandler::new(ctx.clone())));

    // Outbound — draft only, never auto-send.
    registry.register(Arc::new(outbound::OutboundDraftHandler::new(ctx.clone())));

    // Agent scaffold — install / uninstall / create.
    registry.register(Arc::new(agents::AgentInstallHandler::new(ctx.clone())));
    registry.register(Arc::new(agents::AgentUninstallHandler::new(ctx.clone())));
    registry.register(Arc::new(agents::AgentCreateHandler::new(ctx.clone())));

    // Nursery — hatch a new mascot.
    registry.register(Arc::new(nursery::NurseryHatchHandler::new(ctx.clone())));

    // Multimodal — 4 omni tools.
    registry.register(Arc::new(multimodal::DescribeImageHandler::new(ctx.clone())));
    registry.register(Arc::new(multimodal::DescribeAudioHandler::new(ctx.clone())));
    registry.register(Arc::new(multimodal::DescribeVideoHandler::new(ctx.clone())));
    registry.register(Arc::new(multimodal::GenerateImageHandler::new(ctx.clone())));
}
