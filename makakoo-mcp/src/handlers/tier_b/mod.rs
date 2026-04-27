//! Tier-B tool handlers — 15 write / mutation tools + the 4 omni
//! multimodal tools. See `tier_b/<submodule>.rs` for implementation.
//!
//! Registration happens in [`register_tier_b`], which is called from
//! `handlers::register_all` at server boot after the `ToolContext` is
//! fully constructed.

pub mod agents;
pub mod browse;
pub mod channel_ops;
pub mod infect;
pub mod journal;
pub mod knowledge;
pub mod multimodal;
pub mod nursery;
pub mod outbound;
pub mod perms;
pub mod pi;
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

    // Infect — project-scoped harvey install from chat.
    registry.register(Arc::new(infect::HarveyInfectLocalHandler::new(ctx.clone())));

    // Knowledge ingest — structured media → multimodal Qdrant collection.
    registry.register(Arc::new(knowledge::HarveyKnowledgeIngestHandler::new(
        ctx.clone(),
    )));

    // Pi — v0.2 Phase B.3/B.4: pi --rpc wrappers for agentic workflows.
    registry.register(Arc::new(pi::PiRunHandler::new(ctx.clone())));
    registry.register(Arc::new(pi::PiSessionForkHandler::new(ctx.clone())));
    registry.register(Arc::new(pi::PiSessionLabelHandler::new(ctx.clone())));
    registry.register(Arc::new(pi::PiSessionExportHandler::new(ctx.clone())));
    registry.register(Arc::new(pi::PiSetModelHandler::new(ctx.clone())));
    registry.register(Arc::new(pi::PiSteerHandler::new(ctx.clone())));

    // v0.4 Phase E: browser-harness CDP driver. Registered unconditionally;
    // falls back to a clear RPC error if the agent-browser-harness plugin
    // isn't installed under $MAKAKOO_HOME/plugins/.
    registry.register(Arc::new(browse::HarveyBrowseHandler::new(ctx.clone())));

    // v0.3 USER-GRANTS Phase E: conversational runtime user-grant tools.
    registry.register(Arc::new(perms::GrantWriteAccessHandler::new(ctx.clone())));
    registry.register(Arc::new(perms::RevokeWriteAccessHandler::new(ctx.clone())));
    registry.register(Arc::new(perms::ListWriteGrantsHandler::new(ctx.clone())));

    // v2-MEGA Phase 6: OpenClaw-parity channel-ops trait surface.
    registry.register(Arc::new(channel_ops::ChannelDirectoryListChannelsHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelDirectoryListUsersHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelDirectoryLookupUserHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelMessagingSendDmHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelMessagingSendChannelHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelMessagingBroadcastHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelThreadingCreateThreadHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelThreadingListThreadsHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelThreadingFollowThreadHandler::new(
        ctx.clone(),
    )));
    registry.register(Arc::new(channel_ops::ChannelApprovalRequestHandler::new(
        ctx.clone(),
    )));
}
