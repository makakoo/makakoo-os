"""
Harvey OS Core — Multi-agent orchestration.

Top-level exports for the swarm plumbing shipped across Phase 1.5 → Phase 3:

  Phase 1.5 — Foundation:
    ArtifactStore, PersistentEventBus, get_default_store, get_default_bus

  Phase 2 — Coordination:
    AgentCoordinator

  Phase 3 — Swarm intelligence:
    TeamComposition, TeamRoster, TeamMember, build_workflow_from_team
    IntelligentRouter, IntentClassification
    ResourceMonitor, ResourceSnapshot, ScaleDecision
    FailureRecovery, CircuitBreaker, CircuitState

Prefer importing from this package root over reaching into individual
modules:

    from core.orchestration import IntelligentRouter, TeamComposition
"""

from core.orchestration.artifact_store import (
    ArtifactStore,
    get_default_store,
)
from core.orchestration.persistent_event_bus import (
    PersistentEventBus,
    get_default_bus,
)
from core.orchestration.agent_coordinator import AgentCoordinator
from core.orchestration.agent_team import (
    TeamComposition,
    TeamMember,
    TeamRoster,
    build_workflow_from_team,
)
from core.orchestration.intelligent_router import (
    IntelligentRouter,
    IntentClassification,
)
from core.orchestration.resource_monitor import (
    ResourceMonitor,
    ResourceSnapshot,
    ScaleDecision,
)
from core.orchestration.failure_recovery import (
    CircuitBreaker,
    CircuitState,
    FailureRecovery,
)

__all__ = [
    # Phase 1.5
    "ArtifactStore",
    "PersistentEventBus",
    "get_default_store",
    "get_default_bus",
    # Phase 2
    "AgentCoordinator",
    # Phase 3 — team composition
    "TeamComposition",
    "TeamMember",
    "TeamRoster",
    "build_workflow_from_team",
    # Phase 3 — routing
    "IntelligentRouter",
    "IntentClassification",
    # Phase 3 — resource monitor
    "ResourceMonitor",
    "ResourceSnapshot",
    "ScaleDecision",
    # Phase 3 — failure recovery
    "CircuitBreaker",
    "CircuitState",
    "FailureRecovery",
]
