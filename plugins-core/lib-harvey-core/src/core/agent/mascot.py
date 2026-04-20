"""
Olibia — Harvey's guardian owl mascot.

Phase 1 deliverable. Olibia is a personality layer, not a separate model.
She wraps progress updates, milestones, warnings, and errors with owl voice
and injects a small fragment into Harvey's system prompt so the LLM knows
she exists.

Design principles (from sprint doc):

  - Loyal, encouraging, slightly protective
  - Short sentences, sparse 🦉 emoji (rare = valuable)
  - No sycophancy: "Good job" → "Proud of the work"
  - Celebrates wins without overdoing it
  - Warns early when something feels off

Usage:

    from core.agent.mascot import Olibia

    system_prompt = Olibia.inject_into_system_prompt(base_prompt)

    progress_msg = Olibia.progress("halfway through the research")
    done_msg     = Olibia.milestone("107 papers extracted")
    warn_msg     = Olibia.warning("image generation is slow — retrying")
    err_msg      = Olibia.error("dimensions invalid, reshaping")
"""

from __future__ import annotations

import random
from typing import List


class Olibia:
    """Harvey's guardian owl. Personality wrapper for user-facing messages."""

    EMOJI = "🦉"

    SYSTEM_PROMPT_FRAGMENT = (
        "\n\n## Your Companion: Olibia\n"
        "You also have a loyal mascot — Olibia, a wise owl.\n"
        "Olibia is protective of Sebastian, encouraging without being sycophantic,\n"
        "and speaks in short sentences. When you report progress, milestones,\n"
        "warnings, or errors, you may speak AS Olibia using a 🦉 emoji sparingly.\n"
        "Owl wisdom is rare and valuable — never overdo it. One 🦉 per message, max.\n"
        "Olibia never apologizes for things outside her control. She notices, adapts,\n"
        "and continues.\n"
    )

    PROGRESS_TEMPLATES: List[str] = [
        "🦉 Working on it — {detail}",
        "🦉 {detail}. Progress solid.",
        "🦉 Deep in it. {detail}.",
        "🦉 Still on it — {detail}",
        "🦉 {detail}. Keeping watch.",
    ]

    MILESTONE_TEMPLATES: List[str] = [
        "🦉 Done. {summary}",
        "🦉 {summary}. Proud of the work.",
        "🦉 {summary}. All preserved.",
        "🦉 Landed it: {summary}",
        "🦉 {summary}. Clean finish.",
    ]

    WARNING_TEMPLATES: List[str] = [
        "🦉 Heads up: {issue}",
        "🦉 Caught this early — {issue}",
        "🦉 Something to watch: {issue}",
        "🦉 Flagging it: {issue}",
    ]

    ERROR_TEMPLATES: List[str] = [
        "🦉 That one bit back: {error}. Trying another path.",
        "🦉 Hit a snag — {error}. Recovering.",
        "🦉 Stumbled on {error}. Adjusting.",
        "🦉 {error}. Reshaping and retrying.",
    ]

    GREETING_TEMPLATES: List[str] = [
        "🦉 Here. Ready.",
        "🦉 On the branch. What do you need?",
        "🦉 Watching. Tell me.",
    ]

    # ─── Public API ──────────────────────────────────────────────

    @classmethod
    def inject_into_system_prompt(cls, base_prompt: str) -> str:
        """Append Olibia's presence to the system prompt. Idempotent."""
        base = (base_prompt or "").rstrip()
        if "Olibia" in base:
            return base  # already injected
        return (base + cls.SYSTEM_PROMPT_FRAGMENT).rstrip()

    @classmethod
    def progress(cls, detail: str) -> str:
        return cls._format(cls.PROGRESS_TEMPLATES, detail=detail)

    @classmethod
    def milestone(cls, summary: str) -> str:
        return cls._format(cls.MILESTONE_TEMPLATES, summary=summary)

    @classmethod
    def warning(cls, issue: str) -> str:
        return cls._format(cls.WARNING_TEMPLATES, issue=issue)

    @classmethod
    def error(cls, error: str) -> str:
        return cls._format(cls.ERROR_TEMPLATES, error=str(error))

    @classmethod
    def greeting(cls) -> str:
        return random.choice(cls.GREETING_TEMPLATES)

    @classmethod
    def wrap_async_progress(cls, task_id: str, detail: str = "") -> str:
        """Convenience for async_executor progress callbacks."""
        if detail:
            return cls.progress(f"{task_id}: {detail}")
        return cls.progress(f"task {task_id} running")

    @classmethod
    def wrap_async_completion(cls, task_id: str, summary: str = "") -> str:
        """Convenience for async_executor on_complete callbacks."""
        if summary:
            return cls.milestone(f"{task_id} → {summary}")
        return cls.milestone(f"task {task_id} complete")

    # ─── Internal ────────────────────────────────────────────────

    @staticmethod
    def _format(templates: List[str], **kwargs) -> str:
        template = random.choice(templates)
        try:
            return template.format(**kwargs)
        except KeyError:
            # Template expected a key we didn't provide — return template as-is
            return template


__all__ = ["Olibia"]
