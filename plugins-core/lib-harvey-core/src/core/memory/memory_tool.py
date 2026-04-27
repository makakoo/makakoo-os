#!/usr/bin/env python3
"""
Memory Tool — Self-registering memory tool for Harvey OS.

Provides the memory tool API (add/replace/remove) that integrates with Harvey's
ToolRegistry for self-registration at import time.

Source pattern: hermes-agent/tools/memory_tool.py (548 lines)
Key constraint: Uses Harvey's tool registry self-registration pattern
"""

import json
from typing import Any, Optional

from .frozen_memory import MemoryStore

# =============================================================================
# Tool Handler
# =============================================================================


def memory_tool(
    action: str,
    target: str = "memory",
    content: Optional[str] = None,
    old_text: Optional[str] = None,
    store: Optional[MemoryStore] = None,
) -> str:
    """
    Single entry point for the memory tool. Dispatches to MemoryStore methods.

    Returns JSON string with results.
    """
    if store is None:
        return json.dumps(
            {
                "success": False,
                "error": "Memory is not available. It may be disabled in config or this environment.",
            },
            ensure_ascii=False,
        )

    if target not in ("memory", "user"):
        return json.dumps(
            {
                "success": False,
                "error": f"Invalid target '{target}'. Use 'memory' or 'user'.",
            },
            ensure_ascii=False,
        )

    if action == "add":
        if not content:
            return json.dumps(
                {"success": False, "error": "Content is required for 'add' action."},
                ensure_ascii=False,
            )
        result = store.add(target, content)

    elif action == "replace":
        if not old_text:
            return json.dumps(
                {
                    "success": False,
                    "error": "old_text is required for 'replace' action.",
                },
                ensure_ascii=False,
            )
        if not content:
            return json.dumps(
                {
                    "success": False,
                    "error": "content is required for 'replace' action.",
                },
                ensure_ascii=False,
            )
        result = store.replace(target, old_text, content)

    elif action == "remove":
        if not old_text:
            return json.dumps(
                {
                    "success": False,
                    "error": "old_text is required for 'remove' action.",
                },
                ensure_ascii=False,
            )
        result = store.remove(target, old_text)

    else:
        return json.dumps(
            {
                "success": False,
                "error": f"Unknown action '{action}'. Use: add, replace, remove",
            },
            ensure_ascii=False,
        )

    return json.dumps(result, ensure_ascii=False)


def check_memory_requirements() -> bool:
    """Memory tool has no external requirements -- always available."""
    return True


# =============================================================================
# OpenAI Function-Calling Schema
# =============================================================================

MEMORY_SCHEMA = {
    "name": "memory",
    "description": (
        "Save durable information to persistent memory that survives across sessions. "
        "Memory is injected into future turns, so keep it compact and focused on facts "
        "that will still matter later.\n\n"
        "WHEN TO SAVE (do this proactively, don't wait to be asked):\n"
        "- User corrects you or says 'remember this' / 'don't do that again'\n"
        "- User shares a preference, habit, or personal detail (name, role, timezone, coding style)\n"
        "- You discover something about the environment (OS, installed tools, project structure)\n"
        "- You learn a convention, API quirk, or workflow specific to this user's setup\n"
        "- You identify a stable fact that will be useful again in future sessions\n\n"
        "PRIORITY: User preferences and corrections > environment facts > procedural knowledge. "
        "The most valuable memory prevents the user from having to repeat themselves.\n\n"
        "Do NOT save task progress, session outcomes, completed-work logs, or temporary TODO "
        "state to memory; use session_search to recall those from past transcripts.\n"
        "If you've discovered a new way to do something, solved a problem that could be "
        "necessary later, save it as a skill with the skill tool.\n\n"
        "TWO TARGETS:\n"
        "- 'user': who the user is -- name, role, preferences, communication style, pet peeves\n"
        "- 'memory': your notes -- environment facts, project conventions, tool quirks, lessons learned\n\n"
        "ACTIONS: add (new entry), replace (update existing -- old_text identifies it), "
        "remove (delete -- old_text identifies it).\n\n"
        "SKIP: trivial/obvious info, things easily re-discovered, raw data dumps, and temporary task state."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["add", "replace", "remove"],
                "description": "The action to perform.",
            },
            "target": {
                "type": "string",
                "enum": ["memory", "user"],
                "description": "Which memory store: 'memory' for personal notes, 'user' for user profile.",
            },
            "content": {
                "type": "string",
                "description": "The entry content. Required for 'add' and 'replace'.",
            },
            "old_text": {
                "type": "string",
                "description": "Short unique substring identifying the entry to replace or remove.",
            },
        },
        "required": ["action", "target"],
    },
}


# =============================================================================
# Self-Registration with Harvey's Tool Registry
# =============================================================================

from ..registry.tool_registry import registry

registry.register(
    name="memory",
    toolset="memory",
    schema=MEMORY_SCHEMA,
    handler=lambda args, **kw: memory_tool(
        action=args.get("action", ""),
        target=args.get("target", "memory"),
        content=args.get("content"),
        old_text=args.get("old_text"),
        store=kw.get("store"),
    ),
    check_fn=check_memory_requirements,
    emoji="🧠",
)
