//! Swarm subsystem — Tier-C cognitive orchestration layer.
//!
//! Ports the core swarm primitives from `core/orchestration/` (Python)
//! into makakoo-core so both the umbrella CLI and the MCP server can
//! dispatch subagents and inspect swarm runs without diverging.
//!
//! Submodules:
//!
//! * [`artifacts`] — append-only typed log of plans / results / logs /
//!   checkpoints, backed by `swarm_artifacts` in the unified SQLite file.
//! * [`coordinator`] — tokio-task lifecycle for every spawned subagent.
//! * [`gateway`] — dispatch surface (coordinator + artifact + LLM + bus).
//! * [`olibia`] — passive owl-voiced listener for `sancho.*` / `swarm.*`
//!   / `superbrain.*` / `nursery.*` events.
//! * [`state`] — `SwarmState` handle bundling all three for global reuse.
//! * [`team`] — `TeamComposition` factories for pre-wired rosters
//!   (research / archive / minimal). Ported from the Python
//!   `agent_team.py` in v0.2 Phase A.2.

pub mod artifacts;
pub mod coordinator;
pub mod gateway;
pub mod olibia;
pub mod state;
pub mod team;

pub use artifacts::{Artifact, ArtifactKind, ArtifactStore};
pub use coordinator::{AgentCoordinator, SubagentHandle, SubagentSpec, SubagentStatus};
pub use gateway::{
    DispatchRequest, DispatchResponse, SwarmGateway, SwarmRunStatus, TeamDispatchRequest,
    TeamDispatchResponse,
};
pub use olibia::{OlibiaEvent, OlibiaSubagent};
pub use state::{SwarmState, SwarmStateHandle};
pub use team::{TeamComposition, TeamMember, TeamRoster};
