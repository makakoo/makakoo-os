#!/usr/bin/env python3
"""
Handoff Generator — Sprint 3

Generates a session handoff summary when compaction fires.
The summary preserves work progress across session rotations.
"""

import json
import logging
import os
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

from google import genai
from google.genai import types

logger = logging.getLogger(__name__)

_HANDOFF_TEMPLATE = """## Session Handoff

**Session ID:** {session_id}
**Started:** {started_at}
**Runs:** {runs} | **Input tokens:** {input_tokens:,} | **Output tokens:** {output_tokens:,}
**Duration:** {duration_hours:.1f} hours

### What was being worked on:
{ongoing_work}

### Key decisions made:
{decisions}

### Open threads / next steps:
{next_steps}

### Important context to preserve:
{context}
"""

# Gemini model for summarization
_SUMMARY_MODEL = "gemini-3-flash-preview"
# Max recent messages to include in LLM prompt
_MAX_MESSAGES_FOR_LLM = 20


class HandoffGenerator:
    """Generates handoff summaries for session compaction."""

    def __init__(self, gemini_api_key: Optional[str] = None):
        self.gemini_api_key = gemini_api_key or self._load_api_key()
        self._client: Optional[genai.Client] = None

    @property
    def client(self) -> Optional[genai.Client]:
        """Lazy-init Gemini client only when needed."""
        if self._client is None and self.gemini_api_key:
            self._client = genai.Client(api_key=self.gemini_api_key)
        return self._client

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def generate_handoff(
        self,
        session_id: str,
        started_at: datetime,
        runs: int,
        input_tokens: int,
        output_tokens: int,
        messages: List[Dict],
    ) -> str:
        """Generate a handoff summary from session history.

        Uses the LLM to summarize the conversation into structured handoff notes.
        Falls back to template-based summarization if LLM unavailable.

        Args:
            session_id: Current session ID
            started_at: When session started
            runs: Number of runs in this session
            input_tokens: Total input tokens used
            output_tokens: Total output tokens used
            messages: Full conversation message history

        Returns:
            Handoff note string (markdown format)
        """
        # Try LLM path first
        if self.client:
            try:
                summary_parts = self._summarize_with_llm(messages, session_id)
                return self._fill_template(
                    session_id=session_id,
                    started_at=started_at,
                    runs=runs,
                    input_tokens=input_tokens,
                    output_tokens=output_tokens,
                    ongoing_work=summary_parts.get("ongoing_work", "Unknown"),
                    decisions=summary_parts.get("decisions", "None recorded."),
                    next_steps=summary_parts.get("next_steps", "None recorded."),
                    context=summary_parts.get("context", "None recorded."),
                )
            except Exception as e:
                logger.warning("LLM handoff generation failed, falling back to template: %s", e)

        # Fallback: template-based without LLM
        return self.generate_simple_handoff(
            session_id=session_id,
            started_at=started_at,
            runs=runs,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            messages=messages,
        )

    def generate_simple_handoff(
        self,
        session_id: str,
        started_at: datetime,
        runs: int,
        input_tokens: int,
        output_tokens: int,
        messages: List[Dict],
    ) -> str:
        """Template-based handoff without LLM.

        Extracts what we can without an LLM call:
        - Last user message (ongoing work)
        - Last assistant response summary
        - Number of tool calls used
        """
        ongoing_work = "Could not determine — no LLM available."
        decisions = "None recorded."
        next_steps = "None recorded."
        context_parts: List[str] = []

        # Count tool calls
        tool_call_count = sum(
            1 for m in messages
            if m.get("role") == "assistant" and m.get("tool_calls")
        )
        if tool_call_count:
            context_parts.append(f"Tool calls made: {tool_call_count}")

        # Last user message as ongoing work indicator
        last_user_msg = None
        for m in reversed(messages):
            if m.get("role") == "user":
                last_user_msg = m.get("content", "") or ""
                break

        if last_user_msg:
            # Truncate long messages
            if len(last_user_msg) > 500:
                last_user_msg = last_user_msg[:500] + "..."
            ongoing_work = f"Last user request: {last_user_msg}"

        # Last assistant message as context
        last_assistant = None
        for m in reversed(messages):
            if m.get("role") == "assistant":
                last_assistant = m.get("content", "") or ""
                break

        if last_assistant:
            if len(last_assistant) > 500:
                last_assistant = last_assistant[:500] + "..."
            context_parts.append(f"Last assistant response: {last_assistant}")

        # Session stats as context
        context_parts.append(
            f"Session stats: {runs} runs, {input_tokens:,} input tokens, "
            f"{output_tokens:,} output tokens"
        )

        context = "\n".join(context_parts) if context_parts else "No additional context."

        return self._fill_template(
            session_id=session_id,
            started_at=started_at,
            runs=runs,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            ongoing_work=ongoing_work,
            decisions=decisions,
            next_steps=next_steps,
            context=context,
        )

    # ------------------------------------------------------------------
    # LLM summarization
    # ------------------------------------------------------------------

    def _summarize_with_llm(
        self,
        messages: List[Dict],
        session_id: str,
    ) -> Dict[str, str]:
        """Use Gemini Flash to summarize conversation into handoff parts.

        Returns dict with keys: ongoing_work, decisions, next_steps, context
        """
        if not self.client:
            raise RuntimeError("No Gemini client available")

        # Use only the last N messages to stay within token budget
        recent = messages[-_MAX_MESSAGES_FOR_LLM:] if messages else []

        serialized = self._serialize_for_summary(recent)

        system_prompt = """You are a session handoff assistant. Your job is to summarize a conversation
into a structured handoff note so a colleague can continue the work in the next session.

Return ONLY a valid JSON object with exactly these 4 keys — no markdown, no explanation,
no preamble:

{
  "ongoing_work": "What the user was actively trying to accomplish in this session",
  "decisions": "Key technical decisions made, including why they were made",
  "next_steps": "What needs to happen next to continue the work",
  "context": "Important values, file paths, error messages, or configuration details to preserve"
}

Be specific: include file paths, command outputs, error messages, and concrete values.
If a section has nothing to record, write "None recorded." for that field."""

        try:
            response = self.client.models.generate_content(
                model=_SUMMARY_MODEL,
                contents=[
                    types.Content(
                        role="user",
                        parts=[types.Part(text=serialized)],
                    )
                ],
                config=types.GenerateContentConfig(
                    system_instruction=types.Content(
                        parts=[types.Part(text=system_prompt)]
                    ),
                    response_mime_type="application/json",
                    max_output_tokens=2048,
                    temperature=0.3,
                ),
            )

            text = response.text.strip()
            # Parse JSON from response (handle potential wrapper)
            if text.startswith("```"):
                # Strip markdown code block
                lines = text.split("\n")
                lines = [l for l in lines if not l.startswith("```")]
                text = "\n".join(lines).strip()

            result = json.loads(text)
            # Validate required keys
            for key in ("ongoing_work", "decisions", "next_steps", "context"):
                if key not in result:
                    result[key] = "None recorded."
            return result

        except json.JSONDecodeError as e:
            logger.warning("Failed to parse LLM JSON response: %s — raw: %s", e, text[:500])
            raise RuntimeError(f"LLM returned invalid JSON: {e}")
        except Exception as e:
            logger.warning("LLM summarization failed: %s", e)
            raise RuntimeError(f"LLM call failed: {e}")

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _serialize_for_summary(self, messages: List[Dict]) -> str:
        """Serialize conversation messages into labeled text for the summarizer.

        Truncates long content but preserves structure (role labels, tool calls).
        """
        parts = []
        for msg in messages:
            role = msg.get("role", "unknown")
            content = msg.get("content") or ""

            if role == "tool":
                tool_id = msg.get("tool_call_id", "")
                if len(content) > 2000:
                    content = content[:1000] + "\n...[truncated]...\n" + content[-500:]
                parts.append(f"[TOOL RESULT {tool_id}]: {content}")
                continue

            if role == "assistant":
                if len(content) > 2000:
                    content = content[:1000] + "\n...[truncated]...\n" + content[-500:]
                tool_calls = msg.get("tool_calls") or []
                if tool_calls:
                    tc_parts = []
                    for tc in tool_calls:
                        if isinstance(tc, dict):
                            fn = tc.get("function", {})
                            name = fn.get("name", "?")
                            args = fn.get("arguments", "")
                            if len(args) > 400:
                                args = args[:300] + "..."
                            tc_parts.append(f"  {name}({args})")
                    if tc_parts:
                        content += "\n[Tool calls:\n" + "\n".join(tc_parts) + "\n]"
                parts.append(f"[ASSISTANT]: {content}")
                continue

            # User and other roles
            if len(content) > 2000:
                content = content[:1000] + "\n...[truncated]...\n" + content[-500:]
            parts.append(f"[{role.upper()}]: {content}")

        return "\n\n".join(parts)

    def _fill_template(
        self,
        session_id: str,
        started_at: datetime,
        runs: int,
        input_tokens: int,
        output_tokens: int,
        ongoing_work: str,
        decisions: str,
        next_steps: str,
        context: str,
    ) -> str:
        """Fill in all template placeholders and return the handoff string."""
        # Calculate duration
        now = datetime.now(timezone.utc)
        if started_at.tzinfo is None:
            started_at = started_at.replace(tzinfo=timezone.utc)
        duration = now - started_at
        duration_hours = duration.total_seconds() / 3600

        return _HANDOFF_TEMPLATE.format(
            session_id=session_id,
            started_at=started_at.isoformat(),
            runs=runs,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            duration_hours=duration_hours,
            ongoing_work=ongoing_work,
            decisions=decisions,
            next_steps=next_steps,
            context=context,
        )

    def _load_api_key(self) -> Optional[str]:
        """Load GEMINI_API_KEY from environment."""
        return os.environ.get("GEMINI_API_KEY")


# ------------------------------------------------------------------
# Convenience standalone function
# ------------------------------------------------------------------

def generate_session_handoff(
    session_id: str,
    started_at: datetime,
    runs: int,
    input_tokens: int,
    output_tokens: int,
    messages: List[Dict],
    gemini_api_key: Optional[str] = None,
) -> str:
    """One-shot handoff generation.

    Usage:
        handoff = generate_session_handoff(
            session_id="sess_abc123",
            started_at=datetime(2026, 3, 28, 10, 0, 0),
            runs=5,
            input_tokens=45000,
            output_tokens=12000,
            messages=message_history,
        )
    """
    gen = HandoffGenerator(gemini_api_key=gemini_api_key)
    return gen.generate_handoff(
        session_id=session_id,
        started_at=started_at,
        runs=runs,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        messages=messages,
    )
