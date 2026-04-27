"""
Inbox pipeline tick — email triage cadence.

v0.2 Phase C.6. Every 4h: invoke the triage wizard over recent mail,
record a summary line in the Brain. Actual Gmail access happens
through the `gws` CLI (user-authenticated once, cached OAuth token).

Non-fatal on missing state: a fresh install without the triage config
reports "no triage state" and exits 0 so SANCHO treats it as healthy.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home)
    return Path.home() / "MAKAKOO"


def _append_journal_line(home: Path, line: str) -> None:
    today = datetime.now(timezone.utc).strftime("%Y_%m_%d")
    jdir = home / "data" / "Brain" / "journals"
    jdir.mkdir(parents=True, exist_ok=True)
    journal = jdir / f"{today}.md"
    if not line.startswith("- "):
        line = f"- {line}"
    with open(journal, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def _load_triage_state(home: Path) -> dict | None:
    p = home / "data" / "inbox-triage" / "state.json"
    if not p.exists():
        return None
    try:
        with open(p, encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return None


def _list_recent_mail() -> list[dict]:
    """Run `gws gmail` to list unread mail in the last 4h. Returns an
    empty list if gws is missing or the call fails — we never crash
    the SANCHO tick on an external-tool outage."""
    cmd = [
        "gws",
        "gmail",
        "users",
        "messages",
        "list",
        "me",
        "--query",
        "newer_than:4h -from:noreply -from:notifications -label:promotions",
        "--format",
        "json",
    ]
    try:
        out = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=60,
        )
    except FileNotFoundError:
        return []
    except Exception:
        return []
    if out.returncode != 0:
        return []
    try:
        data = json.loads(out.stdout)
    except Exception:
        return []
    if isinstance(data, list):
        return data
    return data.get("messages", []) or []


def main() -> int:
    parser = argparse.ArgumentParser(description="Inbox pipeline SANCHO tick.")
    parser.add_argument("--task", default="inbox_pipeline", help=argparse.SUPPRESS)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    home = _makakoo_home()
    triage_state = _load_triage_state(home)
    if triage_state is None:
        line = "- [[SANCHO]] [[inbox_pipeline]] no triage state — skipped"
        if not args.dry_run:
            _append_journal_line(home, line)
        print(line.lstrip("- "))
        return 0

    messages = _list_recent_mail()
    summary_line = (
        f"- [[SANCHO]] [[inbox_pipeline]] {len(messages)} new messages in last 4h"
    )
    if args.dry_run:
        print(summary_line.lstrip("- "))
        return 0

    _append_journal_line(home, summary_line)
    print(summary_line.lstrip("- "))
    return 0


if __name__ == "__main__":
    sys.exit(main())
