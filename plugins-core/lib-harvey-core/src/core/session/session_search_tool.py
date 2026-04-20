#!/usr/bin/env python3
"""
Session Search Tool - FTS5 Full-Text Search for Session Recall.

Searches past session transcripts in SQLite via FTS5, then summarizes the top
matching sessions using the LLM. Returns focused summaries of past conversations
rather than raw transcripts, keeping the main model's context window clean.

Flow:
  1. FTS5 search finds matching messages ranked by relevance
  2. Groups by session, takes the top N unique sessions (default 3)
  3. Loads each session's conversation, truncates to ~100k chars centered on matches
  4. Sends to LLM with a focused summarization prompt
  5. Returns per-session summaries with metadata

Two modes:
  - Recent sessions (no query): Return titles, previews, timestamps — no LLM cost
  - Keyword search (with query): FTS5 → group → truncate → LLM summarize

Source pattern: hermes-agent/tools/session_search_tool.py (504 lines)
Adapted for Harvey OS with:
  - Harvey's HARVEY_HOME for database path
  - Integration with Harvey's tool registry
  - Harvey's LLM client pattern for summarization
"""

import asyncio
import concurrent.futures
import json
import logging
import os
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

# Harvey imports
from ..registry.tool_registry import registry

# Constants
HARVEY_HOME = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
DEFAULT_SESSIONS_DIR = Path(HARVEY_HOME) / "data" / "sessions"
MAX_SESSION_CHARS = 100_000
MAX_SUMMARY_TOKENS = 10000

# Hidden sources (third-party integrations don't clutter user history)
_HIDDEN_SESSION_SOURCES = ("tool",)


def _format_timestamp(ts: Union[int, float, str, None]) -> str:
    """Convert a Unix timestamp (float/int) or ISO string to human-readable date."""
    if ts is None:
        return "unknown"
    try:
        if isinstance(ts, (int, float)):
            dt = datetime.fromtimestamp(ts)
            return dt.strftime("%B %d, %Y at %I:%M %p")
        if isinstance(ts, str):
            if ts.replace(".", "").replace("-", "").isdigit():
                dt = datetime.fromtimestamp(float(ts))
                return dt.strftime("%B %d, %Y at %I:%M %p")
            return ts
    except (ValueError, OSError, OverflowError):
        pass
    return str(ts)


def _format_conversation(messages: List[Dict[str, Any]]) -> str:
    """Format session messages into a readable transcript for summarization."""
    parts = []
    for msg in messages:
        role = msg.get("role", "unknown").upper()
        content = msg.get("content") or ""
        tool_name = msg.get("tool_name")

        if role == "TOOL" and tool_name:
            if len(content) > 500:
                content = content[:250] + "\n...[truncated]...\n" + content[-250:]
            parts.append(f"[TOOL:{tool_name}]: {content}")
        elif role == "ASSISTANT":
            tool_calls = msg.get("tool_calls")
            if tool_calls and isinstance(tool_calls, list):
                tc_names = []
                for tc in tool_calls:
                    if isinstance(tc, dict):
                        name = tc.get("name") or tc.get("function", {}).get("name", "?")
                        tc_names.append(name)
                if tc_names:
                    parts.append(f"[ASSISTANT]: [Called: {', '.join(tc_names)}]")
                if content:
                    parts.append(f"[ASSISTANT]: {content}")
            else:
                parts.append(f"[ASSISTANT]: {content}")
        else:
            parts.append(f"[{role}]: {content}")

    return "\n\n".join(parts)


def _truncate_around_matches(
    full_text: str, query: str, max_chars: int = MAX_SESSION_CHARS
) -> str:
    """
    Truncate a conversation transcript to max_chars, centered around
    where the query terms appear.
    """
    if len(full_text) <= max_chars:
        return full_text

    query_terms = query.lower().split()
    text_lower = full_text.lower()
    first_match = len(full_text)
    for term in query_terms:
        pos = text_lower.find(term)
        if pos != -1 and pos < first_match:
            first_match = pos

    if first_match == len(full_text):
        first_match = 0

    half = max_chars // 2
    start = max(0, first_match - half)
    end = min(len(full_text), start + max_chars)
    if end - start < max_chars:
        start = max(0, end - max_chars)

    truncated = full_text[start:end]
    prefix = "...[earlier conversation truncated]...\n\n" if start > 0 else ""
    suffix = "\n\n...[later conversation truncated]..." if end < len(full_text) else ""
    return prefix + truncated + suffix


