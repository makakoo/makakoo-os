#!/usr/bin/env python3
"""Session-memory observer — extracts durable memories from Claude Code session transcripts.

Runs as a SANCHO task every 30 minutes. Walks all Claude Code session JSONL files,
reads new messages since the last tick (cursor-tracked), normalizes to a plain
transcript, and calls the existing `auto_memory_extractor.extract_memories` with
output targeted at a `drafts/` subdir so the user ratifies before promotion.

Design decisions:
- Draft-first, never auto-promote to live memory. Review via `memory_review.py`.
- Cursor is per-file byte offset. Robust to session file appends (the normal case).
  If a file is truncated, we reset the cursor to 0 and re-scan.
- Only tick when a session has meaningful delta (>= MIN_NEW_MESSAGES user turns).
  Avoids waking the LLM for single-tool-call turns.
- Skips active sessions (modified within last ACTIVE_SESSION_COOLDOWN_S) to avoid
  racing a live conversation. We'd rather lag 10 minutes than poison the current
  session's cache with mid-flight extraction events.

Exit codes:
  0 — clean tick (zero or more drafts written)
  1 — recoverable error (logged, will retry next tick)
  2 — fatal error (missing deps, unreadable state dir)
"""

from __future__ import annotations

import json
import os
import sys
import time
import traceback
from datetime import datetime, timezone
from pathlib import Path

MIN_NEW_MESSAGES = 4
ACTIVE_SESSION_COOLDOWN_S = 600
MAX_TRANSCRIPT_CHARS = 50_000
CURSOR_FILENAME = "cursors.json"


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home).expanduser()
    return Path.home() / "MAKAKOO"


def _claude_projects_dir() -> Path:
    return Path.home() / ".claude" / "projects" / "-Users-sebastian-MAKAKOO"


def _state_dir() -> Path:
    home = _makakoo_home()
    d = home / "state" / "skill-meta-session-memory-observer"
    d.mkdir(parents=True, exist_ok=True)
    return d


def _drafts_dir() -> Path:
    memory_root = Path.home() / ".claude" / "projects" / "-Users-sebastian-MAKAKOO" / "memory"
    d = memory_root / "drafts"
    d.mkdir(parents=True, exist_ok=True)
    return d


def _load_cursors() -> dict:
    path = _state_dir() / CURSOR_FILENAME
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text())
    except (json.JSONDecodeError, OSError):
        return {}


def _save_cursors(cursors: dict) -> None:
    path = _state_dir() / CURSOR_FILENAME
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(cursors, indent=2))
    tmp.replace(path)


def _extract_text_content(message: dict) -> str:
    """Backwards-compat helper used by tests. Delegates to the claude-code adapter."""
    from adapters import _text_from_blocks  # type: ignore
    return _text_from_blocks(message.get("message", {}).get("content", ""))


