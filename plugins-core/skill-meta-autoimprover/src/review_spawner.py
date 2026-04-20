#!/usr/bin/env python3
"""
Background Review Spawner — Auto-Improver Core

Spawns a daemon thread that forks a review agent to review
the conversation for memory/skill opportunities AFTER response delivery.
"""

import json
import os
import sys
import threading
import traceback
from datetime import datetime
from typing import Callable, Dict, List, Literal, Optional, Any

# --------------------------------------------------------------------------- #
# Paths
# --------------------------------------------------------------------------- #

_HARVEY_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
_LLM_BASE_URL = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1")
_LLM_MODEL = os.environ.get("LLM_MODEL", "auto")
_LLM_API_KEY = os.environ.get("LLM_API_KEY", os.environ.get("SWITCHAI_KEY", ""))

# --------------------------------------------------------------------------- #
# Module-level state (set by nudge_triggers.py / the main agent)
# --------------------------------------------------------------------------- #

_review_memory = False
_review_skills = False
_messages_snapshot: List[Dict] = []
_background_review_callback: Optional[Callable] = None
_agent_ref = None  # Will be set by the main agent

# --------------------------------------------------------------------------- #
# REVIEW PROMPTS (from Hermes run_agent.py:1421-1454)
# --------------------------------------------------------------------------- #

_MEMORY_REVIEW_PROMPT = """Review the conversation above and consider saving to memory if appropriate.

Focus on:
1. Has the user revealed things about themselves — their persona, desires,
   preferences, or personal details worth remembering?
2. Has the user expressed expectations about how you should behave, their work
   style, or ways they want you to operate?

If something stands out, save it to the Brain.
If nothing is worth saving, just say 'Nothing to save.' and stop."""

_SKILL_REVIEW_PROMPT = """Review the conversation above and consider saving or updating a skill if appropriate.

Focus on: was a non-trivial approach used to complete a task that required trial
and error, or changing course due to experiential findings along the way, or did
the user expect or desire a different method or outcome?

If a relevant skill already exists, update it with what you learned.
Otherwise, create a new skill if the approach is reusable.
If nothing is worth saving, just say 'Nothing to save.' and stop."""

_COMBINED_REVIEW_PROMPT = """Review the conversation above and consider two things:

**Memory**: Has the user revealed things about themselves — their persona,
desires, preferences, or personal details? Has the user expressed expectations
about how you should behave, their work style, or ways they want you to operate?
If so, save using the Brain tools.

**Skills**: Was a non-trivial approach used to complete a task that required trial
and error, or changing course due to experiential findings along the way, or did
the user expect or desire a different method or outcome? If a relevant skill
already exists, update it. Otherwise, create a new one if the approach is reusable.

Only act if there's something genuinely worth saving.
If nothing stands out, just say 'Nothing to save.' and stop."""

# --------------------------------------------------------------------------- #
# TOOL DEFINITIONS (OpenAI function-calling format for Brain writes)
# --------------------------------------------------------------------------- #