async def _summarize_session(
    conversation_text: str, query: str, session_meta: Dict[str, Any]
) -> Optional[str]:
    """Summarize a single session conversation focused on the search query."""
    system_prompt = (
        "You are reviewing a past conversation transcript to help recall what happened. "
        "Summarize the conversation with a focus on the search topic. Include:\n"
        "1. What the user asked about or wanted to accomplish\n"
        "2. What actions were taken and what the outcomes were\n"
        "3. Key decisions, solutions found, or conclusions reached\n"
        "4. Any specific commands, files, URLs, or technical details that were important\n"
        "5. Anything left unresolved or notable\n\n"
        "Be thorough but concise. Preserve specific details (commands, paths, error messages) "
        "that would be useful to recall. Write in past tense as a factual recap."
    )

    source = session_meta.get("source", "unknown")
    started = _format_timestamp(session_meta.get("started_at"))

    user_prompt = (
        f"Search topic: {query}\n"
        f"Session source: {source}\n"
        f"Session date: {started}\n\n"
        f"CONVERSATION TRANSCRIPT:\n{conversation_text}\n\n"
        f"Summarize this conversation with focus on: {query}"
    )

    max_retries = 3
    for attempt in range(max_retries):
        try:
            # Use Harvey's LLM gateway (switchAI local)
            from ..superbrain.providers import call_llm

            response = await asyncio.wait_for(
                call_llm(
                    model="miniimax",
                    messages=[
                        {"role": "system", "content": system_prompt},
                        {"role": "user", "content": user_prompt},
                    ],
                    temperature=0.1,
                    max_tokens=MAX_SUMMARY_TOKENS,
                ),
                timeout=90.0,
            )

            if response and hasattr(response, "content") and response.content:
                return response.content
            if isinstance(response, dict) and response.get("content"):
                return response["content"]
            if isinstance(response, str):
                return response

            logging.warning(
                "Session search LLM returned empty content (attempt %d/%d)",
                attempt + 1,
                max_retries,
            )
            if attempt < max_retries - 1:
                await asyncio.sleep(1 * (attempt + 1))
                continue
            return None

        except asyncio.TimeoutError:
            logging.warning(
                "Session summarization timed out after 90s (attempt %d/%d)",
                attempt + 1,
                max_retries,
            )
            if attempt < max_retries - 1:
                await asyncio.sleep(1 * (attempt + 1))
                continue
            return None
        except Exception as e:
            logging.warning(
                "Session summarization failed: %s (attempt %d/%d)",
                e,
                attempt + 1,
                max_retries,
                exc_info=True,
            )
            if attempt < max_retries - 1:
                await asyncio.sleep(1 * (attempt + 1))
            else:
                return None


def _list_recent_sessions(db, limit: int, current_session_id: str = None) -> str:
    """Return metadata for the most recent sessions (no LLM calls)."""
    try:
        sessions = db.list_sessions(
            limit=limit + 5, exclude_sources=list(_HIDDEN_SESSION_SOURCES)
        )

        # Resolve current session lineage to exclude it
        current_root = None
        if current_session_id:
            try:
                sid = current_session_id
                visited = set()
                while sid and sid not in visited:
                    visited.add(sid)
                    s = db.get_session(sid)
                    parent = s.get("parent_session_id") if s else None
                    sid = parent if parent else None
                current_root = max(visited, key=len) if visited else current_session_id
            except Exception:
                current_root = current_session_id

        results = []
        for s in sessions:
            sid = s.get("id", "")
            if current_root and (sid == current_root or sid == current_session_id):
                continue
            if s.get("parent_session_id"):
                continue
            results.append(
                {
                    "session_id": sid,
                    "title": s.get("title") or None,
                    "source": s.get("source", ""),
                    "started_at": s.get("started_at", ""),
                    "last_active": s.get("last_active", ""),
                    "message_count": s.get("message_count", 0),
                    "preview": "",
                }
            )
            if len(results) >= limit:
                break

        return json.dumps(
            {
                "success": True,
                "mode": "recent",
                "results": results,
                "count": len(results),
                "message": f"Showing {len(results)} most recent sessions. Use a keyword query to search specific topics.",
            },
            ensure_ascii=False,
        )
    except Exception as e:
        logging.error("Error listing recent sessions: %s", e, exc_info=True)
        return json.dumps(
            {"success": False, "error": f"Failed to list recent sessions: {e}"},
            ensure_ascii=False,
        )


