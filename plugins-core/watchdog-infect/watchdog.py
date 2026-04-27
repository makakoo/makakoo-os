#!/usr/bin/env python3
"""Watchdog that catches + heals `makakoo infect` drift on a SANCHO tick.

Runs `makakoo infect --verify --json` every 6h. On clean: silent no-op.
On drift: runs `makakoo infect --global` (idempotent audit+repair from
sprint-007), re-verifies, writes one line to today's Brain journal.

Exit codes:
  0 — clean, or drift detected and successfully healed
  1 — drift detected but post-heal verify *still* dirty (real conflict)
  2 — `makakoo` binary not on PATH or JSON output unparseable
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path

MAX_DRIFT_FOR_CRITICAL = 3
FALLBACK_BINARY = Path.home() / ".cargo" / "bin" / "makakoo"


def resolve_makakoo() -> str | None:
    """PATH lookup, with ~/.cargo/bin fallback for launchd (restricted PATH)."""
    found = shutil.which("makakoo")
    if found:
        return found
    if FALLBACK_BINARY.exists() and os.access(FALLBACK_BINARY, os.X_OK):
        return str(FALLBACK_BINARY)
    return None


def journal_path() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if not home:
        home = str(Path.home() / ".makakoo")
    today = datetime.now().strftime("%Y_%m_%d")
    return Path(home) / "data" / "Brain" / "journals" / f"{today}.md"


def append_journal(line: str) -> None:
    path = journal_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a", encoding="utf-8") as f:
        f.write(line)
        if not line.endswith("\n"):
            f.write("\n")


def run_makakoo(makakoo: str, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [makakoo, *args],
        check=False,
        capture_output=True,
        text=True,
    )


def parse_verify_json(stdout: str) -> dict:
    """Parse `makakoo infect --verify --json` output. Raises on bad shape."""
    data = json.loads(stdout)
    if not isinstance(data, dict) or "clean" not in data or "targets" not in data:
        raise ValueError(f"unexpected verify --json shape: {stdout[:200]}")
    return data


def summarise_drift(data: dict) -> tuple[int, list[str]]:
    """(issue_count, ['cursor (mcp-stale-command)', 'vibe (memory-symlink-broken, ...)'])"""
    dirty_targets = [t for t in data.get("targets", []) if not t.get("clean", True)]
    total_issues = sum(len(t.get("issues", [])) for t in dirty_targets)
    desc = [
        f"{t['name']} ({', '.join(t.get('issues', []))})"
        for t in dirty_targets
    ]
    return total_issues, desc


def tick(makakoo: str) -> int:
    verify = run_makakoo(makakoo, "infect", "--verify", "--json")
    # A non-zero return from `--verify` means either drift (exit 1) or a
    # real execution failure (exit 2+, stale binary, arg rejection).
    # Exit 1 is expected — drift. Anything else is an audit failure.
    if verify.returncode not in (0, 1):
        stamp = datetime.now().strftime("%H:%M:%S")
        snippet = (verify.stderr or verify.stdout or "<no output>").strip()[:200]
        append_journal(
            f"- [[Makakoo Watchdog]] audit failed at {stamp}: "
            f"makakoo exit {verify.returncode}: {snippet}"
        )
        print(
            f"watchdog-infect: `makakoo infect --verify --json` exited {verify.returncode}: "
            f"{snippet}",
            file=sys.stderr,
        )
        return 2
    try:
        data = parse_verify_json(verify.stdout)
    except (ValueError, json.JSONDecodeError) as exc:
        stamp = datetime.now().strftime("%H:%M:%S")
        append_journal(
            f"- [[Makakoo Watchdog]] parse error at {stamp}: {exc}"
        )
        print(f"watchdog-infect: verify --json unparseable: {exc}", file=sys.stderr)
        return 2

    if data.get("clean"):
        # Silent — no journal, no output. Healthy tick.
        return 0

    # Drift detected. Run the heal path.
    issue_count, drift_desc = summarise_drift(data)
    heal = run_makakoo(makakoo, "infect", "--global")

    # Re-verify; retry once to absorb transient FS races.
    post = run_makakoo(makakoo, "infect", "--verify", "--json")
    try:
        post_data = parse_verify_json(post.stdout)
    except (ValueError, json.JSONDecodeError):
        post_data = {"clean": False}
    if not post_data.get("clean"):
        retry = run_makakoo(makakoo, "infect", "--verify", "--json")
        try:
            post_data = parse_verify_json(retry.stdout)
        except (ValueError, json.JSONDecodeError):
            post_data = {"clean": False}

    now = datetime.now().strftime("%H:%M:%S")
    critical = "⚠️ critical drift conflict — " if issue_count > MAX_DRIFT_FOR_CRITICAL else ""
    post_state = "clean" if post_data.get("clean") else "STILL DIRTY"

    lines = [
        f"- {critical}[[Makakoo Watchdog]] caught infect drift at {now}",
        f"  - Drifted targets: {'; '.join(drift_desc)}",
        f"  - Ran `makakoo infect --global`; exit {heal.returncode}",
        f"  - Post-heal verify: {post_state}",
    ]
    append_journal("\n".join(lines))

    return 0 if post_data.get("clean") else 1


def main() -> int:
    parser = argparse.ArgumentParser()
    # SANCHO's SubprocessHandler appends `--task <name>`; accept and ignore.
    parser.add_argument("--task", default="watchdog_infect")
    parser.parse_args()

    makakoo = resolve_makakoo()
    if not makakoo:
        stamp = datetime.now().strftime("%H:%M:%S")
        append_journal(
            f"- [[Makakoo Watchdog]] binary not found at {stamp}: "
            "PATH lookup and ~/.cargo/bin/makakoo both missing"
        )
        print(
            "watchdog-infect: `makakoo` binary not on PATH and no ~/.cargo/bin fallback",
            file=sys.stderr,
        )
        return 2

    return tick(makakoo)


if __name__ == "__main__":
    sys.exit(main())
