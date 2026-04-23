#!/usr/bin/env python3
"""
SANCHO heartbeat for freelance-office secretary.
Wraps the existing ~/MAKAKOO/data/office-secretary/secretary.py
so the email polling runs on SANCHO's schedule (every 4h) instead of
a separate LaunchAgent.

Every tick:
1. Calls secretary.py for all active freelance-office clients
2. Reads tick.log for results
3. Appends a summary line to today's Brain journal

No-op if no new mail (secretary.py is already silent on zero-activity).
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


def main() -> int:
    parser = argparse.ArgumentParser(
        description="SANCHO tick: freelance-office email secretary."
    )
    parser.add_argument("--task", default="freelance_office_secretary", help=argparse.SUPPRESS)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    home = _makakoo_home()
    secretary_dir = home / "data" / "office-secretary"
    tick_log = secretary_dir / "log" / "tick.log"
    run_script = secretary_dir / "run.sh"

    if not run_script.exists():
        print(f"office-secretary not found at {run_script} — skipping")
        return 0

    if args.dry_run:
        print(f"Would run: {run_script} --no-notify")
        return 0

    # Run the secretary. --no-notify suppresses Telegram during SANCHO ticks.
    # We still get the tick.log entry which tells us what happened.
    try:
        result = subprocess.run(
            ["bash", str(run_script)],
            capture_output=True,
            text=True,
            timeout=120,
            cwd=str(secretary_dir),
        )
    except subprocess.TimeoutExpired:
        print("secretary timed out after 120s — skipping journal entry")
        return 0
    except FileNotFoundError:
        print(f"bash or {run_script} not found — skipping")
        return 0

    # Read the last tick.log entry to build a journal summary
    new_mail_count = 0
    clients_checked = []
    silent = True

    if tick_log.exists():
        try:
            lines = tick_log.read_text(encoding="utf-8").strip().splitlines()
            if lines:
                last = json.loads(lines[-1])
                new_mail_count = sum(
                    c.get("new", 0) for c in last.get("activity", [])
                )
                clients_checked = last.get("clients_checked", [])
                silent = last.get("silent", True)
        except (json.JSONDecodeError, OSError):
            pass

    # Journal entry — always log even on silent ticks for traceability
    journal_line = (
        f"[[freelance-office]] secretary tick: "
        f"clients={clients_checked or 'none'} "
        f"new_emails={new_mail_count} "
        f"silent={silent}"
    )
    _append_journal_line(home, journal_line)

    if silent or new_mail_count == 0:
        print(f"freelance_office_secretary: no new mail ({clients_checked or 'no clients'})")
    else:
        print(
            f"freelance_office_secretary: {new_mail_count} new email(s) "
            f"across {clients_checked}"
        )

    return 0


if __name__ == "__main__":
    sys.exit(main())
