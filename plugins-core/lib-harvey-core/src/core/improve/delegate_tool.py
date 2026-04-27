#!/usr/bin/env python3
"""
Delegate Tool — Subagent Architecture for Harvey OS.

Spawns child processes with isolated context, restricted toolsets,
and their own terminal sessions. Supports single-task and batch (parallel)
modes. The parent blocks until all children complete.

Each child gets:
  - A fresh conversation (no parent history)
  - A restricted toolset (configurable, with blocked tools always stripped)
  - A focused system prompt built from the delegated goal + context
  - Depth limiting: children cannot spawn further subagents

The parent's context only sees the delegation call and the summary result,
never the child's intermediate tool calls or reasoning.

Ported from: hermes-agent/tools/delegate_tool.py (794 lines)
Adapted for: Harvey OS
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
import sys
import threading
import time
import uuid
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional

logger = logging.getLogger(__name__)

HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", str(Path.home() / ".harvey")))

DELEGATE_BLOCKED_TOOLS = frozenset(
    [
        "delegate_task",
        "clarify",
        "memory",
        "send_message",
        "execute_code",
    ]
)

MAX_CONCURRENT_CHILDREN = 3
MAX_DEPTH = 2
DEFAULT_MAX_ITERATIONS = 50
DEFAULT_TOOLSETS = ["terminal", "file", "web"]

DELEGATE_SESSIONS_DIR = HARVEY_HOME / "delegate-sessions"


@dataclass
class DelegateTask:
    goal: str
    context: Optional[str] = None
    toolsets: Optional[List[str]] = None
    max_iterations: Optional[int] = None


@dataclass
class DelegateResult:
    session_id: str
    status: str
    summary: str
    duration_seconds: float
    model: Optional[str] = None
    exit_reason: str = "completed"
    api_calls: int = 0
    tokens: Dict[str, int] = field(default_factory=lambda: {"input": 0, "output": 0})
    tool_trace: List[Dict[str, Any]] = field(default_factory=list)
    error: Optional[str] = None


def _resolve_toolset(
    requested_toolsets: Optional[List[str]],
    parent_toolsets: Optional[List[str]] = None,
) -> List[str]:
    """Resolve effective toolsets, stripping blocked ones."""
    blocked = {"delegation", "clarify", "memory", "code_execution"}
    if requested_toolsets:
        available = set(parent_toolsets or DEFAULT_TOOLSETS)
        allowed = [t for t in requested_toolsets if t in available and t not in blocked]
        if not allowed:
            return [
                t for t in (parent_toolsets or DEFAULT_TOOLSETS) if t not in blocked
            ]
        return allowed
    if parent_toolsets:
        return [t for t in parent_toolsets if t not in blocked]
    return [t for t in DEFAULT_TOOLSETS if t not in blocked]


def _build_child_system_prompt(
    goal: str,
    context: Optional[str] = None,
    allowed_tools: Optional[List[str]] = None,
) -> str:
    """Build a focused system prompt for a child agent."""
    parts = [
        "You are a focused subagent working on a specific delegated task.",
        "",
        f"YOUR TASK:\n{goal}",
    ]
    if context and context.strip():
        parts.append(f"\nCONTEXT:\n{context}")
    if allowed_tools:
        parts.append(f"\nAVAILABLE TOOLSETS: {', '.join(allowed_tools)}")
    parts.append(
        "\nComplete this task using the tools available to you. "
        "When finished, provide a clear, concise summary of:\n"
        "- What you did\n"
        "- What you found or accomplished\n"
        "- Any files you created or modified\n"
        "- Any issues encountered\n\n"
        "Be thorough but concise -- your response is returned to the "
        "parent agent as a summary."
    )
    return "\n".join(parts)


def _call_delegate_llm(
    messages: List[dict],
    model: Optional[str] = None,
    max_iterations: int = DEFAULT_MAX_ITERATIONS,
) -> Dict[str, Any]:
    """
    Call LLM via switchAILocal for a delegate child.
    Returns a dict with final_response, api_calls, input_tokens, output_tokens.
    """
    import httpx

    base_url = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1").rstrip("/")
    api_key = os.environ.get("SWITCHAI_KEY", "")
    model = model or os.environ.get("LLM_MODEL", "auto")

    user_message = messages[0]["content"] if messages else "complete the task"
    system_prompt = _build_child_system_prompt(user_message)

    payload: Dict[str, Any] = {
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_message},
        ],
        "temperature": 0.3,
        "max_tokens": 4096,
    }

    last_error = None
    for attempt in range(3):
        try:
            with httpx.Client(timeout=120.0) as client:
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
                usage = result.get("usage", {})
                return {
                    "final_response": content
                    if isinstance(content, str)
                    else str(content),
                    "api_calls": 1,
                    "input_tokens": usage.get("prompt_tokens", 0),
                    "output_tokens": usage.get("completion_tokens", 0),
                }
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


def _run_child_session(
    session_id: str,
    goal: str,
    context: Optional[str],
    allowed_tools: List[str],
    depth: int,
    max_iterations: int,
) -> DelegateResult:
    """
    Run a single delegate child session.
    Called in a thread from the thread pool.
    """
    start_time = time.monotonic()
    session_dir = DELEGATE_SESSIONS_DIR / session_id
    session_dir.mkdir(parents=True, exist_ok=True)

    try:
        messages = [{"role": "user", "content": goal}]
        result = _call_delegate_llm(messages, max_iterations=max_iterations)
        duration = round(time.monotonic() - start_time, 2)

        return DelegateResult(
            session_id=session_id,
            status="completed",
            summary=result.get("final_response", ""),
            duration_seconds=duration,
            model=os.environ.get("LLM_MODEL", "auto"),
            exit_reason="completed",
            api_calls=result.get("api_calls", 0),
            tokens={
                "input": result.get("input_tokens", 0),
                "output": result.get("output_tokens", 0),
            },
            tool_trace=[],
        )

    except Exception as exc:
        duration = round(time.monotonic() - start_time, 2)
        logger.exception(f"[delegate:{session_id}] failed")
        return DelegateResult(
            session_id=session_id,
            status="error",
            summary="",
            duration_seconds=duration,
            exit_reason="error",
            error=str(exc),
        )


def delegate_task(
    goal: Optional[str] = None,
    context: Optional[str] = None,
    toolsets: Optional[List[str]] = None,
    tasks: Optional[List[Dict[str, Any]]] = None,
    max_iterations: Optional[int] = None,
    parent_agent: Optional[Any] = None,
    depth: int = 0,
) -> str:
    """
    Spawn one or more child agents to handle delegated tasks.

    Supports two modes:
      - Single: provide goal (+ optional context, toolsets)
      - Batch:  provide tasks array [{goal, context, toolsets}, ...]

    Returns JSON with results array, one entry per task.
    """
    if depth >= MAX_DEPTH:
        return json.dumps(
            {
                "error": (
                    f"Delegation depth limit reached ({MAX_DEPTH}). "
                    "Subagents cannot spawn further subagents."
                )
            }
        )

    effective_max_iter = max_iterations or DEFAULT_MAX_ITERATIONS

    if tasks and isinstance(tasks, list):
        task_list = tasks[:MAX_CONCURRENT_CHILDREN]
    elif goal and isinstance(goal, str) and goal.strip():
        task_list = [{"goal": goal, "context": context, "toolsets": toolsets}]
    else:
        return json.dumps(
            {"error": "Provide either 'goal' (single task) or 'tasks' (batch)."}
        )

    if not task_list:
        return json.dumps({"error": "No tasks provided."})

    for i, task in enumerate(task_list):
        if not task.get("goal", "").strip():
            return json.dumps({"error": f"Task {i} is missing a 'goal'."})

    overall_start = time.monotonic()
    results: List[Dict[str, Any]] = []

    parent_toolsets = None
    if parent_agent and hasattr(parent_agent, "enabled_toolsets"):
        parent_toolsets = parent_agent.enabled_toolsets

    n_tasks = len(task_list)

    if n_tasks == 1:
        task = task_list[0]
        session_id = f"delegate_{uuid.uuid4().hex[:8]}"
        allowed = _resolve_toolset(task.get("toolsets") or toolsets, parent_toolsets)

        result = _run_child_session(
            session_id=session_id,
            goal=task["goal"],
            context=task.get("context"),
            allowed_tools=allowed,
            depth=depth,
            max_iterations=effective_max_iter,
        )
        results.append(
            {
                "session_id": session_id,
                "status": result.status,
                "summary": result.summary,
                "duration_seconds": result.duration_seconds,
                "model": result.model,
                "exit_reason": result.exit_reason,
                "api_calls": result.api_calls,
                "tokens": result.tokens,
                "tool_trace": result.tool_trace,
                "error": result.error,
            }
        )
    else:
        with ThreadPoolExecutor(max_workers=MAX_CONCURRENT_CHILDREN) as executor:
            futures = {}
            for i, task in enumerate(task_list):
                session_id = f"delegate_{uuid.uuid4().hex[:8]}"
                allowed = _resolve_toolset(
                    task.get("toolsets") or toolsets, parent_toolsets
                )
                future = executor.submit(
                    _run_child_session,
                    session_id=session_id,
                    goal=task["goal"],
                    context=task.get("context"),
                    allowed_tools=allowed,
                    depth=depth,
                    max_iterations=effective_max_iter,
                )
                futures[future] = (i, session_id)

            for future in as_completed(futures):
                idx, session_id = futures[future]
                try:
                    result = future.result()
                except Exception as exc:
                    results.append(
                        {
                            "session_id": session_id,
                            "status": "error",
                            "summary": None,
                            "error": str(exc),
                            "api_calls": 0,
                            "duration_seconds": 0,
                        }
                    )
                    continue

                results.append(
                    {
                        "session_id": session_id,
                        "status": result.status,
                        "summary": result.summary,
                        "duration_seconds": result.duration_seconds,
                        "model": result.model,
                        "exit_reason": result.exit_reason,
                        "api_calls": result.api_calls,
                        "tokens": result.tokens,
                        "tool_trace": result.tool_trace,
                        "error": result.error,
                    }
                )

        results.sort(key=lambda r: r.get("session_id", ""))

    total_duration = round(time.monotonic() - overall_start, 2)

    return json.dumps(
        {
            "results": results,
            "total_duration_seconds": total_duration,
        },
        ensure_ascii=False,
    )


def spawn_delegate(
    goal: str,
    context: Optional[str] = None,
    toolsets: Optional[List[str]] = None,
    max_iterations: Optional[int] = None,
    allowed_tools: Optional[List[str]] = None,
    depth: int = 0,
) -> DelegateResult:
    """
    Programmatic single-task delegate spawn.
    Returns a DelegateResult directly (not JSON string).
    """
    session_id = f"delegate_{uuid.uuid4().hex[:8]}"
    effective_toolsets = toolsets or allowed_tools
    resolved_tools = _resolve_toolset(effective_toolsets)

    result = _run_child_session(
        session_id=session_id,
        goal=goal,
        context=context,
        allowed_tools=resolved_tools,
        depth=depth,
        max_iterations=max_iterations or DEFAULT_MAX_ITERATIONS,
    )
    return result


def list_active_sessions() -> List[Dict[str, Any]]:
    """List all active delegate sessions."""
    if not DELEGATE_SESSIONS_DIR.exists():
        return []

    sessions = []
    for d in DELEGATE_SESSIONS_DIR.iterdir():
        if d.is_dir():
            meta_path = d / "meta.json"
            if meta_path.exists():
                try:
                    sessions.append(json.loads(meta_path.read_text()))
                except Exception:
                    pass
    return sessions


def get_session_status(session_id: str) -> Optional[Dict[str, Any]]:
    """Get status of a specific delegate session."""
    session_dir = DELEGATE_SESSIONS_DIR / session_id
    if not session_dir.exists():
        return None

    meta_path = session_dir / "meta.json"
    if meta_path.exists():
        try:
            return json.loads(meta_path.read_text())
        except Exception:
            pass
    return None


def save_session_meta(session_id: str, data: Dict[str, Any]) -> None:
    """Save session metadata to disk."""
    DELEGATE_SESSIONS_DIR.mkdir(parents=True, exist_ok=True)
    (DELEGATE_SESSIONS_DIR / session_id / "meta.json").write_text(
        json.dumps(data, indent=2)
    )


DELEGATE_TASK_SCHEMA = {
    "name": "delegate_task",
    "description": (
        "Spawn one or more subagents to work on tasks in isolated contexts. "
        "Each subagent gets its own conversation and toolset. "
        "Only the final summary is returned -- intermediate tool results "
        "never enter your context window.\n\n"
        "TWO MODES (one of 'goal' or 'tasks' is required):\n"
        "1. Single task: provide 'goal' (+ optional context, toolsets)\n"
        "2. Batch (parallel): provide 'tasks' array with up to 3 items. "
        "All run concurrently and results are returned together.\n\n"
        "WHEN TO USE delegate_task:\n"
        "- Reasoning-heavy subtasks (debugging, code review, research synthesis)\n"
        "- Tasks that would flood your context with intermediate data\n"
        "- Parallel independent workstreams (research A and B simultaneously)\n\n"
        "WHEN NOT TO USE:\n"
        "- Mechanical multi-step work with no reasoning needed -> use execute_code\n"
        "- Single tool call -> just call the tool directly\n"
        "- Tasks needing user interaction -> subagents cannot use clarify\n\n"
        "IMPORTANT:\n"
        "- Subagents have NO memory of your conversation. Pass all relevant "
        "info (file paths, error messages, constraints) via the 'context' field.\n"
        "- Subagents CANNOT call: delegate_task, clarify, memory, send_message, "
        "execute_code.\n"
        "- Each subagent gets its own session (separate working directory and state).\n"
        "- Results are always returned as an array, one entry per task."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "goal": {
                "type": "string",
                "description": (
                    "What the subagent should accomplish. Be specific and "
                    "self-contained -- the subagent knows nothing about your "
                    "conversation history."
                ),
            },
            "context": {
                "type": "string",
                "description": (
                    "Background information the subagent needs: file paths, "
                    "error messages, project structure, constraints. The more "
                    "specific you are, the better the subagent performs."
                ),
            },
            "toolsets": {
                "type": "array",
                "items": {"type": "string"},
                "description": (
                    "Toolsets to enable for this subagent. "
                    "Default: inherits your enabled toolsets. "
                    "Common patterns: ['terminal', 'file'] for code work, "
                    "['web'] for research, ['terminal', 'file', 'web'] for "
                    "full-stack tasks."
                ),
            },
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "goal": {"type": "string", "description": "Task goal"},
                        "context": {
                            "type": "string",
                            "description": "Task-specific context",
                        },
                        "toolsets": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Toolsets for this specific task",
                        },
                    },
                    "required": ["goal"],
                },
                "maxItems": 3,
                "description": (
                    "Batch mode: up to 3 tasks to run in parallel. Each gets "
                    "its own subagent with isolated context and terminal session. "
                    "When provided, top-level goal/context/toolsets are ignored."
                ),
            },
            "max_iterations": {
                "type": "integer",
                "description": (
                    "Max tool-calling turns per subagent (default: 50). "
                    "Only set lower for simple tasks."
                ),
            },
        },
        "required": [],
    },
}
