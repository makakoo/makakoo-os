#!/usr/bin/env python3
"""
SANCHO task: auto-update @traylinx/switchailocal npm global package.

Checks npm registry for a newer version, updates if found, logs to Brain.
Does NOT restart the service — use `makakoo agent restart agent-switchailocal`
manually or add a post_update hook if desired.

Interval: 24h.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import urllib.request
import json
from datetime import datetime, timezone
from pathlib import Path

PACKAGE = "@traylinx/switchailocal"
TASK_NAME = "cli_switchailocal_update"
STATE_NAME = "sancho-task-cli-switchailocal"


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    return Path(home).expanduser() if home else Path.home() / "MAKAKOO"


def _state_dir() -> Path:
    sd = _makakoo_home() / "state" / STATE_NAME
    sd.mkdir(parents=True, exist_ok=True)
    return sd


def _state_file() -> Path:
    return _state_dir() / "installed_version.txt"


def _today_journal(home: Path) -> Path:
    today = datetime.now(timezone.utc).strftime("%Y_%m_%d")
    journals = home / "data" / "Brain" / "journals"
    journals.mkdir(parents=True, exist_ok=True)
    return journals / f"{today}.md"


def _append_journal(path: Path, line: str) -> None:
    if not line.endswith("\n"):
        line += "\n"
    with open(path, "a", encoding="utf-8") as f:
        f.write(line)


def _npm_bin() -> str:
    return shutil.which("npm") or "npm"


def _installed_version() -> str | None:
    npm = _npm_bin()
    result = subprocess.run(
        [npm, "list", "-g", "--depth=0", "--json"],
        capture_output=True, text=True, timeout=30,
    )
    if result.returncode != 0:
        return None
    try:
        data = json.loads(result.stdout)
        pkg = data.get("dependencies", {}).get(PACKAGE, {})
        return pkg.get("version")
    except (json.JSONDecodeError, KeyError):
        return None


def _latest_version() -> str | None:
    url = f"https://registry.npmjs.org/{PACKAGE.replace('/', '%2F')}/latest"
    try:
        req = urllib.request.Request(url, headers={"Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=15) as resp:
            data = json.loads(resp.read())
            return data.get("version")
    except Exception as e:
        print(f"[{TASK_NAME}] npm registry error: {e}", file=sys.stderr)
        return None


def _npm_install() -> subprocess.CompletedProcess:
    npm = _npm_bin()
    return subprocess.run(
        [npm, "install", "-g", PACKAGE],
        capture_output=True, text=True, timeout=120,
    )


def main() -> int:
    home = _makakoo_home()
    journal = _today_journal(home)
    state_file = _state_file()

    installed = _installed_version()
    latest = _latest_version()

    if latest is None:
        _append_journal(journal,
            f"- [[Harvey]] switchAILocal update check failed — could not reach npm registry.")
        return 3

    if installed is None:
        _append_journal(journal,
            f"- [[Harvey]] switchAILocal not installed — run `npm install -g {PACKAGE}`.")
        return 4

    prev_state = state_file.read_text().strip() if state_file.exists() else ""
    if prev_state and latest == installed:
        # No change since last check
        print(f"[{TASK_NAME}] at latest: {latest}")
        return 0

    if latest == installed:
        print(f"[{TASK_NAME}] at latest: {latest}")
        return 0

    print(f"[{TASK_NAME}] update available: {installed} → {latest}")

    _append_journal(journal,
        f"- [[Harvey]] switchAILocal updating: {PACKAGE} @ {installed} → {latest}...")

    result = _npm_install()
    if result.returncode != 0:
        _append_journal(journal,
            f"- [[Harvey]] switchAILocal update FAILED (exit {result.returncode}). "
            f"Run `npm install -g {PACKAGE}` manually.")
        return 1

    new_installed = _installed_version() or latest
    state_file.write_text(new_installed)

    _append_journal(journal,
        f"- [[Harvey]] switchAILocal updated: {PACKAGE} → {new_installed}. "
        f"Restart the service with `makakoo agent restart agent-switchailocal`.")

    print(f"[{TASK_NAME}] done: {new_installed}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
