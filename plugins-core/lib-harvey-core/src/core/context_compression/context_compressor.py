"""
Structured Context Compression for Harvey OS.

Prevents context window exhaustion in long conversations by summarizing
middle turns with a structured template that preserves specific details
(file paths, commands, error messages).

Key features:
  - 7-section structured template (Goal/Constraints/Progress/Decisions/Files/Next Steps/Critical Context)
  - Iterative updates (preserves info across multiple compactions)
  - Token-budget tail protection (~20% of context as tail buffer)
  - Tool output pruning before LLM (cheap pre-pass, no API call)
  - Tool call integrity sanitizer (fixes orphaned tool_call/tool_result pairs)
  - Boundary alignment (never splits assistant+tool_results groups)

Integrates with Harvey's existing switchAILocal LLM gateway — no new client code.
"""

from __future__ import annotations

import logging
import os
import time
from typing import Any, Dict, List, Optional

import httpx

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

SUMMARY_PREFIX = (
    "[CONTEXT COMPACTION] Earlier turns in this conversation were compacted "
    "to save context space. The summary below describes work that was "
    "already completed, and the current session state may still reflect "
    "that work (for example, files may already be changed). Use the summary "
    "and the current state to continue from where things left off, and "
    "avoid repeating work:"
)
LEGACY_SUMMARY_PREFIX = "[CONTEXT SUMMARY]:"

# Minimum tokens for the summary output
_MIN_SUMMARY_TOKENS = 2000
# Proportion of compressed content to allocate for summary
_SUMMARY_RATIO = 0.20
# Absolute ceiling for summary tokens (even on very large context windows)
_SUMMARY_TOKENS_CEILING = 12_000

# Placeholder used when pruning old tool results
_PRUNED_TOOL_PLACEHOLDER = "[Old tool output cleared to save context space]"

# Chars per token rough estimate (standard for English)
_CHARS_PER_TOKEN = 4

# ---------------------------------------------------------------------------
# Model context lengths (fallback when not provided)
# ---------------------------------------------------------------------------

_MODEL_CONTEXT_LENGTHS: Dict[str, int] = {
    "auto": 1000000,
    "claude-sonnet-4-20250514": 200000,
    "claude-3-5-sonnet-20241022": 200000,
    "claude-3-opus-20240229": 200000,
    "gpt-4o": 128000,
    "gpt-4-turbo": 128000,
    "gpt-4": 8192,
    "gpt-3.5-turbo": 16384,
}


def _get_model_context_length(
    model: str, config_context_length: Optional[int] = None
) -> int:
    """Get context length for a model, with configurable override."""
    if config_context_length is not None:
        return config_context_length
    # Check exact match first
    if model in _MODEL_CONTEXT_LENGTHS:
        return _MODEL_CONTEXT_LENGTHS[model]
    # Check prefix match for provider/model patterns
    for known, length in _MODEL_CONTEXT_LENGTHS.items():
        if model.startswith(known.split(":")[0]) and ":" in known:
            return length
        if known in model:
            return length
    # Default fallback for unknown models
    return 100000


def _estimate_messages_tokens_rough(messages: List[Dict[str, Any]]) -> int:
    """Rough token estimate for a message list.

    Uses character count / chars_per_token + overhead for roles/metadata.
    """
    total = 0
    for msg in messages:
        content = msg.get("content") or ""
        total += len(content) // _CHARS_PER_TOKEN
        # Role and metadata overhead (~10 tokens per message)
        total += 10
        # Tool call arguments
        if msg.get("role") == "assistant":
            for tc in msg.get("tool_calls") or []:
                if isinstance(tc, dict):
                    args = tc.get("function", {}).get("arguments") or ""
                    total += len(args) // _CHARS_PER_TOKEN
    return total


# ---------------------------------------------------------------------------
# LLM Client (Harvey's switchAILocal pattern)
# ---------------------------------------------------------------------------


