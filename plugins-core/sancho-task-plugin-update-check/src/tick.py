"""
Daily plugin update-check tick.

Opt-in, report-only. Shells `makakoo plugin outdated --json`, appends one
Brain journal line per drifted plugin, never auto-installs. Sebastian
runs `makakoo plugin update <name>` manually after reviewing the diff.

Journal format (locked — matches SPRINT §5.C.4):
    - [[Harvey]] plugin update available: <name> @ <short-sha> → <short-sha> (manifest|content drift). Run `makakoo plugin update <name>`.

Exit codes:
    0 — ran cleanly (regardless of whether any drift was found)
    2 — `makakoo` binary missing
    3 — `makakoo plugin outdated` returned a non-zero exit
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home).expanduser()
    return Path.home() / "MAKAKOO"


def _today_journal(home: Path) -> Path:
    today = datetime.now(timezone.utc).strftime("%Y_%m_%d")
    journals = home / "data" / "Brain" / "journals"
    journals.mkdir(parents=True, exist_ok=True)
    return journals / f"{today}.md"


def _append_journal(path: Path, line: str) -> None:
    """Append a single outliner line. Journal files are Logseq-outliner
    format: every line starts with `- `. Caller passes the full line."""
    if not line.endswith("\n"):
        line += "\n"
    with open(path, "a", encoding="utf-8") as f:
        f.write(line)


def _short(sha: str | None) -> str:
    if not sha:
        return "(new)"
    return sha[:7]


def _render_line(row: dict) -> str | None:
    """Turn one JSON row from `plugin outdated` into a journal line, or
    None if the plugin is up-to-date."""
    # Error rows come first — they have no `drift` field because the
    # probe never resolved an upstream sha.
    if row.get("error"):
        return (
            f"- [[Harvey]] plugin update check failed: "
            f"{row.get('name', '?')} — {row['error']}"
        )
    if not row.get("drift"):
        return None
    drift_type = row.get("drift_type", "drift")
    return (
        f"- [[Harvey]] plugin update available: "
        f"{row.get('name', '?')} @ {_short(row.get('current'))} → "
        f"{_short(row.get('upstream'))} "
        f"({drift_type}). "
        f"Run `makakoo plugin update {row.get('name', '?')}`."
    )


def main() -> int:
    makakoo = shutil.which("makakoo") or os.environ.get("MAKAKOO_BIN")
    if not makakoo:
        print("sancho-task-plugin-update-check: makakoo binary not on PATH", file=sys.stderr)
        return 2

    result = subprocess.run(
        [makakoo, "plugin", "outdated", "--json"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    if result.returncode != 0:
        print(
            f"sancho-task-plugin-update-check: `makakoo plugin outdated` exit={result.returncode}: "
            f"{result.stderr.strip()}",
            file=sys.stderr,
        )
        return 3

    try:
        rows = json.loads(result.stdout or "[]")
    except json.JSONDecodeError as e:
        print(f"sancho-task-plugin-update-check: bad JSON from plugin outdated: {e}", file=sys.stderr)
        return 3

    home = _makakoo_home()
    journal = _today_journal(home)
    appended = 0
    for row in rows:
        line = _render_line(row)
        if line:
            _append_journal(journal, line)
            appended += 1

    # stdout for SANCHO's own telemetry.
    print(
        f"sancho-task-plugin-update-check: {len(rows)} tracked, "
        f"{appended} drift entries appended to {journal}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
