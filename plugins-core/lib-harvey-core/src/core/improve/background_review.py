#!/usr/bin/env python3
"""
Background Review — Hermes Auto-Improve Pattern for Harvey OS.

After every conversation, spawn a background thread that reviews the session
for worth-saving patterns and writes them to FrozenMemory or skill idea queue —
autonomously, without disturbing the main session.

Source: hermes-agent/run_agent.py:1588-1692 (_spawn_background_review)

Key design:
- Thread fork (not process) — lightweight, shares memory with parent
- Same model as parent — gets the same reasoning capability
- quiet_mode — no user-visible output
- Optional FrozenMemory store — writes go to ~/.harvey/frozen-memory/
- JSON-structured output — LLM responds with structured memory/skill suggestions
- Safe cleanup — daemon thread, won't block parent process exit
"""

from __future__ import annotations

import json
import logging
import os
import re
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, List, Optional

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Review prompts (structured JSON output)
# ---------------------------------------------------------------------------

_MEMORY_REVIEW_PROMPT = """You are a memory review agent. After each session, you extract key information from the transcript to save to persistent memory.

Review the conversation above and consider saving to memory if appropriate.

Focus on:
1. User preferences, personal details, communication style
2. User expectations about behavior and workflow
3. Project-specific conventions, coding style, architecture decisions
4. Anything the user would want to remember for next time

Respond ONLY with a JSON object:
{
  "memory_entries": [
    {"target": "memory", "content": "entry text for agent memory"},
    {"target": "user", "content": "entry text for user profile"}
  ],
  "notes": "brief explanation of what you found and why it's worth saving"
}

If nothing is worth saving, respond with:
{"memory_entries": [], "notes": "Nothing worth preserving in this session."}

Be specific — include actual values, names, paths when relevant.
Use the user's terminology, not generic phrasing."""

_SKILL_REVIEW_PROMPT = """You are a skill review agent. After each session, you identify reusable approaches worth codifying as skills.

Review the conversation above and consider saving a skill if appropriate.

Focus on:
1. Non-trivial approaches that solved a specific problem
2. Trial-and-error learning that produced a working solution
3. Techniques that worked well and could apply to future tasks
4. Patterns the user explicitly validated or requested again

Was a non-trivial approach used that required trial and error?

Respond ONLY with a JSON object:
{
  "skill_ideas": [
    {
      "name": "skill-name",
      "description": "1-2 sentence description of what this skill does",
      "when_to_use": "situation where this skill applies",
      "trigger_phrases": ["phrase1", "phrase2"]
    }
  ],
  "notes": "brief explanation"
}

If nothing is worth saving as a skill, respond with:
{"skill_ideas": [], "notes": "No reusable skills identified."}"""

_COMBINED_REVIEW_PROMPT = """You are a combined memory and skill review agent.

Review the conversation above for BOTH memory-worthy and skill-worthy content.

MEMORY FOCUS:
- User preferences, personal details, communication style
- Project conventions and architectural decisions
- What the user would want to remember next time

SKILL FOCUS:
- Reusable approaches from trial-and-error
- Techniques that solved specific problems well
- Narrow, practical patterns (not vague best practices)

Respond ONLY with a JSON object:
{
  "memory_entries": [
    {"target": "memory", "content": "entry for agent memory"},
    {"target": "user", "content": "entry for user profile"}
  ],
  "skill_ideas": [
    {
      "name": "skill-name",
      "description": "what this skill does",
      "when_to_use": "when to apply it",
      "trigger_phrases": ["phrase1", "phrase2"]
    }
  ],
  "notes": "explanation of findings"
}

If nothing worth saving, use empty arrays: {"memory_entries": [], "skill_ideas": [], "notes": "..."}"""


# ---------------------------------------------------------------------------
# Result types
# ---------------------------------------------------------------------------


@dataclass
class ReviewResult:
    """What the background review produced."""

    memory_entries_added: int
    skill_ideas_saved: int
    notes: str
    error: Optional[str] = None


# ---------------------------------------------------------------------------
# Background Review core
# ---------------------------------------------------------------------------


