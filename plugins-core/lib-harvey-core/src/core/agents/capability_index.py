"""
CapabilityIndex — action → agent routing table for the Harvey ticketing v1.

Built from an AgentRegistry, walks every active manifest's `tools` list,
and builds a flat `{action: agent_name}` lookup. This is the single source
of truth the planner uses to pick an agent for a given action instead of
hardcoding `role:` strings in the LLM prompt.

Design decision (from SPRINT-HARVEY-TICKETING round 2 negotiation):
  - Raise on collision at build time — an action cannot be claimed by two
    agents. If this ever happens in the real codebase, it represents an
    architectural decision that should be explicit, not silently
    last-write-wins. The error message points at the exact fix.
  - Inactive manifests (status=disabled|experimental) are NOT indexed.
  - An agent with zero actions is allowed — it contributes nothing to the
    index. Useful for daemon/cron agents that don't expose subagent tools.

Usage:
    from core.agents import AgentRegistry, CapabilityIndex
    registry = AgentRegistry()
    index = CapabilityIndex.build_from_registry(registry)
    agent_name = index.route("search_all")  # -> "researcher"
"""

from __future__ import annotations

import logging
from typing import Dict, List, Optional

from .loader import AgentRegistry
from .manifest import AgentManifest, AgentState

log = logging.getLogger("harvey.agents.capability_index")


class CapabilityCollision(ValueError):
    """Raised when two agents claim the same action.

    Message template is actionable: it names both agents and suggests
    the `conflict_policy` manifest field to add when overlap is
    intentional (not implemented in v1 — intended as a forcing function
    to make the next developer think before silently picking one).
    """
    pass


class CapabilityIndex:
    """Flat action → agent lookup built from an AgentRegistry."""

    def __init__(self, mapping: Optional[Dict[str, str]] = None):
        self._map: Dict[str, str] = dict(mapping or {})

    # ─── Build ──────────────────────────────────────────────────

    @classmethod
    def build_from_registry(cls, registry: AgentRegistry) -> "CapabilityIndex":
        """Walk the registry, build the action→agent map, raise on collision."""
        mapping: Dict[str, str] = {}
        for manifest in registry.list_all():
            if not manifest.is_active():
                continue
            _add_manifest_actions(mapping, manifest)
        log.info(
            f"[capability_index] built with {len(mapping)} action(s) "
            f"from {registry.count()} manifest(s)"
        )
        return cls(mapping)

    # ─── Query ──────────────────────────────────────────────────

    def route(self, action: str) -> Optional[str]:
        """Return the agent name that claims this action, or None."""
        return self._map.get(action)

    def actions_for(self, agent_name: str) -> List[str]:
        """Return all actions claimed by this agent, sorted."""
        return sorted(a for a, n in self._map.items() if n == agent_name)

    def all_actions(self) -> List[str]:
        """Return every indexed action, sorted."""
        return sorted(self._map.keys())

    def __contains__(self, action: str) -> bool:
        return action in self._map

    def __len__(self) -> int:
        return len(self._map)

    def to_dict(self) -> Dict[str, str]:
        """Return a copy of the internal map — for introspection/logging."""
        return dict(self._map)


# ─── Internal helpers ────────────────────────────────────────────


def _add_manifest_actions(mapping: Dict[str, str], manifest: AgentManifest) -> None:
    """Add every tool from `manifest` to `mapping` or raise on collision."""
    for tool in manifest.tools:
        action = tool.name
        if not action:
            continue
        existing = mapping.get(action)
        if existing is not None and existing != manifest.name:
            raise CapabilityCollision(
                f"action '{action}' is claimed by both '{existing}' and "
                f"'{manifest.name}' — add `conflict_policy: first|error` to "
                f"one of the agent.yaml manifests to resolve, or rename "
                f"one of the actions to disambiguate."
            )
        mapping[action] = manifest.name