_BRAIN_TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "create_page",
            "description": "Create a new Brain page or overwrite an existing one.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Page name (e.g. 'Sebastian - Profile')"},
                    "properties": {
                        "type": "object",
                        "description": "Page properties (e.g. {'type': 'page'})",
                        "additionalProperties": True,
                    },
                    "content": {"type": "string", "description": "Page body content in markdown"},
                },
                "required": ["name"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "append_block",
            "description": "Append a bullet block to an existing Brain page. Creates the page if it doesn't exist.",
            "parameters": {
                "type": "object",
                "properties": {
                    "page_name": {"type": "string", "description": "The page to append to"},
                    "block_content": {"type": "string", "description": "The bullet content to append"},
                    "properties": {
                        "type": "object",
                        "description": "Optional block properties",
                        "additionalProperties": True,
                    },
                },
                "required": ["page_name", "block_content"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "log_to_today_journal",
            "description": "Append a timestamped entry to today's Brain journal.",
            "parameters": {
                "type": "object",
                "properties": {
                    "block_content": {"type": "string", "description": "The journal entry content"},
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional tags for the entry",
                    },
                },
                "required": ["block_content"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "upsert_property",
            "description": "Update or insert a property on a Brain page.",
            "parameters": {
                "type": "object",
                "properties": {
                    "page_name": {"type": "string", "description": "The page to update"},
                    "key": {"type": "string", "description": "Property key (e.g. 'preference', 'updated')"},
                    "value": {"type": "string", "description": "Property value"},
                },
                "required": ["page_name", "key", "value"],
            },
        },
    },
]

# --------------------------------------------------------------------------- #
# Tool implementations (delegate to brain_bridge)
# --------------------------------------------------------------------------- #

def _init_brain_writer():
    """Lazily import brain_bridge to avoid circular imports."""
    try:
        # brain_bridge lives at core/memory/brain_bridge.py
        _bridge_path = os.path.join(
            _HARVEY_ROOT, "core", "memory", "brain_bridge.py"
        )
        if os.path.exists(_bridge_path):
            import importlib.util
            spec = importlib.util.spec_from_file_location("brain_bridge", _bridge_path)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            return module
    except Exception:
        pass
    return None


def _tool_create_page(arguments: Dict) -> Dict:
    """Execute create_page tool call."""
    bridge = _init_brain_writer()
    if bridge is None:
        return {"success": False, "message": "Brain bridge unavailable", "target": ""}
    name = arguments.get("name", "")
    props = arguments.get("properties", {"type": "page"})
    content = arguments.get("content", "")
    ok = bridge.create_page(name, props, content)
    if ok:
        return {"success": True, "message": f"Created page: {name}", "target": "page"}
    return {"success": False, "message": f"Failed to create page: {name}", "target": "page"}


def _tool_append_block(arguments: Dict) -> Dict:
    """Execute append_block tool call."""
    bridge = _init_brain_writer()
    if bridge is None:
        return {"success": False, "message": "Brain bridge unavailable", "target": ""}
    page_name = arguments.get("page_name", "")
    block_content = arguments.get("block_content", "")
    props = arguments.get("properties")
    ok = bridge.append_block(page_name, block_content, props)
    if ok:
        return {"success": True, "message": f"Entry added to {page_name}", "target": page_name}
    return {"success": False, "message": f"Failed to append block to {page_name}", "target": page_name}


def _tool_log_to_today_journal(arguments: Dict) -> Dict:
    """Execute log_to_today_journal tool call."""
    bridge = _init_brain_writer()
    if bridge is None:
        return {"success": False, "message": "Brain bridge unavailable", "target": ""}
    block_content = arguments.get("block_content", "")
    tags = arguments.get("tags", [])
    ok = bridge.log_to_today_journal(block_content, tags)
    if ok:
        return {"success": True, "message": "Entry added to journal", "target": "journal"}
    return {"success": False, "message": "Failed to log to journal", "target": "journal"}


def _tool_upsert_property(arguments: Dict) -> Dict:
    """Execute upsert_property tool call."""
    bridge = _init_brain_writer()
    if bridge is None:
        return {"success": False, "message": "Brain bridge unavailable", "target": ""}
    page_name = arguments.get("page_name", "")
    key = arguments.get("key", "")
    value = arguments.get("value", "")
    ok = bridge.upsert_property(page_name, key, value)
    if ok:
        return {"success": True, "message": f"Updated {key} on {page_name}", "target": page_name}
    return {"success": False, "message": f"Failed to upsert property {key}", "target": page_name}


_TOOL_IMPLEMENTATIONS = {
    "create_page": _tool_create_page,
    "append_block": _tool_append_block,
    "log_to_today_journal": _tool_log_to_today_journal,
    "upsert_property": _tool_upsert_property,
}


def _execute_tool(tool_name: str, arguments: Dict) -> Dict:
    """Execute a tool by name and return result dict."""
    impl = _TOOL_IMPLEMENTATIONS.get(tool_name)
    if impl is None:
        return {"success": False, "message": f"Unknown tool: {tool_name}", "target": ""}
    try:
        return impl(arguments)
    except Exception as e:
        return {"success": False, "message": f"Tool error: {e}", "target": ""}


# --------------------------------------------------------------------------- #
# LLM Gateway client (same configuration as dispatcher.py)
# --------------------------------------------------------------------------- #

def _make_llm_client():
    """Create OpenAI client pointing at the Harvey LLM gateway."""
    try:
        from openai import OpenAI
        return OpenAI(api_key=_LLM_API_KEY, base_url=_LLM_BASE_URL)
    except Exception:
        # Fallback for env without openai package — use basic httpx approach
        return None


# --------------------------------------------------------------------------- #
# Session context loader (from memory-retrieval)
# --------------------------------------------------------------------------- #

def _load_session_context() -> str:
    """Load pre-session context from Brain via MemoryLoader."""
    try:
        sys.path.insert(0, os.path.join(_HARVEY_ROOT, "core", "memory"))
        from memory_loader import MemoryLoader
        loader = MemoryLoader()
        return loader.load_session_context(available_tokens=4000)
    except Exception:
        return ""


# --------------------------------------------------------------------------- #
# Message scanning helpers (from Hermes run_agent.py:1504-1540)
# --------------------------------------------------------------------------- #

def _scan_for_actions(session_messages: List[Dict]) -> List[str]:
    """Scan review agent messages for successful tool actions."""
    actions = []
    for msg in session_messages:
        if not isinstance(msg, dict):
            continue
        role = msg.get("role", "")
        content = msg.get("content", "")

        # Handle tool result messages (assistant tool calls)
        if role == "tool" or role == "function":
            try:
                if isinstance(content, str):
                    data = json.loads(content)
                else:
                    data = content
            except (json.JSONDecodeError, TypeError):
                continue
            if not isinstance(data, dict):
                continue
            if not data.get("success"):
                continue
            message = data.get("message", "")
            target = data.get("target", "")

            if "created" in message.lower():
                actions.append(message)
            elif "updated" in message.lower():
                actions.append(message)
            elif "added" in message.lower() or (target and "add" in message.lower()):
                label = "Memory" if target == "memory" else "User profile" if target == "user" else target
                actions.append(f"{label} updated")
            elif "Entry added" in message:
                label = "Memory" if target == "memory" else "User profile" if target == "user" else target
                actions.append(f"{label} updated")
            elif "removed" in message.lower() or "replaced" in message.lower():
                label = "Memory" if target == "memory" else "User profile" if target == "user" else target
                actions.append(f"{label} updated")

    return actions


def _build_summary(actions: List[str]) -> str:
    """Build a compact summary from action list (deduplicated, joined)."""
    if not actions:
        return ""
    # Deduplicate while preserving order
    seen = set()
    unique = []
    for a in actions:
        if a not in seen:
            seen.add(a)
            unique.append(a)
    return " · ".join(unique)


# --------------------------------------------------------------------------- #
# Core review loop (runs in daemon thread)
# --------------------------------------------------------------------------- #

_MAX_ITERATIONS = 8


def _run_review(
    messages_snapshot: List[Dict],
    prompt: str,
    callback: Optional[Callable],
) -> None:
    """
    The actual review logic running in the daemon thread.

    Loads session context, then enters an LLM tool-calling loop (max 8 iters).
    Each iteration: LLM decides to use a Brain tool or respond with text.
    Tool results are fed back to the LLM. After the LLM says "Nothing to save."
    or exhausts iterations, we scan for actions and call the callback.
    """
    import contextlib

    session_messages: List[Dict] = []

    # Build the system prompt for the review agent
    session_context = _load_session_context()
    context_block = "[SYSTEM CONTEXT]\n" + session_context if session_context else ""
    system_prompt = f"""You are Harvey's background review agent. Your job is to review
the conversation below and save anything worth remembering to the Brain.

You have access to Brain tools: create_page, append_block, log_to_today_journal, upsert_property.

{context_block}

Be concise. Only write to Brain if there's genuinely something worth saving.
If nothing stands out, say 'Nothing to save.' and nothing else."""

    # Assemble the full message history for the review agent:
    # - System prompt (as first user message for simplicity, or system role)
    # - Conversation snapshot (truncated to last ~40 messages to fit context)
    review_messages: List[Dict] = [
        {"role": "system", "content": system_prompt},
    ]

    # Append conversation snapshot (from main agent)
    # Each message: {role: "user"|"assistant"|"tool", content: str}
    # Truncate if too long (rough: each msg ~200 tokens)
    MAX_SNAPSHOT_MSGS = 40
    snapshot = messages_snapshot[-MAX_SNAPSHOT_MSGS:] if messages_snapshot else []

    for msg in snapshot:
        role = msg.get("role", "user")
        content = msg.get("content", "")
        # Skip empty or very large messages
        if not content or len(str(content)) > 8000:
            continue
        # Map roles if needed (LLM API uses user/assistant/system/tool)
        if role not in ("user", "assistant", "system", "tool", "function"):
            role = "user"
        review_messages.append({"role": role, "content": str(content)[:8000]})

    # Add the review prompt as the final user message
    review_messages.append({"role": "user", "content": prompt})

    # Tool choice state
    client = _make_llm_client()
    if client is None:
        # No LLM available — silently skip
        return

    # Redirect stdout/stderr to /dev/null for the entire review
    with open(os.devnull, "w") as devnull, \
         contextlib.redirect_stdout(devnull), \
         contextlib.redirect_stderr(devnull):

        tool_iteration_count = 0

        for iteration in range(_MAX_ITERATIONS):
            try:
                # Call LLM with tool definitions
                response = client.chat.completions.create(
                    model=_LLM_MODEL,
                    messages=review_messages,
                    tools=_BRAIN_TOOLS,
                    tool_choice="auto",
                    temperature=0.3,  # Lower temperature for deterministic review
                    max_tokens=2048,
                )
            except Exception as e:
                # Silently fail — review is best-effort
                break

            choice = response.choices[0]
            finish_reason = choice.finish_reason
            msg_content = choice.message.content or ""
            tool_calls = choice.message.tool_calls or []

            # Record assistant message
            assistant_msg = {"role": "assistant", "content": msg_content}
            if tool_calls:
                assistant_msg["tool_calls"] = [
                    {"id": tc.id, "function": {"name": tc.function.name, "arguments": tc.function.arguments}}
                    for tc in tool_calls
                ]
            review_messages.append(assistant_msg)

            # Check if LLM decided to respond with text (no tool calls)
            if not tool_calls:
                # LLM gave a text response — check if it said "Nothing to save."
                if "nothing to save" in msg_content.lower().strip():
                    break
                # Otherwise assume it's done talking
                break

            # Execute tool calls
            for tc in tool_calls:
                tool_name = tc.function.name
                raw_args = tc.function.arguments

                # Parse arguments
                try:
                    args = json.loads(raw_args) if isinstance(raw_args, str) else raw_args
                except (json.JSONDecodeError, TypeError):
                    args = {}

                # Execute tool
                result = _execute_tool(tool_name, args)
                result_json = json.dumps(result)

                # Append tool result message
                review_messages.append({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": result_json,
                })

                tool_iteration_count += 1

        # After review loop, scan for successful actions
        actions = _scan_for_actions(review_messages)
        if actions:
            summary = _build_summary(actions)
            cb = callback
            if cb:
                try:
                    cb(f"💾 {summary}")
                except Exception:
                    pass


# --------------------------------------------------------------------------- #
# MAIN SPAWNER
# --------------------------------------------------------------------------- #

def spawn_background_review(
    messages_snapshot: List[Dict],
    review_memory: bool = False,
    review_skills: bool = False,
    callback: Optional[Callable] = None,
) -> None:
    """Spawn a daemon thread to run the background review.

    Args:
        messages_snapshot: Copy of conversation messages for review
        review_memory: Whether to run memory review
        review_skills: Whether to run skill review
        callback: Optional callback to surface results to user
    """
    # Pick the right prompt
    if review_memory and review_skills:
        prompt = _COMBINED_REVIEW_PROMPT
    elif review_memory:
        prompt = _MEMORY_REVIEW_PROMPT
    else:
        prompt = _SKILL_REVIEW_PROMPT

    def _run():
        try:
            _run_review(messages_snapshot, prompt, callback)
        except Exception:
            # Best-effort daemon — never propagate
            pass

    t = threading.Thread(target=_run, daemon=True, name="harvey-bg-review")
    t.start()


# --------------------------------------------------------------------------- #
# Module-level helpers for nudge_triggers.py
# --------------------------------------------------------------------------- #

def set_messages_snapshot(messages: List[Dict]) -> None:
    """Called by main agent to update the messages snapshot."""
    global _messages_snapshot
    _messages_snapshot = list(messages)


def set_callback(cb: Callable) -> None:
    """Called by main agent to set the result callback."""
    global _background_review_callback
    _background_review_callback = cb


def set_review_flags(memory: bool, skills: bool) -> None:
    """Called by nudge_triggers or main agent to set which reviews to run."""
    global _review_memory, _review_skills
    _review_memory = memory
    _review_skills = skills


def trigger_review() -> None:
    """Convenience: spawn review with current module state."""
    if not _review_memory and not _review_skills:
        return
    spawn_background_review(
        messages_snapshot=_messages_snapshot,
        review_memory=_review_memory,
        review_skills=_review_skills,
        callback=_background_review_callback,
    )