def session_search(
    query: str = "",
    role_filter: str = None,
    limit: int = 3,
    db=None,
    current_session_id: str = None,
) -> str:
    """
    Search past sessions and return focused summaries of matching conversations.

    Uses FTS5 to find matches, then summarizes the top sessions with LLM.
    The current session is excluded from results since the agent already has that context.
    """
    if db is None:
        return json.dumps(
            {"success": False, "error": "Session database not available."},
            ensure_ascii=False,
        )

    limit = min(limit, 5)  # Cap at 5 sessions

    # Recent sessions mode: when query is empty, return metadata only
    if not query or not query.strip():
        return _list_recent_sessions(db, limit, current_session_id)

    query = query.strip()

    try:
        # Parse role filter
        role_list = None
        if role_filter and role_filter.strip():
            role_list = [r.strip() for r in role_filter.split(",") if r.strip()]

        # FTS5 search
        raw_results = db.search_messages(
            query=query,
            role_filter=role_list,
            exclude_sources=list(_HIDDEN_SESSION_SOURCES),
            limit=50,
            offset=0,
        )

        if not raw_results:
            return json.dumps(
                {
                    "success": True,
                    "query": query,
                    "results": [],
                    "count": 0,
                    "message": "No matching sessions found.",
                },
                ensure_ascii=False,
            )

        # Resolve child sessions to parent
        def _resolve_to_parent(session_id: str) -> str:
            visited = set()
            sid = session_id
            while sid and sid not in visited:
                visited.add(sid)
                try:
                    session = db.get_session(sid)
                    if not session:
                        break
                    parent = session.get("parent_session_id")
                    if parent:
                        sid = parent
                    else:
                        break
                except Exception:
                    break
            return sid

        current_lineage_root = (
            _resolve_to_parent(current_session_id) if current_session_id else None
        )

        # Group by resolved session_id
        seen_sessions = {}
        for result in raw_results:
            raw_sid = result["session_id"]
            resolved_sid = _resolve_to_parent(raw_sid)

            if current_lineage_root and resolved_sid == current_lineage_root:
                continue
            if current_session_id and raw_sid == current_session_id:
                continue
            if resolved_sid not in seen_sessions:
                result = dict(result)
                result["session_id"] = resolved_sid
                seen_sessions[resolved_sid] = result
            if len(seen_sessions) >= limit:
                break

        # Prepare sessions for parallel summarization
        tasks = []
        for session_id, match_info in seen_sessions.items():
            try:
                messages = db.get_messages_as_conversation(session_id)
                if not messages:
                    continue
                session_meta = db.get_session(session_id) or {}
                conversation_text = _format_conversation(messages)
                conversation_text = _truncate_around_matches(conversation_text, query)
                tasks.append((session_id, match_info, conversation_text, session_meta))
            except Exception as e:
                logging.warning(
                    "Failed to prepare session %s: %s", session_id, e, exc_info=True
                )

        # Summarize all sessions in parallel
        async def _summarize_all() -> List[Union[str, Exception]]:
            coros = [
                _summarize_session(text, query, meta) for _, _, text, meta in tasks
            ]
            return await asyncio.gather(*coros, return_exceptions=True)

        try:
            # Run in event loop
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)
            try:
                results = loop.run_until_complete(_summarize_all())
            finally:
                loop.close()
        except Exception as e:
            logging.warning("Session summarization failed: %s", e, exc_info=True)
            results = [None] * len(tasks)

        summaries = []
        for (session_id, match_info, conversation_text, _), result in zip(
            tasks, results
        ):
            if isinstance(result, Exception):
                logging.warning(
                    "Failed to summarize session %s: %s", session_id, result
                )
                result = None

            entry = {
                "session_id": session_id,
                "when": _format_timestamp(match_info.get("session_started")),
                "source": match_info.get("source", "unknown"),
                "model": match_info.get("model"),
            }

            if result:
                entry["summary"] = result
            else:
                preview = (
                    (conversation_text[:500] + "\n…[truncated]")
                    if conversation_text
                    else "No preview available."
                )
                entry["summary"] = (
                    f"[Raw preview — summarization unavailable]\n{preview}"
                )

            summaries.append(entry)

        return json.dumps(
            {
                "success": True,
                "query": query,
                "results": summaries,
                "count": len(summaries),
                "sessions_searched": len(seen_sessions),
            },
            ensure_ascii=False,
        )

    except Exception as e:
        logging.error("Session search failed: %s", e, exc_info=True)
        return json.dumps(
            {"success": False, "error": f"Search failed: {str(e)}"}, ensure_ascii=False
        )


