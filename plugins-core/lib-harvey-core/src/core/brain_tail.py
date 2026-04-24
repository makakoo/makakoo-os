#!/usr/bin/env python3
"""brain_tail — Brain-journal cursor-tailing primitive.

Extracted from core/mcp/http_shim.py during Harvey Octopus generalization
(Phase 1, 2026-04-24). The tool returns Brain-journal lines matching a
substring pattern, newer than a caller-provided cursor.

Wire contract (matches MCP `tools/call` response shape):

    method:   tools/call
    name:     brain_tail
    arguments:
      pattern:            str (required, substring match — NOT regex)
      cursor_date:        str (optional, "YYYY_MM_DD" — default: today)
      cursor_line:        int (optional, default 0) — line number in cursor_date
                          file to start AFTER. 0 returns from line 0.
      include_yesterday:  bool (default true) — if cursor_date is yesterday,
                          include today's file lines after yesterday's are
                          exhausted. Handles midnight rollover cleanly.

Returns a dict like:

    {
      "hits": [
        {"date": "YYYY_MM_DD", "line": N, "text": "...line content...",
         "nonce": "<extracted-or-null>"},
        ...
      ],
      "next_cursor": {"date": "YYYY_MM_DD", "line": N_next},
    }

**Nonce extraction (new in Phase 1):** each journal line is scanned for a
trailing `{nonce=<id>}` token (inserted by the HTTP shim when the write
originated from a signed peer). The extracted `nonce` is returned alongside
the hit so listeners can maintain a nonce-aware LRU cache and drop self-
echoes without relying on a brittle time-window filter. Lines without a
nonce token return `nonce: null` — typical for human-authored Brain entries
written via Logseq or hand edits.
"""

from __future__ import annotations

import datetime
import os
import re

MAKAKOO_HOME = os.environ.get("MAKAKOO_HOME", os.path.expanduser("~/MAKAKOO"))
BRAIN_JOURNALS_DIR = os.path.join(MAKAKOO_HOME, "data", "Brain", "journals")

# Format: trailing ` {nonce=<alnum-id>}` inserted by http_shim on signed
# write. Kept permissive (alnum + hyphen) so UUIDs, ULIDs, and short ids
# all match. Matches only once per line and only at the end — anchoring
# prevents accidental matches on an unrelated `{nonce=...}` literal inside
# a quoted Brain note.
_NONCE_RE = re.compile(r"\{nonce=([A-Za-z0-9\-_]+)\}\s*$")


def _brain_journal_path(day_str: str) -> str:
    return os.path.join(BRAIN_JOURNALS_DIR, f"{day_str}.md")


def _yesterday_str(today_str: str) -> str:
    y = datetime.datetime.strptime(today_str, "%Y_%m_%d").date() - datetime.timedelta(days=1)
    return y.strftime("%Y_%m_%d")


def _read_journal_lines(path: str) -> list[str]:
    try:
        with open(path, "r") as f:
            return f.read().split("\n")
    except FileNotFoundError:
        return []


def extract_nonce(line: str) -> str | None:
    """Return the trailing `{nonce=<id>}` token on a journal line, or None.

    Public helper so the HTTP shim's intercept code and the pod-side
    listener can share one implementation.
    """
    m = _NONCE_RE.search(line)
    return m.group(1) if m else None


def brain_tail(
    pattern: str,
    cursor_date: str | None,
    cursor_line: int,
    include_yesterday: bool,
) -> dict:
    """Tail Brain journals for `pattern`-matching lines newer than the cursor.

    Handles midnight rollover: if `cursor_date` is from yesterday and today
    is different, consumes remaining yesterday lines first, then flips the
    cursor to today at line 0.
    """
    today_str = datetime.date.today().strftime("%Y_%m_%d")
    cursor_date = cursor_date or today_str

    hits: list[dict] = []

    if include_yesterday and cursor_date != today_str:
        y_path = _brain_journal_path(cursor_date)
        y_lines = _read_journal_lines(y_path)
        start = max(0, cursor_line)
        for i, line in enumerate(y_lines[start:], start=start):
            if pattern in line:
                hits.append({
                    "date": cursor_date,
                    "line": i,
                    "text": line,
                    "nonce": extract_nonce(line),
                })
        cursor_line = 0
        cursor_date = today_str

    t_path = _brain_journal_path(today_str)
    t_lines = _read_journal_lines(t_path)
    start = max(0, cursor_line)
    for i, line in enumerate(t_lines[start:], start=start):
        if pattern in line:
            hits.append({
                "date": today_str,
                "line": i,
                "text": line,
                "nonce": extract_nonce(line),
            })

    return {
        "hits": hits,
        "next_cursor": {"date": today_str, "line": len(t_lines)},
    }