def _read_new_messages(session_path: Path, cursor_offset: int) -> tuple[list[dict], int]:
    """Read JSONL lines from `cursor_offset` to EOF. Returns (messages, new_offset).

    If the file is shorter than the cursor (truncation/rotation), reset to 0.
    """
    try:
        size = session_path.stat().st_size
    except OSError:
        return [], cursor_offset

    if size < cursor_offset:
        cursor_offset = 0

    messages = []
    with session_path.open("rb") as f:
        f.seek(cursor_offset)
        chunk = f.read()
        new_offset = cursor_offset + len(chunk)

    for line in chunk.decode("utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            messages.append(json.loads(line))
        except json.JSONDecodeError:
            continue

    return messages, new_offset


def _build_transcript(messages: list[dict], adapter=None) -> tuple[str, int]:
    """Normalize messages via an adapter into a plain transcript.

    Returns (text, user_turn_count). If no adapter supplied, falls back to the
    Claude Code parser so existing tests stay green.
    """
    if adapter is None:
        from adapters import ALL_ADAPTERS  # type: ignore
        adapter = ALL_ADAPTERS[0]  # Claude Code

    lines = []
    user_turns = 0
    for m in messages:
        nm = adapter.parse_line(m)
        if nm is None:
            continue
        role = "USER" if nm.role == "user" else "ASSISTANT"
        lines.append(f"## {role}\n{nm.text}")
        if nm.role == "user":
            user_turns += 1

    transcript = "\n\n".join(lines)
    if len(transcript) > MAX_TRANSCRIPT_CHARS:
        transcript = transcript[-MAX_TRANSCRIPT_CHARS:]
    return transcript, user_turns


def _session_is_active(session_path: Path) -> bool:
    """Skip sessions modified within cooldown — a live conversation is still writing."""
    try:
        mtime = session_path.stat().st_mtime
    except OSError:
        return True
    return (time.time() - mtime) < ACTIVE_SESSION_COOLDOWN_S


def _import_extractor():
    """Import the canonical extract_memories function. Injects lib-harvey-core onto path."""
    for candidate in (
        _makakoo_home() / "plugins" / "lib-harvey-core" / "src",
        Path("/Users/sebastian/MAKAKOO/plugins/lib-harvey-core/src"),
    ):
        if candidate.exists() and str(candidate) not in sys.path:
            sys.path.insert(0, str(candidate))
    from core.memory.auto_memory_extractor import extract_memories  # type: ignore
    return extract_memories


def _reclassify(drafts_root: Path) -> int:
    """Apply heuristic type classifier to every draft. Returns count of reclassified files."""
    try:
        HERE = Path(__file__).resolve().parent
        if str(HERE) not in sys.path:
            sys.path.insert(0, str(HERE))
        from classifier import reclassify_all  # type: ignore
    except ImportError:
        return 0
    tally = reclassify_all(drafts_root)
    return sum(tally.values())


def _auto_promote(drafts_root: Path) -> dict:
    """Promote low-risk draft types (user/feedback/reference) straight into live memory."""
    try:
        HERE = Path(__file__).resolve().parent
        if str(HERE) not in sys.path:
            sys.path.insert(0, str(HERE))
        from auto_promoter import auto_promote  # type: ignore
    except ImportError:
        return {}
    return auto_promote(drafts_root)


def _session_label(session_path: Path) -> str:
    """Short label derived from the session filename stem (first 8 chars of the UUID)."""
    return session_path.stem.split("-")[0]


def process_session(
    session_path: Path,
    cursor: int,
    extract_fn,
    drafts_root: Path,
    adapter=None,
) -> tuple[int, int]:
    """Process one session file. Returns (new_cursor, drafts_written)."""
    messages, new_offset = _read_new_messages(session_path, cursor)
    if not messages:
        return new_offset, 0

    transcript, user_turns = _build_transcript(messages, adapter=adapter)
    if user_turns < MIN_NEW_MESSAGES:
        return new_offset, 0

    stamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
    cli_prefix = adapter.name if adapter else "claude"
    label = f"{stamp}_{cli_prefix}_{_session_label(session_path)}"

    try:
        extract_fn(transcript, drafts_root, label)
    except Exception:
        traceback.print_exc()
        return cursor, 0

    return new_offset, 1


def _load_adapters():
    HERE = Path(__file__).resolve().parent
    if str(HERE) not in sys.path:
        sys.path.insert(0, str(HERE))
    from adapters import ALL_ADAPTERS  # type: ignore
    return ALL_ADAPTERS


def tick() -> int:
    try:
        extract_fn = _import_extractor()
    except ImportError as e:
        print(json.dumps({"status": "error", "reason": f"cannot import extractor: {e}"}))
        return 2

    adapters = _load_adapters()
    drafts_root = _drafts_dir()
    cursors = _load_cursors()

    processed_sessions = 0
    drafts_written = 0
    skipped_active = 0
    per_adapter: dict[str, int] = {}

    for adapter in adapters:
        adapter_drafts = 0
        for session_path in adapter.sessions():
            if _session_is_active(session_path):
                skipped_active += 1
                continue

            key = f"{adapter.name}:{session_path}"
            cursor = cursors.get(key, 0)
            new_cursor, drafts = process_session(
                session_path, cursor, extract_fn, drafts_root, adapter=adapter
            )
            cursors[key] = new_cursor
            if drafts:
                drafts_written += drafts
                processed_sessions += 1
                adapter_drafts += drafts
        if adapter_drafts:
            per_adapter[adapter.name] = adapter_drafts

    _save_cursors(cursors)

    reclassified = _reclassify(drafts_root) if drafts_written else 0
    promotion_tally = _auto_promote(drafts_root) if drafts_written else {}

    result = {
        "status": "ok",
        "sessions_processed": processed_sessions,
        "drafts_written": drafts_written,
        "skipped_active": skipped_active,
        "reclassified": reclassified,
        "per_adapter": per_adapter,
        "auto_promoted": promotion_tally,
    }
    print(json.dumps(result))
    return 0


def main() -> int:
    try:
        return tick()
    except Exception:
        traceback.print_exc()
        return 1


if __name__ == "__main__":
    sys.exit(main())
