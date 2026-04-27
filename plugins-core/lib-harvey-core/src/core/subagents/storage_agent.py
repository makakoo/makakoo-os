"""
StorageAgent — Persists outputs to Harvey's Brain (Brain journal / pages).

Phase 2 deliverable. Thin wrapper over the `brain_write` tool. Accepts
content from ctx, or falls back to the synthesizer's output via
resolved_artifacts.
"""

from __future__ import annotations

import logging
from typing import Any, Dict

from core.subagents.subagent import Subagent

log = logging.getLogger("harvey.subagent.storage")


class StorageAgent(Subagent):
    NAME = "storage"
    ACTIONS = ["save_to_brain", "archive"]
    DESCRIPTION = "Saves content to the Brain journal."

    def execute(self, step, ctx: Dict) -> Dict[str, Any]:
        content = _resolve_content(ctx)
        if not content:
            return {
                "ok": False,
                "stored": False,
                "error": "no content to store",
            }

        result = self.tool("brain_write", {"content": content})
        return {
            "ok": True,
            "stored": True,
            "bytes": len(content),
            "action": step.action,
            "tool_result": result,
        }


def _resolve_content(ctx: Dict) -> str:
    """Pull content from ctx, ctx.summary, or resolved_artifacts.{summary|content}."""
    if content := ctx.get("content"):
        return str(content)
    if summary := ctx.get("summary"):
        return str(summary)

    resolved = ctx.get("resolved_artifacts") or {}
    for payload in resolved.values():
        if isinstance(payload, dict):
            for key in ("summary", "content", "findings", "text"):
                if key in payload and payload[key]:
                    return str(payload[key])
    return ""


__all__ = ["StorageAgent"]
