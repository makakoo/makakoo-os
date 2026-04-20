"""
ImageGenAgent — Generates, refines, and upscales images via Harvey's
generate_image tool (MiniMax image-01 behind switchAILocal).

Phase 2 deliverable.
"""

from __future__ import annotations

import logging
import re
from typing import Any, Dict

from core.subagents.subagent import Subagent

log = logging.getLogger("harvey.subagent.image_gen")

# Parse "Image saved to /path/to/file.png (123KB). Add [[SEND_PHOTO:...]]..."
_PATH_PATTERN = re.compile(r"Image saved to ([^\s\(]+)")


class ImageGenAgent(Subagent):
    """Wraps the generate_image tool as a DAG-addressable subagent."""

    NAME = "image_gen"
    ACTIONS = ["generate", "refine"]
    DESCRIPTION = "Generates images from text prompts via MiniMax."

    # SPRINT-HARVEY-TICKETING Phase 4: MiniMax image-01 does not like
    # concurrent generate calls from the same account — rate-limit to
    # one in-flight call via AgentCoordinator's BoundedSemaphore wrap.
    MAX_CONCURRENCY = 1

    def execute(self, step, ctx: Dict) -> Dict[str, Any]:
        prompt = (
            ctx.get("prompt")
            or ctx.get("initial_request")
            or ""
        )
        if not prompt:
            return {"error": "no prompt provided", "ok": False}

        save_path = ctx.get("save_path", "")
        aspect_ratio = ctx.get("aspect_ratio", "1:1")

        # For `refine`, we optionally feed prior image path into the prompt
        if step.action == "refine":
            prior = ctx.get("prior_image_path") or ""
            feedback = ctx.get("feedback", "")
            if prior and feedback:
                prompt = f"{prompt} (refining {prior}: {feedback})"

        log.info(f"[image_gen] generating: {prompt[:80]!r}")
        raw = self.tool(
            "generate_image",
            {
                "prompt": prompt,
                "save_path": save_path,
                "aspect_ratio": aspect_ratio,
            },
        )

        # tool returns a string; parse the saved path
        path = _extract_path(raw)
        ok = path is not None and "error" not in (raw or "").lower()

        result = {
            "ok": ok,
            "prompt": prompt,
            "aspect_ratio": aspect_ratio,
            "path": path,
            "raw": raw,
        }

        if path:
            # Publish a human-addressable artifact beyond the DAG's
            # auto-published `{wf_id}:{step_id}` one.
            try:
                self.publish_artifact(
                    name=f"image:{path.split('/')[-1]}",
                    payload={"path": path, "prompt": prompt},
                )
            except Exception as e:
                log.debug(f"[image_gen] ad-hoc artifact failed: {e}")

        return result


def _extract_path(tool_result: str) -> str:
    if not tool_result:
        return ""
    m = _PATH_PATTERN.search(tool_result)
    return m.group(1) if m else ""


__all__ = ["ImageGenAgent"]
