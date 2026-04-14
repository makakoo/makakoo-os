//! Shared swarm state handle — the single Arc the CLI and MCP both reach
//! for.
//!
//! `SwarmState` bundles the three stateful pieces (gateway, coordinator,
//! artifact store) so callers don't have to carry three Arcs everywhere.
//! A process-global singleton is available via [`SwarmState::init`] /
//! [`SwarmState::global`] using `OnceCell`, matching the style of the
//! event bus and gateway.

use std::sync::Arc;

use once_cell::sync::OnceCell;

use crate::error::{MakakooError, Result};

use super::artifacts::ArtifactStore;
use super::coordinator::AgentCoordinator;
use super::gateway::SwarmGateway;

/// Typed handle to a shared swarm state.
pub type SwarmStateHandle = Arc<SwarmState>;

/// All the swarm subsystems wired together.
pub struct SwarmState {
    pub gateway: Arc<SwarmGateway>,
    pub coordinator: Arc<AgentCoordinator>,
    pub artifacts: Arc<ArtifactStore>,
}

static STATE: OnceCell<SwarmStateHandle> = OnceCell::new();

impl SwarmState {
    /// Install the global swarm state. First caller wins; subsequent
    /// calls return an error so we never drop a live subagent handle.
    pub fn init(
        gateway: Arc<SwarmGateway>,
        coordinator: Arc<AgentCoordinator>,
        artifacts: Arc<ArtifactStore>,
    ) -> Result<SwarmStateHandle> {
        let handle = Arc::new(Self {
            gateway,
            coordinator,
            artifacts,
        });
        STATE
            .set(Arc::clone(&handle))
            .map_err(|_| MakakooError::internal("SwarmState already initialised"))?;
        Ok(handle)
    }

    /// Access the global swarm state, if installed.
    pub fn global() -> Option<SwarmStateHandle> {
        STATE.get().map(Arc::clone)
    }
}