class BackgroundReview:
    """
    Spawns a background thread that reviews a conversation for worth-saving
    patterns and writes them to FrozenMemory or skill idea queue.

    Usage:
        review = BackgroundReview(
            model="auto",
            memory_store=memory_store,
        )
        review.spawn(messages, review_memory=True, review_skills=True)

    The spawn() call returns immediately. The review runs in a daemon thread.
    When it completes, it optionally calls background_review_callback with a
    compact summary (e.g. "💾 2 memories saved · 1 skill idea").
    """

    def __init__(
        self,
        model: Optional[str] = None,
        memory_store: Optional[Any] = None,
        background_review_callback: Optional[Callable[[str], None]] = None,
    ):
        self.model = model or os.environ.get("LLM_MODEL", "auto")
        self.memory_store = memory_store
        self.background_review_callback = background_review_callback

    def spawn(
        self,
        messages: List[dict],
        review_memory: bool = True,
        review_skills: bool = True,
    ) -> None:
        """
        Spawn a background review thread.

        Args:
            messages: Full conversation transcript (list of message dicts)
            review_memory: Whether to review for memory saves
            review_skills: Whether to review for skill saves
        """
        if not review_memory and not review_skills:
            return

        if review_memory and review_skills:
            prompt = _COMBINED_REVIEW_PROMPT
        elif review_memory:
            prompt = _MEMORY_REVIEW_PROMPT
        else:
            prompt = _SKILL_REVIEW_PROMPT

        def _run_review():
            result = self._do_review(prompt, messages, review_memory, review_skills)
            if result.error:
                logger.debug("Background review failed: %s", result.error)
            elif result.memory_entries_added > 0 or result.skill_ideas_saved > 0:
                parts = []
                if result.memory_entries_added > 0:
                    parts.append(f"{result.memory_entries_added} mem")
                if result.skill_ideas_saved > 0:
                    parts.append(f"{result.skill_ideas_saved} skill")
                summary = "💾 " + " · ".join(parts)
                if self.background_review_callback:
                    try:
                        self.background_review_callback(summary)
                    except Exception:
                        pass

        t = threading.Thread(
            target=_run_review,
            daemon=True,
            name="harvey-bg-review",
        )
        t.start()

    def _do_review(
        self,
        prompt: str,
        messages: List[dict],
        review_memory: bool,
        review_skills: bool,
    ) -> ReviewResult:
        """
        Run the review. Calls LLM via switchAILocal, parses JSON response,
        writes memory entries to FrozenMemory and skill ideas to queue.
        """
        # Build conversation for review agent
        review_messages = [
            {
                "role": "system",
                "content": (
                    "You are a background review agent. Respond ONLY with JSON. "
                    "No preamble, no explanation, just the JSON object."
                ),
            }
        ]

        # Add conversation (last 50 messages to avoid huge context)
        for msg in messages[-50:]:
            review_messages.append(
                {
                    "role": msg.get("role", "user"),
                    "content": msg.get("content", "")[:4000],  # cap individual messages
                }
            )

        # Add review instruction
        review_messages.append({"role": "user", "content": prompt})

        # Call LLM
        try:
            response = _call_review_llm(review_messages, model=self.model)
        except RuntimeError as e:
            return ReviewResult(0, 0, "", error=str(e))
        except Exception as e:
            return ReviewResult(0, 0, "", error=str(e))

        response_text = response if isinstance(response, str) else str(response)

        # Parse JSON from response
        entries_added = 0
        skills_saved = 0

        parsed = self._extract_json(response_text)
        if parsed is None:
            return ReviewResult(0, 0, "Could not parse JSON from review response")

        notes = parsed.get("notes", "")

        # Write memory entries
        if review_memory and self.memory_store:
            for entry in parsed.get("memory_entries", []):
                target = entry.get("target", "memory")
                content = entry.get("content", "").strip()
                if content and len(content) > 10:
                    try:
                        result = self.memory_store.add(target, content)
                        if result.get("success"):
                            entries_added += 1
                    except Exception as e:
                        logger.debug("Failed to add memory entry: %s", e)

        # Save skill ideas to queue
        if review_skills:
            skill_ideas = parsed.get("skill_ideas", [])
            if skill_ideas:
                try:
                    skills_saved = self._save_skill_ideas(skill_ideas)
                except Exception as e:
                    logger.debug("Failed to save skill ideas: %s", e)

        return ReviewResult(
            memory_entries_added=entries_added,
            skill_ideas_saved=skills_saved,
            notes=notes,
        )

    @staticmethod
    def _extract_json(text: str) -> Optional[dict]:
        """Extract JSON from LLM response text."""
        text = text.strip()

        # Try direct JSON parse first
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            pass

        # Try to find JSON in code block
        match = re.search(r"```(?:json)?\s*\n?(.*?)\n?```", text, re.DOTALL)
        if match:
            try:
                return json.loads(match.group(1).strip())
            except json.JSONDecodeError:
                pass

        # Try to find { ... } pattern
        start = text.find("{")
        end = text.rfind("}") + 1
        if start >= 0 and end > start:
            try:
                return json.loads(text[start:end])
            except json.JSONDecodeError:
                pass

        return None

    def _save_skill_ideas(self, skill_ideas: List[dict]) -> int:
        """Save skill ideas to the skill ideas queue."""
        ideas_dir = Path.home() / ".harvey" / "frozen-memory" / "skill-ideas"
        ideas_dir.mkdir(parents=True, exist_ok=True)

        saved = 0
        for idea in skill_ideas:
            name = idea.get("name", "").strip().replace(" ", "-").lower()
            if not name:
                continue
            timestamp = Path(ideas_dir).stat().st_mtime if ideas_dir.exists() else 0
            filename = f"{int(timestamp)}_{name[:40]}.json"
            path = ideas_dir / filename
            try:
                path.write_text(json.dumps(idea, indent=2))
                saved += 1
            except Exception:
                pass
        return saved


