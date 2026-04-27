"""
Harvey agent plugin system.

Two layers coexist during the Phase 5 → Phase 9 transition:

1. Legacy scaffolder (core.agents.scaffold) — creates new agent
   directories, lists them, installs / uninstalls. Preserved unchanged.

2. New plugin registry (core.agents.manifest + core.agents.loader) —
   machine-readable AgentManifest schema + an AgentRegistry that scans
   `agents/*/agent.yaml` and auto-discovers `core/subagents/*` classes.

Both layers export from this __init__ so existing callers don't break.
"""

# Legacy scaffolder — keep exports stable
from core.agents.scaffold import (
    scaffold_agent,
    list_agents,
    agent_info,
    install_agent,
    uninstall_agent,
)

# New plugin system
from core.agents.manifest import (
    AgentComm,
    AgentManifest,
    AgentRuntime,
    AgentState,
    AgentTool,
    AgentType,
    ManifestValidationError,
)
from core.agents.loader import AgentRegistry
from core.agents.capability_index import CapabilityCollision, CapabilityIndex

__all__ = [
    # Legacy
    "scaffold_agent",
    "list_agents",
    "agent_info",
    "install_agent",
    "uninstall_agent",
    # New
    "AgentComm",
    "AgentManifest",
    "AgentRegistry",
    "AgentRuntime",
    "AgentState",
    "AgentTool",
    "AgentType",
    "CapabilityCollision",
    "CapabilityIndex",
    "ManifestValidationError",
]
