"""
ResearcherAgent — Literature / knowledge search via Brain + Superbrain.

Phase 2 deliverable. Wraps the brain_search and superbrain_vector_search
Harvey tools. Multiple instances can run in parallel via the DAG executor
(that's the whole point — research campaigns need fan-out).
"""

from __future__ import annotations

import logging
from typing import Any, Dict

from core.subagents.subagent import Subagent

log = logging.getLogger("harvey.subagent.researcher")


class ResearcherAgent(Subagent):
    """Specialized agent for information retrieval from Harvey's memory."""

    NAME = "researcher"
    ACTIONS = ["search_brain", "search_vector", "search_all"]
    DESCRIPTION = "Searches Brain + Superbrain for a given query."

    def execute(self, step, ctx: Dict) -> Dict[str, Any]:
        query = ctx.get("query") or ctx.get("topic") or ""
        if not query:
            return {"error": "no query provided", "ok": False, "findings": {}}

        action = step.action

        if action == "search_brain":
            result = self.tool("brain_search", {"query": query})
            return {
                "ok": True,
                "query": query,
                "source": "brain",
                "findings": result,
            }

        if action == "search_vector":
            result = self.tool("superbrain_vector_search", {"query": query})
            return {
                "ok": True,
                "query": query,
                "source": "vector",
                "findings": result,
            }

        # search_all — both in one step
        brain = self.tool("brain_search", {"query": query})
        vector = self.tool("superbrain_vector_search", {"query": query})
        return {
            "ok": True,
            "query": query,
            "source": "all",
            "findings": {"brain": brain, "vector": vector},
        }


__all__ = ["ResearcherAgent"]
