#!/usr/bin/env python3
"""
Session Archiver — Sprint 3 / Phase 4

Archives compacted sessions to Brain.
Stores handoff summary + optionally the full message history.
Archived sessions can be retrieved by a new session to resume context.

Archive format:
  data/Brain/pages/session_archive/{date}_{session_id}.md
  data/Brain/pages/session_archive/index.json   (session manifest)

Atomic writes via temp file + os.replace() on same filesystem.
Thread-safe via per-session file locks.
"""

import fcntl
import json
import os
import re
import tempfile
import threading
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

BRAIN_DIR = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / "data" / "Brain"
ARCHIVE_DIR = BRAIN_DIR / "pages" / "session_archive"
INDEX_FILE = ARCHIVE_DIR / "index.json"

# Lock timeout (seconds)
_LOCK_TIMEOUT = 10

# ---------------------------------------------------------------------------
# File locking (same pattern as brain_writer.py)
# ---------------------------------------------------------------------------


@contextmanager
def _file_lock(path: Path):
    """Exclusive lock via separate .lock file (never locks the target itself)."""
    lock_path = path.with_suffix(path.suffix + ".lock")
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = open(lock_path, "w")
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        fd.close()


def _atomic_write(path: Path, content: str) -> None:
    """Write content atomically: temp file + os.replace() on same filesystem."""
    try:
        fd, tmp_path = tempfile.mkstemp(
            dir=str(path.parent),
            suffix=".tmp",
            prefix=".sa_",
        )
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(content)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp_path, str(path))
        except BaseException:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass
            raise
    except (OSError, IOError) as e:
        raise RuntimeError(f"Failed to write {path}: {e}")


# ---------------------------------------------------------------------------
# JSON index helpers
# ---------------------------------------------------------------------------


