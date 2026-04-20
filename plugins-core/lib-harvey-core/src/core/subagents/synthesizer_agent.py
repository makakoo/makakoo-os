"""
SynthesizerAgent — Combines findings from multiple upstream agents.

Phase 2 deliverable. Reads `resolved_artifacts` (injected by the
AsyncDAGExecutor from artifact_store), extracts the interesting bits,
and produces a combined summary.

For Phase 2 the synthesis is deterministic (concatenation + bulleting).
Phase 3 will optionally route through an LLM for real synthesis.
"""

from __future__ import annotations

import logging
from typing import Any, Dict, List

from core.subagents.subagent import Subagent

log = logging.getLogger("harvey.subagent.synthesizer")

MAX_SNIPPET = 500


class SynthesizerAgent(Subagent):
    NAME = "synthesizer"
    ACTIONS = ["combine", "summarize"]
    DESCRIPTION = "Combines upstream findings into a single summary."

    def execute(self, step, ctx: Dict) -> Dict[str, Any]:
        # resolved_artifacts is injected by AsyncDAGExecutor based on
        # step.input_context["reads_artifacts"]
        resolved = ctx.get("resolved_artifacts") or {}

        # Also accept inline findings via ctx["findings_list"] for tests
        inline = ctx.get("findings_list") or []

        if not resolved and not inline:
            return {
                "ok": False,
                "summary": "",
                "error": "no artifacts or findings to synthesize",
                "input_count": 0,
            }

        sections: List[str] = ["Synthesis:"]

        for name, payload in resolved.items():
            snippet = _extract_snippet(payload)
            sections.append(f"- [{name}] {snippet}")

        for i, item in enumerate(inline, 1):
            snippet = _extract_snippet(item)
            sections.append(f"- [inline_{i}] {snippet}")

        summary = "\n".join(sections)

        result = {
            "ok": True,
            "summary": summary,
            "input_count": len(resolved) + len(inline),
            "sources": list(resolved.keys()) + [f"inline_{i+1}" for i in range(len(inline))],
        }

        if step.action == "summarize":
            # Terser output — just the bullets, no "Synthesis:" header
            result["summary"] = "\n".join(sections[1:])

        return result


def _extract_snippet(payload: Any) -> str:
    """Pull a short representative string out of a heterogeneous payload."""
    if payload is None:
        return ""
    if isinstance(payload, str):
        return payload[:MAX_SNIPPET]
    if isinstance(payload, dict):
        # Prefer common result fields
        for key in ("findings", "summary", "result", "content", "text"):
            if key in payload:
                inner = payload[key]
                if isinstance(inner, str):
                    return inner[:MAX_SNIPPET]
                if isinstance(inner, dict):
                    return str(inner)[:MAX_SNIPPET]
                return str(inner)[:MAX_SNIPPET]
        return str(payload)[:MAX_SNIPPET]
    if isinstance(payload, (list, tuple)):
        return ", ".join(_extract_snippet(p) for p in payload[:5])[:MAX_SNIPPET]
    return str(payload)[:MAX_SNIPPET]


__all__ = ["SynthesizerAgent"]
