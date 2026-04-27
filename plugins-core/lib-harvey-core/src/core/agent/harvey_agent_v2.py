"""
Harvey Agent v2 — Fixed tool-calling loop.

Phase 1 deliverable. Replaces the broken logic in harvey_agent.py with
pi-mono-inspired patterns:

  1. No hard MAX_TOOL_ROUNDS cap that kills multi-step work (raised to 50 sanity cap)
  2. Separate API retry budget (3 attempts, exponential backoff 2s/4s/8s)
  3. Exit loop only when tool_calls is empty — never on finish_reason=="stop"
     alone, since some providers (MiniMax, certain OpenAI-compatible servers)
     set finish_reason="stop" alongside tool_calls
  4. Tool crashes return [tool_error] strings to the LLM so it can react
  5. Optional Olibia personality injection via mascot.py
  6. Optional async offload of long-running tools via async_executor.py

Side-by-side with harvey_agent.py. Gateway chooses which to load via
HARVEY_AGENT_V2 env flag. Delete v1 after Phase 2 stabilizes.
"""

from __future__ import annotations

import json
import logging
import os
import sys
import time
from typing import Any, Callable, Dict, List, Optional

import requests

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

# Reuse the tool registry + dispatch from v1 — no need to duplicate 1000 lines
from core.agent.harvey_agent import (
    HARVEY_TOOLS,
    TOOL_DISPATCH,
    execute_tool,
    render_harvey_tools,
)

log = logging.getLogger("harvey.agent.v2")

# ─────────────────────────────────────────────────────────────────────
# Tuning constants
# ─────────────────────────────────────────────────────────────────────

# Sanity cap only — pi-mono has none. 50 is large enough for any realistic
# multi-step workflow, small enough to prevent true runaways.
MAX_TOOL_ROUNDS = 50

# API retry budget is SEPARATE from tool rounds. Applies to transient HTTP
# failures (429/5xx) and connection errors. Not to 4xx client errors.
MAX_API_RETRIES = 3
BASE_RETRY_DELAY_S = 2.0  # 2s → 4s → 8s
RETRYABLE_STATUS = {429, 500, 502, 503, 504}

# Tools that should offload to async_executor instead of blocking the loop.
# Starts as a static set; Phase 2 makes it declarative per subagent.
ASYNC_TOOL_NAMES = {
    "generate_image",
    "browse_url",
    "superbrain_vector_search",
}


# ─────────────────────────────────────────────────────────────────────
# Harvey Agent v2
# ─────────────────────────────────────────────────────────────────────