def _call_llm(
    messages: List[Dict[str, Any]],
    model: Optional[str] = None,
    temperature: float = 0.3,
    max_tokens: Optional[int] = None,
) -> str:
    """
    Call MiniMax-M2.7 (or configured model) via switchAILocal (localhost:18080).

    Falls back to RuntimeError if the service is unavailable.
    Retry logic: 3 attempts with brief backoff.
    """
    base_url = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1").rstrip("/")
    api_key = os.environ.get("SWITCHAI_KEY", "")
    default_model = os.environ.get("LLM_MODEL", "auto")

    payload: Dict[str, Any] = {
        "model": model or default_model,
        "messages": messages,
        "temperature": temperature,
    }
    if max_tokens is not None:
        payload["max_tokens"] = max_tokens

    last_error: RuntimeError = RuntimeError("LLM call failed after 3 attempts")
    for attempt in range(3):
        try:
            with httpx.Client(timeout=90.0) as client:
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
            time.sleep(1 * (attempt + 1))

    raise last_error


# ---------------------------------------------------------------------------
# ContextCompressor
# ---------------------------------------------------------------------------


class ContextCompressor:
    """Compresses conversation context when approaching the model's context limit.

    Algorithm:
      1. Prune old tool results >200 chars (cheap pre-pass, no LLM call)
      2. Protect head messages (system prompt + first N exchanges)
      3. Protect tail messages by token budget (~20% of context length)
      4. Summarize middle turns with structured LLM prompt
      5. On subsequent compactions, iteratively update the previous summary
      6. Sanitize tool_call/tool_result pairs (fix orphans)

    Usage:
        compressor = ContextCompressor(
            model="auto",
            threshold_percent=0.50,
        )
        if compressor.should_compress(prompt_tokens):
            compressed = compressor.compress(messages, current_tokens=prompt_tokens)
    """

    def __init__(
        self,
        model: str,
        threshold_percent: float = 0.50,
        protect_first_n: int = 3,
        protect_last_n: int = 20,
        summary_target_ratio: float = 0.20,
        quiet_mode: bool = False,
        summary_model_override: Optional[str] = None,
        config_context_length: Optional[int] = None,
    ):
        """Initialize the context compressor.

        Args:
            model:                   Model name for context length lookup.
            threshold_percent:       When prompt tokens reach this fraction of
                                     context length, trigger compression (0.0-1.0).
            protect_first_n:         Always keep first N messages (head).
            protect_last_n:          Minimum tail messages to protect (fallback).
            summary_target_ratio:    Fraction of threshold tokens for summary budget.
            quiet_mode:              Suppress logging if True.
            summary_model_override:  Use different model for summarization.
            config_context_length:  Override detected context length.
        """
        self.model = model
        self.threshold_percent = threshold_percent
        self.protect_first_n = protect_first_n
        self.protect_last_n = protect_last_n
        self.summary_target_ratio = max(0.10, min(summary_target_ratio, 0.80))
        self.quiet_mode = quiet_mode

        self.context_length = _get_model_context_length(
            model,
            config_context_length=config_context_length,
        )
        self.threshold_tokens = int(self.context_length * threshold_percent)
        self.compression_count = 0

        # Derive token budgets: ratio is relative to the threshold, not total context
        target_tokens = int(self.threshold_tokens * self.summary_target_ratio)
        self.tail_token_budget = target_tokens
        self.max_summary_tokens = min(
            int(self.context_length * 0.05),
            _SUMMARY_TOKENS_CEILING,
        )

        if not quiet_mode:
            logger.info(
                "ContextCompressor: model=%s context_length=%d threshold=%d (%.0f%%) "
                "target_ratio=%.0f%% tail_budget=%d",
                model,
                self.context_length,
                self.threshold_tokens,
                threshold_percent * 100,
                self.summary_target_ratio * 100,
                self.tail_token_budget,
            )

        self.last_prompt_tokens = 0
        self.last_completion_tokens = 0
        self.last_total_tokens = 0

        self.summary_model = summary_model_override or ""

        # Stores the previous compaction summary for iterative updates
        self._previous_summary: Optional[str] = None

    def update_from_response(self, usage: Dict[str, Any]) -> None:
        """Update tracked token usage from API response."""
        self.last_prompt_tokens = usage.get("prompt_tokens", 0)
        self.last_completion_tokens = usage.get("completion_tokens", 0)
        self.last_total_tokens = usage.get("total_tokens", 0)

    def should_compress(self, prompt_tokens: Optional[int] = None) -> bool:
        """Check if context exceeds the compression threshold."""
        tokens = prompt_tokens if prompt_tokens is not None else self.last_prompt_tokens
        return tokens >= self.threshold_tokens

    def should_compress_preflight(self, messages: List[Dict[str, Any]]) -> bool:
        """Quick pre-flight check using rough estimate (before API call)."""
        rough_estimate = _estimate_messages_tokens_rough(messages)
        return rough_estimate >= self.threshold_tokens

    def get_status(self) -> Dict[str, Any]:
        """Get current compression status for display/logging."""
        return {
            "last_prompt_tokens": self.last_prompt_tokens,
            "threshold_tokens": self.threshold_tokens,
            "context_length": self.context_length,
            "usage_percent": min(
                100,
                (self.last_prompt_tokens / self.context_length * 100)
                if self.context_length
                else 0,
            ),
            "compression_count": self.compression_count,
        }

    # ------------------------------------------------------------------
    # Tool output pruning (cheap pre-pass, no LLM call)
    # ------------------------------------------------------------------

    def _prune_old_tool_results(
        self,
        messages: List[Dict[str, Any]],
        protect_tail_count: int,
    ) -> tuple[List[Dict[str, Any]], int]:
        """Replace old tool result contents with a short placeholder.

        Walks backward from the end, protecting the most recent
        ``protect_tail_count`` messages. Older tool results get their
        content replaced with a placeholder string.

        Returns (pruned_messages, pruned_count).
        """
        if not messages:
            return messages, 0

        result = [m.copy() for m in messages]
        pruned = 0
        prune_boundary = len(result) - protect_tail_count

        for i in range(prune_boundary):
            msg = result[i]
            if msg.get("role") != "tool":
                continue
            content = msg.get("content", "")
            if not content or content == _PRUNED_TOOL_PLACEHOLDER:
                continue
            # Only prune if the content is substantial (>200 chars)
            if len(content) > 200:
                result[i] = {**msg, "content": _PRUNED_TOOL_PLACEHOLDER}
                pruned += 1

        return result, pruned

    # ------------------------------------------------------------------
    # Summarization
    # ------------------------------------------------------------------

    def _compute_summary_budget(self, turns_to_summarize: List[Dict[str, Any]]) -> int:
        """Scale summary token budget with the amount of content being compressed.

        The maximum scales with the model's context window (5% of context,
        capped at ``_SUMMARY_TOKENS_CEILING``) so large-context models get
        richer summaries instead of being hard-capped at 8K tokens.
        """
        content_tokens = _estimate_messages_tokens_rough(turns_to_summarize)
        budget = int(content_tokens * _SUMMARY_RATIO)
        return max(_MIN_SUMMARY_TOKENS, min(budget, self.max_summary_tokens))

    def _serialize_for_summary(self, turns: List[Dict[str, Any]]) -> str:
        """Serialize conversation turns into labeled text for the summarizer.

        Includes tool call arguments and result content (up to 3000 chars
        per message) so the summarizer can preserve specific details like
        file paths, commands, and outputs.
        """
        parts = []
        for msg in turns:
            role = msg.get("role", "unknown")
            content = msg.get("content") or ""

            # Tool results: keep more content (3000 chars)
            if role == "tool":
                tool_id = msg.get("tool_call_id", "")
                if len(content) > 3000:
                    content = content[:2000] + "\n...[truncated]...\n" + content[-800:]
                parts.append(f"[TOOL RESULT {tool_id}]: {content}")
                continue

            # Assistant messages: include tool call names AND arguments
            if role == "assistant":
                if len(content) > 3000:
                    content = content[:2000] + "\n...[truncated]...\n" + content[-800:]
                tool_calls = msg.get("tool_calls", [])
                if tool_calls:
                    tc_parts = []
                    for tc in tool_calls:
                        if isinstance(tc, dict):
                            fn = tc.get("function", {})
                            name = fn.get("name", "?")
                            args = fn.get("arguments", "")
                            # Truncate long arguments but keep enough for context
                            if len(args) > 500:
                                args = args[:400] + "..."
                            tc_parts.append(f"  {name}({args})")
                        else:
                            fn = getattr(tc, "function", None)
                            name = getattr(fn, "name", "?") if fn else "?"
                            tc_parts.append(f"  {name}(...)")
                    content += "\n[Tool calls:\n" + "\n".join(tc_parts) + "\n]"
                parts.append(f"[ASSISTANT]: {content}")
                continue

            # User and other roles
            if len(content) > 3000:
                content = content[:2000] + "\n...[truncated]...\n" + content[-800:]
            parts.append(f"[{role.upper()}]: {content}")

        return "\n\n".join(parts)

    def _generate_summary(
        self, turns_to_summarize: List[Dict[str, Any]]
    ) -> Optional[str]:
        """Generate a structured summary of conversation turns.

        Uses a structured template (Goal, Progress, Decisions, Files, Next Steps)
        inspired by Pi-mono and OpenCode. When a previous summary exists,
        generates an iterative update instead of summarizing from scratch.

        Returns None if all attempts fail — the caller should drop
        the middle turns without a summary rather than inject a useless
        placeholder.
        """
        summary_budget = self._compute_summary_budget(turns_to_summarize)
        content_to_summarize = self._serialize_for_summary(turns_to_summarize)

        if self._previous_summary:
            # Iterative update: preserve existing info, add new progress
            prompt = f"""You are updating a context compaction summary. A previous compaction produced the summary below. New conversation turns have occurred since then and need to be incorporated.

PREVIOUS SUMMARY:
{self._previous_summary}

NEW TURNS TO INCORPORATE:
{content_to_summarize}

Update the summary using this exact structure. PRESERVE all existing information that is still relevant. ADD new progress. Move items from "In Progress" to "Done" when completed. Remove information only if it is clearly obsolete.

## Goal
[What the user is trying to accomplish — preserve from previous summary, update if goal evolved]

## Constraints & Preferences
[User preferences, coding style, constraints, important decisions — accumulate across compactions]

## Progress
### Done
[Completed work — include specific file paths, commands run, results obtained]
### In Progress
[Work currently underway]
### Blocked
[Any blockers or issues encountered]

## Key Decisions
[Important technical decisions and why they were made]

## Relevant Files
[Files read, modified, or created — with brief note on each. Accumulate across compactions.]

## Next Steps
[What needs to happen next to continue the work]

## Critical Context
[Any specific values, error messages, configuration details, or data that would be lost without explicit preservation]

Target ~{summary_budget} tokens. Be specific — include file paths, command outputs, error messages, and concrete values rather than vague descriptions.

Write only the summary body. Do not include any preamble or prefix."""
        else:
            # First compaction: summarize from scratch
            prompt = f"""Create a structured handoff summary for a later assistant that will continue this conversation after earlier turns are compacted.

TURNS TO SUMMARIZE:
{content_to_summarize}

Use this exact structure:

## Goal
[What the user is trying to accomplish]

## Constraints & Preferences
[User preferences, coding style, constraints, important decisions]

## Progress
### Done
[Completed work — include specific file paths, commands run, results obtained]
### In Progress
[Work currently underway]
### Blocked
[Any blockers or issues encountered]

## Key Decisions
[Important technical decisions and why they were made]

## Relevant Files
[Files read, modified, or created — with brief note on each]

## Next Steps
[What needs to happen next to continue the work]

## Critical Context
[Any specific values, error messages, configuration details, or data that would be lost without explicit preservation]

Target ~{summary_budget} tokens. Be specific — include file paths, command outputs, error messages, and concrete values rather than vague descriptions. The goal is to prevent the next assistant from repeating work or losing important details.

Write only the summary body. Do not include any preamble or prefix."""

        try:
            call_kwargs: Dict[str, Any] = {
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0.3,
                "max_tokens": summary_budget * 2,
            }
            if self.summary_model:
                call_kwargs["model"] = self.summary_model
            content = _call_llm(**call_kwargs)
            summary = content.strip()
            # Store for iterative updates on next compaction
            self._previous_summary = summary
            return self._with_summary_prefix(summary)
        except RuntimeError:
            logger.warning(
                "Context compression: no provider available for summary. "
                "Middle turns will be dropped without summary."
            )
            return None
        except Exception as e:
            logger.warning("Failed to generate context summary: %s", e)
            return None

    @staticmethod
    def _with_summary_prefix(summary: str) -> str:
        """Normalize summary text to the current compaction handoff format."""
        text = (summary or "").strip()
        for prefix in (LEGACY_SUMMARY_PREFIX, SUMMARY_PREFIX):
            if text.startswith(prefix):
                text = text[len(prefix) :].lstrip()
                break
        return f"{SUMMARY_PREFIX}\n{text}" if text else SUMMARY_PREFIX

    # ------------------------------------------------------------------
    # Tool-call / tool-result pair integrity helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _get_tool_call_id(tc) -> str:
        """Extract the call ID from a tool_call entry (dict or object)."""
        if isinstance(tc, dict):
            return tc.get("id", "")
        return getattr(tc, "id", "") or ""

    def _sanitize_tool_pairs(
        self, messages: List[Dict[str, Any]]
    ) -> List[Dict[str, Any]]:
        """Fix orphaned tool_call / tool_result pairs after compression.

        Two failure modes:
        1. A tool *result* references a call_id whose assistant tool_call was
           removed (summarized/truncated). The API rejects this with
           "No tool call found for function call output with call_id ...".
        2. An assistant message has tool_calls whose results were dropped.
           The API rejects this because every tool_call must be followed by
           a tool result with the matching call_id.

        This method removes orphaned results and inserts stub results for
        orphaned calls so the message list is always well-formed.
        """
        surviving_call_ids: set = set()
        for msg in messages:
            if msg.get("role") == "assistant":
                for tc in msg.get("tool_calls") or []:
                    cid = self._get_tool_call_id(tc)
                    if cid:
                        surviving_call_ids.add(cid)

        result_call_ids: set = set()
        for msg in messages:
            if msg.get("role") == "tool":
                cid = msg.get("tool_call_id")
                if cid:
                    result_call_ids.add(cid)

        # 1. Remove tool results whose call_id has no matching assistant tool_call
        orphaned_results = result_call_ids - surviving_call_ids
        if orphaned_results:
            messages = [
                m
                for m in messages
                if not (
                    m.get("role") == "tool"
                    and m.get("tool_call_id") in orphaned_results
                )
            ]
            if not self.quiet_mode:
                logger.info(
                    "Compression sanitizer: removed %d orphaned tool result(s)",
                    len(orphaned_results),
                )

        # 2. Add stub results for assistant tool_calls whose results were dropped
        missing_results = surviving_call_ids - result_call_ids
        if missing_results:
            patched: List[Dict[str, Any]] = []
            for msg in messages:
                patched.append(msg)
                if msg.get("role") == "assistant":
                    for tc in msg.get("tool_calls") or []:
                        cid = self._get_tool_call_id(tc)
                        if cid in missing_results:
                            patched.append(
                                {
                                    "role": "tool",
                                    "content": "[Result from earlier conversation — see context summary above]",
                                    "tool_call_id": cid,
                                }
                            )
            messages = patched
            if not self.quiet_mode:
                logger.info(
                    "Compression sanitizer: added %d stub tool result(s)",
                    len(missing_results),
                )

        return messages

    def _align_boundary_forward(
        self,
        messages: List[Dict[str, Any]],
        idx: int,
    ) -> int:
        """Push a compress-start boundary forward past any orphan tool results.

        If ``messages[idx]`` is a tool result, slide forward until we hit a
        non-tool message so we don't start the summarised region mid-group.
        """
        while idx < len(messages) and messages[idx].get("role") == "tool":
            idx += 1
        return idx

    def _align_boundary_backward(
        self,
        messages: List[Dict[str, Any]],
        idx: int,
    ) -> int:
        """Pull a compress-end boundary backward to avoid splitting a
        tool_call / result group.

        If the boundary falls in the middle of a tool-result group (i.e.
        there are consecutive tool messages before ``idx``), walk backward
        past all of them to find the parent assistant message. If found,
        move the boundary before the assistant so the entire
        assistant + tool_results group is included in the summarised region
        rather than being split (which causes silent data loss when
        ``_sanitize_tool_pairs`` removes the orphaned tail results).
        """
        if idx <= 0 or idx >= len(messages):
            return idx
        # Walk backward past consecutive tool results
        check = idx - 1
        while check >= 0 and messages[check].get("role") == "tool":
            check -= 1
        # If we landed on the parent assistant with tool_calls, pull the
        # boundary before it so the whole group gets summarised together.
        if (
            check >= 0
            and messages[check].get("role") == "assistant"
            and messages[check].get("tool_calls")
        ):
            idx = check
        return idx

    # ------------------------------------------------------------------
    # Tail protection by token budget
    # ------------------------------------------------------------------

    def _find_tail_cut_by_tokens(
        self,
        messages: List[Dict[str, Any]],
        head_end: int,
        token_budget: Optional[int] = None,
    ) -> int:
        """Walk backward from the end of messages, accumulating tokens until
        the budget is reached. Returns the index where the tail starts.

        ``token_budget`` defaults to ``self.tail_token_budget`` which is
        derived from ``summary_target_ratio * context_length``, so it
        scales automatically with the model's context window.

        Never cuts inside a tool_call/result group. Falls back to the old
        ``protect_last_n`` if the budget would protect fewer messages.
        """
        if token_budget is None:
            token_budget = self.tail_token_budget
        n = len(messages)
        min_tail = self.protect_last_n
        accumulated = 0
        cut_idx = n  # start from beyond the end

        for i in range(n - 1, head_end - 1, -1):
            msg = messages[i]
            content = msg.get("content") or ""
            msg_tokens = len(content) // _CHARS_PER_TOKEN + 10  # +10 for role/metadata
            # Include tool call arguments in estimate
            for tc in msg.get("tool_calls") or []:
                if isinstance(tc, dict):
                    args = tc.get("function", {}).get("arguments", "")
                    msg_tokens += len(args) // _CHARS_PER_TOKEN
            if accumulated + msg_tokens > token_budget and (n - i) >= min_tail:
                break
            accumulated += msg_tokens
            cut_idx = i

        # Ensure we protect at least protect_last_n messages
        fallback_cut = n - min_tail
        if cut_idx > fallback_cut:
            cut_idx = fallback_cut

        # If the token budget would protect everything (small conversations),
        # fall back to the fixed protect_last_n approach so compression can
        # still remove middle turns.
        if cut_idx <= head_end:
            cut_idx = fallback_cut

        # Align to avoid splitting tool groups
        cut_idx = self._align_boundary_backward(messages, cut_idx)

        return max(cut_idx, head_end + 1)

    # ------------------------------------------------------------------
    # Main compression entry point
    # ------------------------------------------------------------------

    def compress(
        self,
        messages: List[Dict[str, Any]],
        current_tokens: Optional[int] = None,
    ) -> List[Dict[str, Any]]:
        """Compress conversation messages by summarizing middle turns.

        Algorithm:
          1. Prune old tool results (cheap pre-pass, no LLM call)
          2. Protect head messages (system prompt + first exchange)
          3. Find tail boundary by token budget (~20% of context as recent buffer)
          4. Summarize middle turns with structured LLM prompt
          5. On re-compression, iteratively update the previous summary

        After compression, orphaned tool_call / tool_result pairs are cleaned
        up so the API never receives mismatched IDs.

        Returns the compressed message list.
        """
        n_messages = len(messages)
        if n_messages <= self.protect_first_n + self.protect_last_n + 1:
            if not self.quiet_mode:
                logger.warning(
                    "Cannot compress: only %d messages (need > %d)",
                    n_messages,
                    self.protect_first_n + self.protect_last_n + 1,
                )
            return messages

        display_tokens = (
            current_tokens
            if current_tokens
            else self.last_prompt_tokens or _estimate_messages_tokens_rough(messages)
        )

        # Phase 1: Prune old tool results (cheap, no LLM call)
        messages, pruned_count = self._prune_old_tool_results(
            messages,
            protect_tail_count=self.protect_last_n * 3,
        )
        if pruned_count and not self.quiet_mode:
            logger.info("Pre-compression: pruned %d old tool result(s)", pruned_count)

        # Phase 2: Determine boundaries
        compress_start = self.protect_first_n
        compress_start = self._align_boundary_forward(messages, compress_start)

        # Use token-budget tail protection instead of fixed message count
        compress_end = self._find_tail_cut_by_tokens(messages, compress_start)

        if compress_start >= compress_end:
            return messages

        turns_to_summarize = messages[compress_start:compress_end]

        if not self.quiet_mode:
            logger.info(
                "Context compression triggered (%d tokens >= %d threshold)",
                display_tokens,
                self.threshold_tokens,
            )
            logger.info(
                "Model context limit: %d tokens (%.0f%% = %d)",
                self.context_length,
                self.threshold_percent * 100,
                self.threshold_tokens,
            )
            tail_msgs = n_messages - compress_end
            logger.info(
                "Summarizing turns %d-%d (%d turns), protecting %d head + %d tail messages",
                compress_start + 1,
                compress_end,
                len(turns_to_summarize),
                compress_start,
                tail_msgs,
            )

        # Phase 3: Generate structured summary
        summary = self._generate_summary(turns_to_summarize)

        # Phase 4: Assemble compressed message list
        compressed: List[Dict[str, Any]] = []
        for i in range(compress_start):
            msg = messages[i].copy()
            if i == 0 and msg.get("role") == "system" and self.compression_count == 0:
                msg["content"] = (
                    (msg.get("content") or "")
                    + "\n\n[Note: Some earlier conversation turns have been compacted "
                    "into a handoff summary to preserve context space. The current session "
                    "state may still reflect earlier work, so build on that summary and "
                    "state rather than re-doing work.]"
                )
            compressed.append(msg)

        _merge_summary_into_tail = False
        if summary:
            last_head_role = (
                messages[compress_start - 1].get("role", "user")
                if compress_start > 0
                else "user"
            )
            first_tail_role = (
                messages[compress_end].get("role", "user")
                if compress_end < n_messages
                else "user"
            )
            # Pick a role that avoids consecutive same-role with both neighbors.
            # Priority: avoid colliding with head (already committed), then tail.
            if last_head_role in ("assistant", "tool"):
                summary_role = "user"
            else:
                summary_role = "assistant"
            # If the chosen role collides with the tail AND flipping wouldn't
            # collide with the head, flip it.
            if summary_role == first_tail_role:
                flipped = "assistant" if summary_role == "user" else "user"
                if flipped != last_head_role:
                    summary_role = flipped
                else:
                    # Both roles would create consecutive same-role messages
                    # (e.g. head=assistant, tail=user — neither role works).
                    # Merge the summary into the first tail message instead
                    # of inserting a standalone message that breaks alternation.
                    _merge_summary_into_tail = True
            if not _merge_summary_into_tail:
                compressed.append({"role": summary_role, "content": summary})
        else:
            if not self.quiet_mode:
                logger.warning(
                    "No summary model available — middle turns dropped without summary"
                )

        for i in range(compress_end, n_messages):
            msg = messages[i].copy()
            if _merge_summary_into_tail and i == compress_end:
                original = msg.get("content") or ""
                msg["content"] = (summary or "") + "\n\n" + original
                _merge_summary_into_tail = False
            compressed.append(msg)

        self.compression_count += 1

        compressed = self._sanitize_tool_pairs(compressed)

        if not self.quiet_mode:
            new_estimate = _estimate_messages_tokens_rough(compressed)
            saved_estimate = display_tokens - new_estimate
            logger.info(
                "Compressed: %d -> %d messages (~%d tokens saved)",
                n_messages,
                len(compressed),
                saved_estimate,
            )
            logger.info("Compression #%d complete", self.compression_count)

        return compressed


# ---------------------------------------------------------------------------
# Convenience function
# ---------------------------------------------------------------------------


def compress_conversation(
    messages: List[Dict[str, Any]],
    model: Optional[str] = None,
    threshold_percent: float = 0.50,
    current_tokens: Optional[int] = None,
) -> List[Dict[str, Any]]:
    """One-shot conversation compression.

    Convenience function that instantiates a ContextCompressor,
    checks if compression is needed, and returns compressed messages.

    Args:
        messages:          List of conversation messages.
        model:             Model name. Defaults to LLM_MODEL env var.
        threshold_percent: Compression threshold (0.0-1.0).
        current_tokens:    Current token count (optional, estimated if not provided).

    Returns:
        Compressed message list (may be unchanged if below threshold).
    """
    import os as _os

    actual_model = model or _os.environ.get("LLM_MODEL", "auto")
    compressor = ContextCompressor(
        model=actual_model,
        threshold_percent=threshold_percent,
        quiet_mode=True,  # Silent for convenience usage
    )

    tokens = current_tokens
    if tokens is None:
        tokens = compressor.last_prompt_tokens

    if not compressor.should_compress(tokens):
        return messages

    return compressor.compress(messages, current_tokens=tokens)
