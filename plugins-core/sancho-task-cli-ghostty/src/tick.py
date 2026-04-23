#!/usr/bin/env python3
"""
SANCHO task: auto-update Ghostty (Homebrew cask) every 24h.

Detects if Ghostty is installed via Homebrew, checks for updates via
`brew outdated --cask ghostty`, upgrades if needed, logs to Brain journal.

Interval: 24h. Config via env:
  GHOSTTY_BREW_BIN  — path to brew binary (default: search PATH)
  GHOSTTY_AUTO_UPGRADE — if "1", auto-upgrade; if "0", report-only (default: "1")
"""
from __future__ import annotations

import os
import platform
import shutil
import subprocess
import sys
import urllib.request
import json
import re
from datetime import datetime, timezone
from pathlib import Path


def _is_macos() -> bool:
    return platform.system() == "Darwin"


TASK_NAME = "cli_ghostty_update"
STATE_NAME = "sancho-task-cli-ghostty"
HOMEBREW_API = "https://formulae.brew.sh/api/cask/ghostty.json"


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


def _brew_bin() -> str:
    return os.environ.get("GHOSTTY_BREW_BIN") or shutil.which("brew") or "brew"


def _is_installed() -> bool:
    brew = _brew_bin()
    result = subprocess.run(
        [brew, "list", "--cask", "ghostty"],
        capture_output=True, text=True, timeout=15,
    )
    return result.returncode == 0


def _installed_version() -> str | None:
    brew = _brew_bin()
    result = subprocess.run(
        [brew, "info", "--cask", "ghostty", "--json"],
        capture_output=True, text=True, timeout=15,
    )
    if result.returncode != 0:
        return None
    try:
        data = json.loads(result.stdout)
        if isinstance(data, list) and data:
            return data[0].get("installed_versions", [""])[0]
        return None
    except (json.JSONDecodeError, IndexError, KeyError):
        return None


def _latest_version() -> str | None:
    """Fetch latest Ghostty version from Homebrew API."""
    try:
        req = urllib.request.Request(HOMEBREW_API, headers={"Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=15) as resp:
            data = json.loads(resp.read())
            versions = data.get("versions", {})
            return versions.get("stable") or versions.get("latest")
    except Exception as e:
        print(f"[{TASK_NAME}] Homebrew API error: {e}", file=sys.stderr)
        return None


def _brew_outdated() -> tuple[bool, str | None, str | None]:
    """Returns (has_update, installed, latest)."""
    brew = _brew_bin()
    result = subprocess.run(
        [brew, "outdated", "--cask", "ghostty", "--json"],
        capture_output=True, text=True, timeout=30,
    )
    if result.returncode == 0 and result.stdout.strip():
        try:
            data = json.loads(result.stdout)
            if isinstance(data, list) and data:
                inst = data[0].get("installed_versions", [""])[0]
                new = data[0].get("current_version", {}).get("version") or inst
                return True, inst, new
        except (json.JSONDecodeError, IndexError, KeyError):
            pass
    return False, None, None


def _brew_upgrade() -> subprocess.CompletedProcess:
    brew = _brew_bin()
    return subprocess.run(
        [brew, "upgrade", "--cask", "ghostty"],
        capture_output=True, text=True, timeout=300,
    )


def main() -> int:
    if not _is_macos():
        print(f"[{TASK_NAME}] not on macOS — skipping.")
        return 0

    home = _makakoo_home()
    journal = _today_journal(home)
    state_file = _state_file()
    auto_upgrade = os.environ.get("GHOSTTY_AUTO_UPGRADE", "1") == "1"

    if not _is_installed():
        _append_journal(journal,
            "- [[Harvey]] Ghostty not installed via Homebrew — skipping update check.")
        return 0

    has_update, installed, latest = _brew_outdated()

    if not has_update:
        print(f"[{TASK_NAME}] at latest ({installed})")
        return 0

    prev_state = state_file.read_text().strip() if state_file.exists() else ""
    pinned = prev_state or installed

    if installed == latest and not prev_state:
        state_file.write_text(latest)

    print(f"[{TASK_NAME}] update available: {installed} → {latest}")

    if not auto_upgrade:
        _append_journal(journal,
            f"- [[Harvey]] Ghostty update available: {installed} → {latest}. "
            f"Run `brew upgrade --cask ghostty` manually.")
        return 0

    _append_journal(journal,
        f"- [[Harvey]] Ghostty updating: {installed} → {latest}...")

    result = _brew_upgrade()
    if result.returncode != 0:
        _append_journal(journal,
            f"- [[Harvey]] Ghostty update FAILED (exit {result.returncode}). "
            f"Run `brew upgrade --cask ghostty` manually.")
        return 1

    new_installed = latest
    state_file.write_text(new_installed)

    _append_journal(journal,
        f"- [[Harvey]] Ghostty updated: → {new_installed}.")

    print(f"[{TASK_NAME}] done: {new_installed}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