def _read_index() -> Dict[str, Any]:
    """Read the session archive index. Returns empty dict if missing."""
    if not INDEX_FILE.exists():
        return {"sessions": [], "version": 1}
    try:
        return json.loads(INDEX_FILE.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {"sessions": [], "version": 1}


def _write_index(index: Dict[str, Any]) -> None:
    """Atomically write the session archive index."""
    _atomic_write(INDEX_FILE, json.dumps(index, indent=2, ensure_ascii=False))


# ---------------------------------------------------------------------------
# Session ID helpers
# ---------------------------------------------------------------------------


def _session_file_name(session_id: str, ended_at: datetime) -> str:
    """Build the archive filename: {date}_{short_id}.md"""
    date_str = ended_at.strftime("%Y_%m_%d")
    # Use first 8 chars of session_id for brevity
    short_id = session_id.replace("-", "")[:8]
    return f"{date_str}_{short_id}.md"


def _session_date_dir(ended_at: datetime) -> Path:
    """Build the date-based subdirectory path: YYYY/MM/"""
    return ARCHIVE_DIR / ended_at.strftime("%Y") / ended_at.strftime("%m")


# ---------------------------------------------------------------------------
# SessionArchiver
# ---------------------------------------------------------------------------


class SessionArchiver:
    """Archives session state to Brain.

    Session data is stored as:
      session_archive/YYYY/MM/{date}_{short_id}.md   (Brain page)
      session_archive/index.json                       (master manifest)

    Thread-safe: uses a per-instance lock to serialize archive operations.
    """

    def __init__(self, archive_dir: Optional[Path] = None):
        self.archive_dir = archive_dir or ARCHIVE_DIR
        self._id_lock = threading.Lock()
        self._ensure_archive_dir()

    # -------------------------------------------------------------------------
    # Setup
    # -------------------------------------------------------------------------

    def _ensure_archive_dir(self) -> None:
        """Create archive directory and index if they don't exist."""
        self.archive_dir.mkdir(parents=True, exist_ok=True)
        INDEX_FILE.parent.mkdir(parents=True, exist_ok=True)
        if not INDEX_FILE.exists():
            _atomic_write(INDEX_FILE, json.dumps({"sessions": [], "version": 1}, indent=2))

    # -------------------------------------------------------------------------
    # Archive
    # -------------------------------------------------------------------------

    def archive_session(
        self,
        session_id: str,
        started_at: datetime,
        ended_at: datetime,
        runs: int,
        input_tokens: int,
        output_tokens: int,
        handoff_summary: str,
        messages: Optional[List[Dict]] = None,
        metadata: Optional[dict] = None,
    ) -> dict:
        """Archive a compacted session.

        Creates a Brain page in the archive for the session date. The page
        contains the handoff summary and session stats. Full messages are
        stored as a JSON block if provided.

        Thread-safe.

        Args:
            session_id: Unique session identifier (UUID)
            started_at: When session started
            ended_at: When session was compacted
            runs: Total runs in session
            input_tokens: Total input tokens
            output_tokens: Total output tokens
            handoff_summary: The handoff note (from handoff_generator)
            messages: Optional full message history (for search)
            metadata: Optional extra metadata dict

        Returns:
            {"success": True, "archive_id": "...", "path": "..."}
            or {"success": False, "error": "..."}
        """
        if not session_id:
            return {"success": False, "error": "session_id is required"}
        if not handoff_summary:
            return {"success": False, "error": "handoff_summary is required"}

        ended_at = ended_at.astimezone(timezone.utc)
        started_at = started_at.astimezone(timezone.utc)

        file_name = _session_file_name(session_id, ended_at)
        date_dir = _session_date_dir(ended_at)
        file_path = date_dir / file_name

        archive_id = f"{ended_at.strftime('%Y%m%d')}_{session_id[:8]}"

        # Build Brain page content (outliner-formatted)
        page_content = self._build_logseq_page(
            session_id=session_id,
            started_at=started_at,
            ended_at=ended_at,
            runs=runs,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            handoff_summary=handoff_summary,
            messages=messages,
            metadata=metadata,
        )

        # Build index entry
        index_entry = {
            "archive_id": archive_id,
            "session_id": session_id,
            "date": ended_at.strftime("%Y-%m-%d"),
            "file_path": str(file_path),
            "runs": runs,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "handoff_summary": handoff_summary,
            "has_messages": messages is not None,
            "archived_at": datetime.now(timezone.utc).isoformat(),
        }
        if metadata:
            index_entry["metadata"] = metadata

        try:
            # Atomic write of the session page
            date_dir.mkdir(parents=True, exist_ok=True)
            with _file_lock(file_path):
                _atomic_write(file_path, page_content)

            # Atomic update of the index
            with _file_lock(INDEX_FILE):
                index = _read_index()
                # Remove existing entry for same session_id if re-archiving
                index["sessions"] = [
                    s for s in index.get("sessions", []) if s.get("session_id") != session_id
                ]
                index["sessions"].insert(0, index_entry)
                _write_index(index)

            return {
                "success": True,
                "archive_id": archive_id,
                "path": str(file_path),
                "handoff_summary": handoff_summary,
            }

        except (OSError, IOError, RuntimeError) as e:
            return {"success": False, "error": str(e)}

    def _build_logseq_page(
        self,
        session_id: str,
        started_at: datetime,
        ended_at: datetime,
        runs: int,
        input_tokens: int,
        output_tokens: int,
        handoff_summary: str,
        messages: Optional[List[Dict]] = None,
        metadata: Optional[dict] = None,
    ) -> str:
        """Build an outliner-formatted page for a session archive entry."""
        date_str = ended_at.strftime("%Y-%m-%d")
        title = f"Session {date_str} — {session_id[:8]}"

        lines = [
            f"# {title}",
            "",
            "- Archive ID:: " + f"{date_str}_{session_id[:8]}",
            "- Session ID:: " + session_id,
            "- Started:: " + started_at.isoformat(),
            "- Ended:: " + ended_at.isoformat(),
            f"- Runs:: {runs}",
            f"- Input Tokens:: {input_tokens:,}",
            f"- Output Tokens:: {output_tokens:,}",
            f"- Total Tokens:: {input_tokens + output_tokens:,}",
            "",
            "## Handoff Summary",
            "",
            handoff_summary.strip(),
        ]

        if metadata:
            lines.append("")
            lines.append("## Metadata")
            lines.append("")
            for k, v in metadata.items():
                safe_v = json.dumps(v) if not isinstance(v, (str, int, float, bool, type(None))) else str(v)
                lines.append(f"- {k}:: {safe_v}")

        if messages:
            lines.append("")
            lines.append("## Full Messages")
            lines.append("")
            messages_json = json.dumps(messages, indent=2, ensure_ascii=False)
            # Store as a code block to keep it readable
            lines.append("```json")
            lines.append(messages_json)
            lines.append("```")

        lines.append("")
        lines.append("## Archived")
        lines.append(f"- Archived at:: {datetime.now(timezone.utc).isoformat()}")
        lines.append(f"- Source:: session_archiver.py")

        return "\n".join(lines)

    # -------------------------------------------------------------------------
    # Retrieve
    # -------------------------------------------------------------------------

    def get_recent_sessions(self, limit: int = 5) -> List[dict]:
        """Get the N most recent archived sessions (summary only).

        Returns list of session index entries sorted by date descending.
        """
        try:
            with _file_lock(INDEX_FILE):
                index = _read_index()
            sessions = index.get("sessions", [])
            return sessions[:limit]
        except (OSError, IOError):
            return []

    def retrieve_session(self, session_id: str) -> Optional[dict]:
        """Retrieve a specific session's full data by session_id.

        Returns the session dict from the index, or None if not found.
        Does NOT re-read the full message JSON from disk — use
        retrieve_session_messages() for that.
        """
        try:
            with _file_lock(INDEX_FILE):
                index = _read_index()
            for entry in index.get("sessions", []):
                if entry.get("session_id") == session_id:
                    return entry
            return None
        except (OSError, IOError):
            return None

    def retrieve_session_messages(self, session_id: str) -> Optional[List[Dict]]:
        """Retrieve the full message history for a session.

        Reads the JSON messages block from the session's archive file.
        Returns None if the session has no messages or wasn't found.
        """
        entry = self.retrieve_session(session_id)
        if not entry:
            return None
        if not entry.get("has_messages"):
            return None

        file_path = Path(entry["file_path"])
        if not file_path.exists():
            return None

        try:
            with _file_lock(file_path):
                content = file_path.read_text(encoding="utf-8")
        except (OSError, IOError):
            return None

        # Extract the JSON code block between ```json and ```
        match = re.search(r"```json\s*\n(.*?)\n```", content, re.DOTALL)
        if not match:
            return None

        try:
            return json.loads(match.group(1))
        except json.JSONDecodeError:
            return None

    # -------------------------------------------------------------------------
    # Handoff for new sessions
    # -------------------------------------------------------------------------

    def get_handoff_for_new_session(self, limit: int = 3) -> str:
        """Get the most recent handoff summaries to prepend to a new session.

        Returns a formatted string suitable for injection as the start of
        a new session system prompt or conversation context.

        Format:
          ## Recent Sessions

          ### {date} — {short_id} ({runs} runs, {tokens:,} tokens)
          {handoff_summary}

          ---
          [more...]
        """
        sessions = self.get_recent_sessions(limit=limit)
        if not sessions:
            return ""

        parts = ["## Recent Sessions\n"]
        for s in sessions:
            date = s.get("date", "?")
            short_id = s.get("session_id", "?")[:8]
            runs = s.get("runs", 0)
            total_tokens = (s.get("input_tokens", 0) or 0) + (s.get("output_tokens", 0) or 0)
            summary = (s.get("handoff_summary") or "").strip()

            parts.append(f"### {date} — {short_id} ({runs} runs, {total_tokens:,} tokens)\n")
            parts.append(f"{summary}\n")
            parts.append("---\n")

        return "".join(parts).strip()

    # -------------------------------------------------------------------------
    # Utilities
    # -------------------------------------------------------------------------

    def list_archived_dates(self) -> List[str]:
        """List all dates that have archived sessions (newest first)."""
        try:
            with _file_lock(INDEX_FILE):
                index = _read_index()
            dates = list(dict.fromkeys(s.get("date") for s in index.get("sessions", []) if s.get("date")))
            return sorted(dates, reverse=True)
        except (OSError, IOError):
            return []

    def prune_old_sessions(self, keep_last: int = 20) -> dict:
        """Remove all but the last N sessions from the archive.

        Deletes the session files and updates the index.
        Returns {"pruned": N, "remaining": M}.
        """
        try:
            with _file_lock(INDEX_FILE):
                index = _read_index()
                sessions = index.get("sessions", [])

            if len(sessions) <= keep_last:
                return {"pruned": 0, "remaining": len(sessions)}

            to_remove = sessions[keep_last:]
            removed_count = 0

            with _file_lock(INDEX_FILE):
                # Re-read inside lock for the update
                index = _read_index()
                sessions = index.get("sessions", [])

                for old_session in sessions[keep_last:]:
                    file_path = Path(old_session.get("file_path", ""))
                    if file_path.exists():
                        try:
                            file_path.unlink()
                        except OSError:
                            pass
                        removed_count += 1

                index["sessions"] = sessions[:keep_last]
                _write_index(index)

            return {"pruned": removed_count, "remaining": keep_last}

        except (OSError, IOError) as e:
            return {"pruned": 0, "remaining": 0, "error": str(e)}


# ---------------------------------------------------------------------------
# CLI entry point (for testing / manual invocation)
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Session Archiver — Sprint 3")
    sub = parser.add_subparsers(dest="command")

    p_archive = sub.add_parser("archive", help="Archive a session")
    p_archive.add_argument("--session-id", required=True)
    p_archive.add_argument("--started-at", required=True)
    p_archive.add_argument("--ended-at", required=True)
    p_archive.add_argument("--runs", type=int, default=0)
    p_archive.add_argument("--input-tokens", type=int, default=0)
    p_archive.add_argument("--output-tokens", type=int, default=0)
    p_archive.add_argument("--handoff-summary", required=True)
    p_archive.add_argument("--messages-file", type=Path, default=None)

    p_list = sub.add_parser("list", help="List recent sessions")
    p_list.add_argument("--limit", type=int, default=5)

    p_handoff = sub.add_parser("handoff", help="Print handoff for new session")
    p_handoff.add_argument("--limit", type=int, default=3)

    args = parser.parse_args()

    archiver = SessionArchiver()

    if args.command == "archive":
        started = datetime.fromisoformat(args.started_at)
        ended = datetime.fromisoformat(args.ended_at)
        messages = None
        if args.messages_file and args.messages_file.exists():
            messages = json.loads(args.messages_file.read_text(encoding="utf-8"))

        result = archiver.archive_session(
            session_id=args.session_id,
            started_at=started,
            ended_at=ended,
            runs=args.runs,
            input_tokens=args.input_tokens,
            output_tokens=args.output_tokens,
            handoff_summary=args.handoff_summary,
            messages=messages,
        )
        print(json.dumps(result, indent=2))

    elif args.command == "list":
        sessions = archiver.get_recent_sessions(limit=args.limit)
        for s in sessions:
            print(f"{s.get('date')} | {s.get('session_id', '')[:8]} | "
                  f"{s.get('runs')} runs | {s.get('handoff_summary', '')[:60]}...")

    elif args.command == "handoff":
        print(archiver.get_handoff_for_new_session(limit=args.limit))

    else:
        parser.print_help()
