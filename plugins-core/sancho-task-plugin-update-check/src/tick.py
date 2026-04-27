#!/usr/bin/env python3
"""
SANCHO task: plugin update checker + optional auto-updater.

Two detection paths:
  - git-sourced plugins  : use `makakoo plugin outdated --json` (kernel-built-in)
  - local-path plugins   : fetch GitHub releases API for each plugin's upstream_repo

Per-plugin config in plugins-to-update.json:
```json
{
  "plugins": [
    {
      "name": "agent-browser-harness",
      "auto_update": true,
      "upstream_repo": "browser-use/browser-harness",
      "post_update": [
        "makakoo agent stop agent-browser-harness",
        "makakoo agent start agent-browser-harness"
      ]
    },
    {
      "name": "agent-arbitrage",
      "auto_update": false,
      "upstream_repo": "traylinx/makakoo-arbitrage"
    }
  ]
}
```

Plugins not listed default to report-only (no auto-update, no GitHub probe).
Plugins with upstream_repo are probed via GitHub releases API even if not drifted
on the filesystem — this handles the case where install.sh fetches a new upstream
ref on every run.

Interval: 24h. Override via SANCHO_TASK_INTERVAL env.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home).expanduser()
    return Path.home() / "MAKAKOO"


def _state_dir() -> Path:
    sd = _makakoo_home() / "state" / "sancho-task-plugin-update-check"
    sd.mkdir(parents=True, exist_ok=True)
    return sd


def _config_path() -> Path:
    return _state_dir() / "plugins-to-update.json"


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


def _short(sha: str | None) -> str:
    if not sha:
        return "(new)"
    return sha[:7]


def _makakoo_bin() -> str | None:
    return shutil.which("makakoo") or os.environ.get("MAKAKOO_BIN")


def _run(args: list[str], timeout: int = 300) -> subprocess.CompletedProcess:
    makakoo = _makakoo_bin()
    assert makakoo, "makakoo not on PATH"
    return subprocess.run(
        [makakoo] + args,
        capture_output=True, text=True, timeout=timeout,
    )


def _run_hook(cmd: str) -> int:
    makakoo = _makakoo_bin()
    if not makakoo:
        return 2
    result = subprocess.run(
        cmd,
        shell=True,
        capture_output=True, text=True, timeout=120,
        env={**os.environ, "MAKAKOO_BIN": makakoo},
    )
    if result.returncode != 0:
        print(f"[plugin_update_check] hook FAILED: {cmd}", file=sys.stderr)
        print(f"  stderr: {result.stderr.strip()}", file=sys.stderr)
    else:
        print(f"[plugin_update_check] hook OK: {cmd}")
    return result.returncode


def _load_config() -> dict[str, dict[str, Any]]:
    path = _config_path()
    if not path.exists():
        return {}
    try:
        data = json.loads(path.read_text())
        return {p["name"]: p for p in data.get("plugins", [])}
    except (json.JSONDecodeError, KeyError, OSError) as e:
        print(f"[plugin_update_check] config parse error {path}: {e}", file=sys.stderr)
        return {}


def _fetch_latest_tag(repo: str) -> str | None:
    """Fetch latest release tag from GitHub API. Returns None on failure."""
    url = f"https://api.github.com/repos/{repo}/releases/latest"
    try:
        req = urllib.request.Request(url, headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "makakoo-plugin-updater/1.0",
        })
        with urllib.request.urlopen(req, timeout=15) as resp:
            data = json.loads(resp.read().decode("utf-8", errors="replace"))
            tag = data.get("tag_name", "")
            return tag.lstrip("v") if tag else None
    except (
        urllib.error.URLError, urllib.error.HTTPError,
        TimeoutError, OSError, json.JSONDecodeError,
    ) as e:
        print(f"[plugin_update_check] GitHub API error for {repo}: {e}", file=sys.stderr)
        return None


def _render_report_line(row: dict) -> str | None:
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
        f"{_short(row.get('upstream'))} ({drift_type}). "
        f"Run `makakoo plugin update {row.get('name', '?')}`."
    )


def _handle_plugin_update(
    name: str,
    current_ref: str,
    latest_ref: str,
    auto_update: bool,
    post_update: list[str],
    journal: Path,
) -> None:
    """Check one plugin. If auto_update, run update + hooks. Otherwise log."""
    if not auto_update:
        _append_journal(journal,
            f"- [[Harvey]] plugin update available: {name} @ {_short(current_ref)} → {_short(latest_ref)}. "
            f"Run `makakoo plugin update {name}`.")
        return

    _append_journal(journal,
        f"- [[Harvey]] auto-updating plugin: {name} @ {_short(current_ref)} → {_short(latest_ref)}...")
    print(f"[plugin_update_check] auto-updating {name}: {current_ref} → {latest_ref}")

    result = _run(["plugin", "update", name])
    if result.returncode != 0:
        _append_journal(journal,
            f"- [[Harvey]] plugin update FAILED: {name} (exit {result.returncode}). "
            f"Run `makakoo plugin update {name}` manually. stderr: {result.stderr.strip()}")
        return

    # Run post-update hooks
    failures = [h for h in post_update if _run_hook(h) != 0]

    # Persist new ref so next run doesn't re-update
    ref_file = _state_dir() / f"{name}.ref"
    ref_file.write_text(latest_ref)

    if failures:
        _append_journal(journal,
            f"- [[Harvey]] plugin updated: {name} → {_short(latest_ref)}. "
            f"Hook failures — run manually: {'; '.join(failures)}")
    else:
        _append_journal(journal,
            f"- [[Harvey]] plugin updated: {name} → {_short(latest_ref)} "
            f"(+ {len(post_update)} hook(s)).")


def main() -> int:
    makakoo = _makakoo_bin()
    if not makakoo:
        print("[plugin_update_check] makakoo binary not on PATH", file=sys.stderr)
        return 2

    config = _load_config()
    home = _makakoo_home()
    journal = _today_journal(home)

    # Path 1: git-sourced plugins — use kernel's outdated check
    result = _run(["plugin", "outdated", "--json"], timeout=120)
    if result.returncode == 0:
        try:
            rows = json.loads(result.stdout or "[]")
        except json.JSONDecodeError as e:
            print(f"[plugin_update_check] bad JSON from outdated: {e}", file=sys.stderr)
            rows = []
    else:
        print(f"[plugin_update_check] `plugin outdated` exit={result.returncode}: {result.stderr.strip()}", file=sys.stderr)
        rows = []

    for row in rows:
        name: str = row.get("name", "?")
        plugin_cfg = config.get(name, {})
        auto_update = plugin_cfg.get("auto_update", False)
        post_update: list[str] = plugin_cfg.get("post_update", [])
        if not row.get("drift"):
            continue
        _handle_plugin_update(
            name,
            row.get("current", "?"),
            row.get("upstream", "?"),
            auto_update,
            post_update,
            journal,
        )

    # Path 2: local-path plugins with known upstream_repo — probe GitHub directly
    for name, plugin_cfg in config.items():
        upstream_repo = plugin_cfg.get("upstream_repo")
        if not upstream_repo:
            continue

        latest_tag = _fetch_latest_tag(upstream_repo)
        if latest_tag is None:
            _append_journal(journal,
                f"- [[Harvey]] plugin update check failed: {name} — could not fetch {upstream_repo} releases.")
            continue

        ref_file = _state_dir() / f"{name}.ref"
        current_tag = ref_file.read_text().strip() if ref_file.exists() else ""

        if current_tag and latest_tag == current_tag:
            print(f"[plugin_update_check] {name}: at latest {latest_tag}")
            continue

        auto_update = plugin_cfg.get("auto_update", False)
        post_update: list[str] = plugin_cfg.get("post_update", [])
        _handle_plugin_update(
            name,
            current_tag or "(unknown)",
            latest_tag,
            auto_update,
            post_update,
            journal,
        )

    print(f"[plugin_update_check] done.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