# ---------------------------------------------------------------------------
# LLM call via switchAILocal
# ---------------------------------------------------------------------------


def _call_review_llm(
    messages: List[dict],
    model: str,
    temperature: float = 0.3,
    max_tokens: int = 2048,
) -> str:
    """
    Call MiniMax-M2.7 (or configured model) via switchAILocal.

    Falls back to RuntimeError if the service is unavailable.
    """
    import httpx

    base_url = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1").rstrip("/")
    api_key = os.environ.get("SWITCHAI_KEY", "")

    payload: dict = {
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
    }

    last_error = RuntimeError("LLM call failed after 3 attempts")
    for attempt in range(3):
        try:
            with httpx.Client(timeout=60.0) as client:
                resp = client.post(
                    f"{base_url}/chat/completions",
                    json=payload,
                    headers={
                        "Authorization": f"Bearer {api_key}",
                        "Content-Type": "application/json",
                    },
                )
            resp.raise_for_status()
            result = resp.json()
            if result.get("choices") and len(result["choices"]) > 0:
                content = result["choices"][0]["message"]["content"]
                return content if isinstance(content, str) else str(content)
            last_error = RuntimeError(
                f"LLM returned null choices (attempt {attempt + 1}/3)"
            )
        except (httpx.ConnectError, httpx.TimeoutException) as e:
            last_error = RuntimeError(f"switchAILocal unavailable or timeout: {e}")
        except Exception as e:
            last_error = RuntimeError(f"LLM call failed: {e}")

        if attempt < 2:
            import time as _time

            _time.sleep(1 * (attempt + 1))

    raise last_error


# ---------------------------------------------------------------------------
# Convenience function
# ---------------------------------------------------------------------------


def spawn_background_review(
    messages: List[dict],
    model: Optional[str] = None,
    memory_store: Optional[Any] = None,
    review_memory: bool = True,
    review_skills: bool = True,
    background_review_callback: Optional[Callable[[str], None]] = None,
) -> None:
    """
    One-shot background review spawn.

    Convenience function that creates a BackgroundReview and immediately
    spawns the thread. Returns immediately.

    Example:
        spawn_background_review(
            messages=conversation_history,
            review_memory=True,
            review_skills=True,
        )
    """
    review = BackgroundReview(
        model=model,
        memory_store=memory_store,
        background_review_callback=background_review_callback,
    )
    review.spawn(messages, review_memory=review_memory, review_skills=review_skills)