def check_session_search_requirements() -> bool:
    """Check that the sessions database directory exists."""
    try:
        return DEFAULT_SESSIONS_DIR.parent.exists()
    except Exception:
        return False


SESSION_SEARCH_SCHEMA = {
    "name": "session_search",
    "description": (
        "Search your long-term memory of past conversations, or browse recent sessions. This is your recall -- "
        "every past session is searchable, and this tool summarizes what happened.\n\n"
        "TWO MODES:\n"
        "1. Recent sessions (no query): Call with no arguments to see what was worked on recently. "
        "Returns titles, previews, and timestamps. Zero LLM cost, instant.\n"
        "2. Keyword search (with query): Search for specific topics across all past sessions. "
        "Returns LLM-generated summaries of matching sessions.\n\n"
        "USE THIS PROACTIVELY when:\n"
        "- The user says 'we did this before', 'remember when', 'last time', 'as I mentioned'\n"
        "- The user asks about a topic you worked on before but don't have in current context\n"
        "- The user references a project, person, or concept that seems familiar but isn't in memory\n"
        "- You want to check if you've solved a similar problem before\n"
        "- The user asks 'what did we do about X?' or 'how did we fix Y?'\n\n"
        "Search syntax: keywords joined with OR for broad recall (elevenlabs OR baseten OR funding), "
        'phrases for exact match ("docker networking"), boolean (python NOT java), prefix (deploy*). '
        "IMPORTANT: Use OR between keywords for best results — FTS5 defaults to AND which misses "
        "sessions that only mention some terms."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query — keywords, phrases, or boolean expressions to find in past sessions. Omit this parameter entirely to browse recent sessions instead.",
            },
            "role_filter": {
                "type": "string",
                "description": "Optional: only search messages from specific roles (comma-separated). E.g. 'user,assistant' to skip tool outputs.",
            },
            "limit": {
                "type": "integer",
                "description": "Max sessions to summarize (default: 3, max: 5).",
                "default": 3,
            },
        },
        "required": [],
    },
}


# Self-registration with Harvey's tool registry
try:
    registry.register(
        name="session_search",
        toolset="session_search",
        schema=SESSION_SEARCH_SCHEMA,
        handler=lambda args, **kw: session_search(
            query=args.get("query", ""),
            role_filter=args.get("role_filter"),
            limit=args.get("limit", 3),
            db=kw.get("db"),
            current_session_id=kw.get("current_session_id"),
        ),
        check_fn=check_session_search_requirements,
        emoji="🔍",
    )
except Exception as e:
    logging.warning("Failed to register session_search tool: %s", e, exc_info=True)


# Exports
__all__ = [
    "session_search",
    "SESSION_SEARCH_SCHEMA",
    "check_session_search_requirements",
]
