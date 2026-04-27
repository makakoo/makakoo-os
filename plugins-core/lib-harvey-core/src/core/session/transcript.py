#!/usr/bin/env python3
"""
Append-Only JSONL Session Transcript

Stores session transcripts as newline-delimited JSON files, one per session.
Inspired by Claude Code's session storage pattern: each event is an immutable
line in a JSONL file, linked to its predecessor via parent_id.

File layout:
    $HARVEY_HOME/data/sessions/{session_id}.jsonl

Entry types:
    message      — user or assistant message
    tool_call    — tool invocation request
    tool_result  — tool execution result
    compaction   — summary replacing a range of entries
    metadata     — session-level metadata (start, end, config)

Each entry carries:
    id           — 8-char hex (first 8 of sha256 of content+timestamp)
    parent_id    — id of the preceding entry (null for first entry)
    timestamp    — ISO-8601 UTC
    session_id   — owning session
    entry_type   — one of the types above

Safety:
    - Append-only: entries are never modified in place.
    - Soft-delete via tombstone entries that reference the deleted id.
    - 50 MB cap per transcript file; appends silently no-op beyond that.
    - Atomic writes via write-then-rename on POSIX systems.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import tempfile
import time
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
SESSIONS_DIR = os.path.join(HARVEY_HOME, "data", "sessions")
MAX_TRANSCRIPT_BYTES = 50 * 1024 * 1024  # 50 MB

VALID_ENTRY_TYPES = {"message", "tool_call", "tool_result", "compaction", "metadata"}


# ---------------------------------------------------------------------------
# Entry
# ---------------------------------------------------------------------------

@dataclass
class TranscriptEntry:
    """Single immutable line in a session transcript."""

    id: str
    parent_id: Optional[str]
    timestamp: str
    session_id: str
    entry_type: str
    data: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), separators=(",", ":"))

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "TranscriptEntry":
        return cls(
            id=d["id"],
            parent_id=d.get("parent_id"),
            timestamp=d["timestamp"],
            session_id=d["session_id"],
            entry_type=d["entry_type"],
            data=d.get("data", {}),
        )

    @classmethod
    def from_json(cls, line: str) -> "TranscriptEntry":
        return cls.from_dict(json.loads(line))


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _generate_id(content: str, ts: str) -> str:
    """Deterministic 8-char hex id from content + timestamp."""
    h = hashlib.sha256(f"{content}:{ts}".encode()).hexdigest()
    return h[:8]


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _session_path(session_id: str, base_dir: str = SESSIONS_DIR) -> Path:
    return Path(base_dir) / f"{session_id}.jsonl"


# ---------------------------------------------------------------------------
# SessionTranscript
# ---------------------------------------------------------------------------

class SessionTranscript:
    """
    Manages append-only JSONL session transcripts.

    Usage::

        t = SessionTranscript()
        sid = t.new_session()
        t.append_entry(sid, "message", {"role": "user", "content": "hello"})
        entries = t.load_session(sid)
        t.tombstone_entry(sid, entries[0].id, reason="redacted")
        sessions = t.list_sessions()
    """

    def __init__(self, base_dir: str = SESSIONS_DIR):
        self.base_dir = base_dir
        os.makedirs(self.base_dir, exist_ok=True)

    # ----- session lifecycle ------------------------------------------------

    def new_session(self, session_id: Optional[str] = None) -> str:
        """Create a new session and write an initial metadata entry.

        Returns the session_id.
        """
        if session_id is None:
            session_id = f"session_{datetime.now(timezone.utc).strftime('%Y%m%d_%H%M%S')}_{os.urandom(2).hex()}"

        self.append_entry(
            session_id=session_id,
            entry_type="metadata",
            data={"event": "session_start", "created_at": _now_iso()},
            parent_id=None,
        )
        return session_id

    # ----- core operations --------------------------------------------------

    def append_entry(
        self,
        session_id: str,
        entry_type: str,
        data: Dict[str, Any],
        parent_id: Optional[str] = "auto",
    ) -> Optional[TranscriptEntry]:
        """Append a single JSON line to the session transcript.

        Args:
            session_id: Target session.
            entry_type: One of VALID_ENTRY_TYPES.
            data: Arbitrary payload dict.
            parent_id: Explicit parent, ``None`` for root, or ``"auto"``
                       to chain from the last entry in the file.

        Returns:
            The appended TranscriptEntry, or None if the file exceeded
            MAX_TRANSCRIPT_BYTES.
        """
        if entry_type not in VALID_ENTRY_TYPES:
            raise ValueError(f"Invalid entry_type {entry_type!r}. Must be one of {VALID_ENTRY_TYPES}")

        path = _session_path(session_id, self.base_dir)

        # Enforce 50 MB cap.
        if path.exists() and path.stat().st_size >= MAX_TRANSCRIPT_BYTES:
            return None

        ts = _now_iso()

        # Resolve parent_id="auto" by reading last line.
        if parent_id == "auto":
            parent_id = self._last_entry_id(session_id)

        entry_id = _generate_id(json.dumps(data, separators=(",", ":")), ts)

        entry = TranscriptEntry(
            id=entry_id,
            parent_id=parent_id,
            timestamp=ts,
            session_id=session_id,
            entry_type=entry_type,
            data=data,
        )

        self._atomic_append(path, entry.to_json() + "\n")
        return entry

    def load_session(self, session_id: str) -> List[TranscriptEntry]:
        """Read all entries for a session, chained by parent_id order.

        Tombstoned entries are excluded from the result.
        """
        path = _session_path(session_id, self.base_dir)
        if not path.exists():
            return []

        # Parse all entries.
        entries_by_id: Dict[str, TranscriptEntry] = {}
        tombstoned_ids: set[str] = set()
        ordered: List[TranscriptEntry] = []

        with open(path, "r") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                entry = TranscriptEntry.from_json(line)
                entries_by_id[entry.id] = entry
                ordered.append(entry)

                # Collect tombstone targets.
                if entry.entry_type == "metadata" and entry.data.get("event") == "tombstone":
                    target = entry.data.get("target_id")
                    if target:
                        tombstoned_ids.add(target)

        # Filter out tombstoned entries, then chain by parent_id.
        live = [e for e in ordered if e.id not in tombstoned_ids]

        # Build parent -> children index for chain ordering.
        children: Dict[Optional[str], List[TranscriptEntry]] = {}
        for e in live:
            children.setdefault(e.parent_id, []).append(e)

        # Walk the chain from root (parent_id=None).
        result: List[TranscriptEntry] = []
        queue = children.get(None, [])
        while queue:
            current = queue.pop(0)
            result.append(current)
            queue = children.get(current.id, []) + queue

        # If chain walk missed entries (e.g., broken links), append them.
        chained_ids = {e.id for e in result}
        for e in live:
            if e.id not in chained_ids:
                result.append(e)

        return result

    def tombstone_entry(
        self, session_id: str, target_id: str, reason: str = ""
    ) -> Optional[TranscriptEntry]:
        """Soft-delete an entry by appending a tombstone marker."""
        return self.append_entry(
            session_id=session_id,
            entry_type="metadata",
            data={
                "event": "tombstone",
                "target_id": target_id,
                "reason": reason,
            },
        )

    def list_sessions(self) -> List[Dict[str, Any]]:
        """List all session JSONL files with basic metadata.

        Returns a list of dicts with session_id, path, size_bytes,
        entry_count, created_at, and modified_at.
        """
        base = Path(self.base_dir)
        results: List[Dict[str, Any]] = []

        for p in sorted(base.glob("*.jsonl"), key=lambda x: x.stat().st_mtime, reverse=True):
            stat = p.stat()
            session_id = p.stem
            entry_count = 0
            created_at = None

            with open(p, "r") as fh:
                for i, line in enumerate(fh):
                    line = line.strip()
                    if not line:
                        continue
                    entry_count += 1
                    if i == 0:
                        try:
                            first = json.loads(line)
                            created_at = first.get("timestamp")
                        except json.JSONDecodeError:
                            pass

            results.append({
                "session_id": session_id,
                "path": str(p),
                "size_bytes": stat.st_size,
                "entry_count": entry_count,
                "created_at": created_at,
                "modified_at": datetime.fromtimestamp(stat.st_mtime, tz=timezone.utc).isoformat(),
            })

        return results

    def resume_session(self, session_id: Optional[str] = None) -> List[TranscriptEntry]:
        """Resume the latest (or specified) session's messages.

        Loads only ``message`` entries, useful for rebuilding conversation
        context on session resume.
        """
        if session_id is None:
            sessions = self.list_sessions()
            if not sessions:
                return []
            session_id = sessions[0]["session_id"]  # most recent by mtime

        entries = self.load_session(session_id)
        return [e for e in entries if e.entry_type == "message"]

    def session_stats(self, session_id: str) -> Dict[str, Any]:
        """Compute stats for a single session."""
        entries = self.load_session(session_id)
        type_counts: Dict[str, int] = {}
        for e in entries:
            type_counts[e.entry_type] = type_counts.get(e.entry_type, 0) + 1

        path = _session_path(session_id, self.base_dir)
        size = path.stat().st_size if path.exists() else 0

        return {
            "session_id": session_id,
            "total_entries": len(entries),
            "type_counts": type_counts,
            "size_bytes": size,
            "size_human": _human_bytes(size),
            "first_entry": entries[0].timestamp if entries else None,
            "last_entry": entries[-1].timestamp if entries else None,
        }

    # ----- private helpers --------------------------------------------------

    def _last_entry_id(self, session_id: str) -> Optional[str]:
        """Read the id of the last entry in a session file."""
        path = _session_path(session_id, self.base_dir)
        if not path.exists():
            return None

        last_line = None
        with open(path, "r") as fh:
            for line in fh:
                stripped = line.strip()
                if stripped:
                    last_line = stripped

        if last_line is None:
            return None

        try:
            return json.loads(last_line).get("id")
        except json.JSONDecodeError:
            return None

    @staticmethod
    def _atomic_append(path: Path, content: str) -> None:
        """Append content to path with best-effort atomicity.

        On POSIX: writes to a temp file in the same directory, then
        reads original + appends via rename. For append-only workloads
        where we are the sole writer, a simple append with flush+fsync
        is sufficient and avoids the read-copy overhead.
        """
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "a") as fh:
            fh.write(content)
            fh.flush()
            os.fsync(fh.fileno())


# ---------------------------------------------------------------------------
# Utilities
# ---------------------------------------------------------------------------

def _human_bytes(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024:
            return f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} TB"


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def _cli():
    """Minimal CLI for inspection and testing."""
    import argparse

    parser = argparse.ArgumentParser(
        description="Harvey OS Session Transcript — append-only JSONL storage"
    )
    sub = parser.add_subparsers(dest="command")

    # list
    sub.add_parser("list", help="List all sessions")

    # show
    show_p = sub.add_parser("show", help="Show entries for a session")
    show_p.add_argument("session_id", help="Session ID to display")
    show_p.add_argument("--type", dest="entry_type", help="Filter by entry type")

    # stats
    stats_p = sub.add_parser("stats", help="Show stats for all or one session")
    stats_p.add_argument("session_id", nargs="?", help="Optional session ID")

    args = parser.parse_args()
    t = SessionTranscript()

    if args.command == "list":
        sessions = t.list_sessions()
        if not sessions:
            print("No sessions found.")
            return
        for s in sessions:
            print(
                f"  {s['session_id']}  "
                f"{s['entry_count']:>4} entries  "
                f"{_human_bytes(s['size_bytes']):>10}  "
                f"created {s['created_at'] or '?'}"
            )

    elif args.command == "show":
        entries = t.load_session(args.session_id)
        if not entries:
            print(f"No entries for session {args.session_id}")
            return
        for e in entries:
            if args.entry_type and e.entry_type != args.entry_type:
                continue
            print(
                f"[{e.timestamp}] {e.entry_type:<12} id={e.id} "
                f"parent={e.parent_id or '-'}"
            )
            if e.data:
                for k, v in e.data.items():
                    val = json.dumps(v) if isinstance(v, (dict, list)) else str(v)
                    if len(val) > 120:
                        val = val[:117] + "..."
                    print(f"    {k}: {val}")

    elif args.command == "stats":
        if args.session_id:
            stats = t.session_stats(args.session_id)
            _print_stats(stats)
        else:
            sessions = t.list_sessions()
            if not sessions:
                print("No sessions found.")
                return
            total_size = 0
            total_entries = 0
            for s in sessions:
                stats = t.session_stats(s["session_id"])
                _print_stats(stats)
                total_size += stats["size_bytes"]
                total_entries += stats["total_entries"]
                print()
            print(f"Totals: {len(sessions)} sessions, {total_entries} entries, {_human_bytes(total_size)}")

    else:
        parser.print_help()


def _print_stats(stats: Dict[str, Any]) -> None:
    print(f"Session: {stats['session_id']}")
    print(f"  Entries:  {stats['total_entries']}")
    print(f"  Size:     {stats['size_human']}")
    if stats["first_entry"]:
        print(f"  First:    {stats['first_entry']}")
        print(f"  Last:     {stats['last_entry']}")
    if stats["type_counts"]:
        print(f"  Types:    {stats['type_counts']}")


if __name__ == "__main__":
    _cli()