class HarveyAgentV2:
    """
    Fixed agentic loop.

    API-compatible with HarveyAgent v1 so gateway can drop it in. Adds
    optional `async_executor` and `olibia` hooks for Phase 1 extras.
    """

    def __init__(
        self,
        llm_url: str = "http://localhost:18080/v1",
        llm_model: str = "auto",
        api_key: str = "",
        max_tokens: int = 4096,
        async_executor: Optional[Any] = None,
        use_olibia: bool = True,
    ):
        self.llm_url = llm_url.rstrip("/")
        self.llm_model = llm_model
        self.api_key = (
            api_key
            or os.environ.get("SWITCHAI_KEY", "")
            or os.environ.get("LLM_API_KEY", "")
        )
        self.max_tokens = max_tokens
        self.async_executor = async_executor
        self.use_olibia = use_olibia

    # ─── Public API ──────────────────────────────────────────────

    def process(
        self,
        message: str,
        history: List[Dict],
        system_prompt: str = "",
        channel: str = "unknown",
    ) -> str:
        """Run a single user message through the fixed tool-calling loop."""
        if self.use_olibia:
            try:
                from core.agent.mascot import Olibia
                system_prompt = Olibia.inject_into_system_prompt(system_prompt or "")
            except Exception as e:
                log.debug(f"Olibia injection skipped: {e}")

        messages: List[Dict] = []
        if system_prompt:
            messages.append({"role": "system", "content": system_prompt})
        for msg in history:
            messages.append({"role": msg["role"], "content": msg["content"]})
        if not history or history[-1].get("content") != message:
            messages.append({"role": "user", "content": message})

        result = self._tool_calling_loop(messages, channel=channel)
        if result is not None:
            return result

        log.info("Tool-calling loop returned None; falling back to no-tools call")
        final = self._call_llm_with_retry(messages, include_tools=False)
        return self._extract_content(final) or "(Agent unreachable)"

    # ─── Tool-calling loop ───────────────────────────────────────

    def _tool_calling_loop(
        self, messages: List[Dict], channel: str = "unknown"
    ) -> Optional[str]:
        msgs: List[Dict] = [m.copy() for m in messages]

        for round_num in range(MAX_TOOL_ROUNDS):
            log.info(f"[v2] Loop round {round_num + 1}/{MAX_TOOL_ROUNDS}")

            response = self._call_llm_with_retry(msgs, include_tools=True)
            if response is None:
                # Signal to caller that transport itself failed. Callers
                # may decide to fall back to a non-tool-calling path.
                return None

            choice = response.get("choices", [{}])[0]
            msg = choice.get("message", {}) or {}
            tool_calls = msg.get("tool_calls") or []
            content = msg.get("content") or ""
            finish_reason = choice.get("finish_reason", "")

            # CRITICAL FIX: exit ONLY when tool_calls is empty.
            # Do not short-circuit on finish_reason=="stop" when tool_calls
            # are present — some providers set both simultaneously.
            if not tool_calls:
                if content:
                    return content
                if finish_reason == "length":
                    return "(Response truncated — token limit reached)"
                if finish_reason == "content_filter":
                    return "(Response filtered by provider)"
                if finish_reason in ("", "stop"):
                    return "(No response generated)"
                return f"(Ended with finish_reason={finish_reason})"

            # Preserve BOTH content and tool_calls in the assistant message
            # so the LLM sees its own prior reasoning in the next round.
            msgs.append(
                {
                    "role": "assistant",
                    "content": content,
                    "tool_calls": tool_calls,
                }
            )

            # Execute each tool call sequentially. Long-running tools may
            # be offloaded via async_executor (if attached) — the LLM still
            # sees a synchronous result string this round (a handle), and
            # the real completion is delivered asynchronously to the user
            # channel by the executor's callback.
            for tc in tool_calls:
                tool_name = (tc.get("function") or {}).get("name", "")
                tool_call_id = tc.get("id") or f"call_{tool_name}_{round_num}"

                result = self._execute_tool_safe(tc, round_num, channel)
                log.info(
                    f"[v2] Tool {tool_name} → {len(result)} chars "
                    f"(round {round_num + 1})"
                )
                msgs.append(
                    {
                        "role": "tool",
                        "content": result,
                        "tool_call_id": tool_call_id,
                    }
                )

        # Sanity cap reached. Ask for a summary with no tools.
        log.warning(f"[v2] Hit MAX_TOOL_ROUNDS={MAX_TOOL_ROUNDS}, asking for summary")
        final = self._call_llm_with_retry(msgs, include_tools=False)
        content = self._extract_content(final)
        return content or "(Agent exceeded tool round sanity cap)"

    # ─── Tool execution (safe) ───────────────────────────────────

    def _execute_tool_safe(
        self, tool_call: Dict, round_num: int, channel: str
    ) -> str:
        """Execute a tool and always return a string, never raise."""
        fn = tool_call.get("function") or {}
        tool_name = fn.get("name", "")
        raw_args = fn.get("arguments", "{}")

        try:
            args = json.loads(raw_args) if isinstance(raw_args, str) else (raw_args or {})
        except (json.JSONDecodeError, TypeError) as e:
            return f"[tool_error] Failed to parse arguments for {tool_name}: {e}"

        # Optional async offload for long-running tools
        if (
            self.async_executor is not None
            and tool_name in ASYNC_TOOL_NAMES
            and hasattr(self.async_executor, "submit")
        ):
            try:
                task_id = self.async_executor.submit(
                    task_id=f"{channel}:{tool_name}:{round_num}:{int(time.time() * 1000)}",
                    fn=execute_tool,
                    args=(tool_name, args),
                )
                return (
                    f"[tool_async] {tool_name} dispatched as task {task_id}. "
                    f"Continue reasoning; the final result will be delivered "
                    f"via callback when ready."
                )
            except Exception as e:
                log.warning(f"[v2] async offload failed for {tool_name}: {e}")
                # Fall through to sync execution

        try:
            return execute_tool(tool_name, args)
        except Exception as e:
            log.exception(f"[v2] Tool {tool_name} raised")
            return f"[tool_error] {tool_name} raised {type(e).__name__}: {e}"

    # ─── LLM transport with retry ────────────────────────────────

    def _call_llm_with_retry(
        self, messages: List[Dict], include_tools: bool = True
    ) -> Optional[dict]:
        """POST to LLM with exponential backoff on transient failures."""
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        payload = {
            "model": self.llm_model,
            "messages": messages,
            "max_tokens": self.max_tokens,
            "temperature": 0.7,
            "stream": False,
        }
        if include_tools:
            # Render per-turn so write_file's description reflects current
            # baseline + active-grant union (Phase C.5).
            payload["tools"] = render_harvey_tools()

        for attempt in range(MAX_API_RETRIES):
            try:
                r = requests.post(
                    f"{self.llm_url}/chat/completions",
                    headers=headers,
                    json=payload,
                    timeout=120,
                )
            except requests.exceptions.ConnectionError:
                if attempt < MAX_API_RETRIES - 1:
                    delay = BASE_RETRY_DELAY_S * (2 ** attempt)
                    log.warning(
                        f"[v2] LLM connection error, retry in {delay:.1f}s "
                        f"(attempt {attempt + 1}/{MAX_API_RETRIES})"
                    )
                    time.sleep(delay)
                    continue
                log.warning(f"[v2] LLM unreachable at {self.llm_url} after retries")
                return None
            except Exception as e:
                log.warning(f"[v2] LLM call error: {e}")
                return None

            if r.status_code == 200:
                data = r.json()
                log.debug(
                    f"[v2] LLM 200 model={data.get('model', '?')} "
                    f"attempt={attempt + 1}"
                )
                return data

            if r.status_code in RETRYABLE_STATUS and attempt < MAX_API_RETRIES - 1:
                delay = BASE_RETRY_DELAY_S * (2 ** attempt)
                log.warning(
                    f"[v2] LLM {r.status_code}, retry in {delay:.1f}s "
                    f"(attempt {attempt + 1}/{MAX_API_RETRIES})"
                )
                time.sleep(delay)
                continue

            # Non-retryable 4xx, or 5xx after all retries exhausted
            log.warning(
                f"[v2] LLM {r.status_code} (final): {r.text[:300]}"
            )
            return None

        return None

    # ─── Helpers ─────────────────────────────────────────────────

    @staticmethod
    def _extract_content(response: Optional[dict]) -> str:
        if not response:
            return ""
        try:
            return (
                response.get("choices", [{}])[0]
                .get("message", {})
                .get("content", "")
                or ""
            )
        except (IndexError, AttributeError):
            return ""


# ─────────────────────────────────────────────────────────────────────
# Drop-in alias so gateway can do `from core.agent.harvey_agent_v2 import HarveyAgent`
# ─────────────────────────────────────────────────────────────────────

HarveyAgent = HarveyAgentV2


__all__ = [
    "HarveyAgentV2",
    "HarveyAgent",
    "MAX_TOOL_ROUNDS",
    "MAX_API_RETRIES",
    "BASE_RETRY_DELAY_S",
    "RETRYABLE_STATUS",
    "ASYNC_TOOL_NAMES",
]
