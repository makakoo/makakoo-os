#!/usr/bin/env python3
"""
SANCHO task: Makakoo OS auto-update.

Reads $MAKAKOO_HOME/config/updates.toml:
  mode = "auto"   -> run `makakoo update --reinfect` every 24h
  mode = "manual" -> do nothing; user runs `makakoo update --reinfect`
  missing config   -> do nothing until setup writes an explicit mode

The task intentionally does not restart the daemon from inside the daemon. The
update command prints `makakoo daemon restart`; journal entries repeat that hint
when a version delta is detected.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import time
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path

TASK_NAME = "makakoo_os_update"
STATE_NAME = "sancho-task-makakoo-update"


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    return Path(home).expanduser() if home else Path.home() / "MAKAKOO"


def _state_dir() -> Path:
    sd = _makakoo_home() / "state" / STATE_NAME
    sd.mkdir(parents=True, exist_ok=True)
    return sd


@contextmanager
def _single_flight_lock():
    """Best-effort cross-tick lock using atomic lock-file creation.

    Avoids `fcntl` so Windows does not run overlapping update ticks. A stale
    lock older than the update timeout plus 5 minutes is cleared.
    """
    lock_path = _state_dir() / "tick.lock"
    fd: int | None = None
    acquired = False
    try:
        fd = _acquire_lock_file(lock_path)
        if fd is None:
            print(f"[{TASK_NAME}] another update tick is already running")
            yield False
            return
        acquired = True
        os.write(fd, f"pid={os.getpid()} at={datetime.now(timezone.utc).isoformat()}\n".encode("utf-8"))
        yield True
    finally:
        if fd is not None:
            os.close(fd)
        if acquired:
            try:
                lock_path.unlink()
            except FileNotFoundError:
                pass


def _lock_stale_seconds() -> int:
    return _update_timeout() + 300


def _update_timeout() -> int:
    raw = os.environ.get("MAKAKOO_UPDATE_TIMEOUT", "1200")
    try:
        value = int(raw)
    except ValueError:
        print(f"[{TASK_NAME}] invalid MAKAKOO_UPDATE_TIMEOUT={raw!r}; using 1200", file=sys.stderr)
        return 1200
    return max(60, value)


def _acquire_lock_file(lock_path: Path) -> int | None:
    flags = os.O_CREAT | os.O_EXCL | os.O_WRONLY
    try:
        return os.open(lock_path, flags, 0o600)
    except FileExistsError:
        try:
            age = time.time() - lock_path.stat().st_mtime
        except FileNotFoundError:
            try:
                return os.open(lock_path, flags, 0o600)
            except FileExistsError:
                return None
        if age <= _lock_stale_seconds():
            return None
        print(f"[{TASK_NAME}] clearing stale update lock at {lock_path}")
        try:
            lock_path.unlink()
        except FileNotFoundError:
            pass
        try:
            return os.open(lock_path, flags, 0o600)
        except FileExistsError:
            return None


def _today_journal(home: Path) -> Path:
    today = datetime.now().strftime("%Y_%m_%d")
    journals = home / "data" / "Brain" / "journals"
    journals.mkdir(parents=True, exist_ok=True)
    return journals / f"{today}.md"


def _append_journal(path: Path, line: str) -> None:
    if not line.endswith("\n"):
        line += "\n"
    with open(path, "a", encoding="utf-8") as f:
        f.write(line)


def _config_path(home: Path) -> Path:
    return home / "config" / "updates.toml"


def _read_mode(home: Path) -> str:
    override = os.environ.get("MAKAKOO_UPDATE_MODE", "").strip().lower()
    if override in {"auto", "manual"}:
        return override
    path = _config_path(home)
    if not path.exists():
        return "manual"
    try:
        for raw in path.read_text(encoding="utf-8").splitlines():
            line = raw.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, value = line.split("=", 1)
            if key.strip() == "mode":
                mode = value.split("#", 1)[0].strip().strip('"\'').lower()
                if mode in {"auto", "manual"}:
                    return mode
    except OSError as e:
        print(f"[{TASK_NAME}] could not read {path}: {e}", file=sys.stderr)
    return "manual"


def _makakoo_bin() -> str | None:
    return os.environ.get("MAKAKOO_BIN") or shutil.which("makakoo")


def _extract_delta(output: str) -> tuple[str, str] | None:
    before = after = None
    in_version_delta = False
    for line in output.splitlines():
        if line.strip() == "# version delta:":
            before = after = None
            in_version_delta = True
            continue
        if not in_version_delta:
            continue
        m_before = re.match(r"\s*before:\s*(.+)", line)
        if m_before:
            before = m_before.group(1).strip()
        m_after = re.match(r"\s*after:\s*(.+)", line)
        if m_after:
            after = m_after.group(1).strip()
            if before and after:
                return (before, after) if before != after else None
    return None


def _run_update(makakoo: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [makakoo, "update", "--reinfect"],
        capture_output=True,
        text=True,
        timeout=_update_timeout(),
    )


def _to_text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return str(value)


def main() -> int:
    with _single_flight_lock() as acquired:
        if not acquired:
            return 0
        return _main_locked()


def _main_locked() -> int:
    home = _makakoo_home()
    journal = _today_journal(home)
    mode = _read_mode(home)

    if mode == "manual":
        print(f"[{TASK_NAME}] manual mode — skipping auto-update")
        return 0

    makakoo = _makakoo_bin()
    if not makakoo:
        _append_journal(journal, "- [[Makakoo OS]] auto-update FAILED — `makakoo` not on PATH.")
        return 2

    print(f"[{TASK_NAME}] auto mode — running makakoo update --reinfect")
    try:
        result = _run_update(makakoo)
    except subprocess.TimeoutExpired as e:
        combined = _to_text(e.stdout) + "\n" + _to_text(e.stderr)
        (_state_dir() / "last-output.txt").write_text(combined[-20000:], encoding="utf-8")
        detail = " ".join(combined.strip().split())[:500]
        suffix = f" {detail}" if detail else ""
        _append_journal(
            journal,
            f"- [[Makakoo OS]] auto-update FAILED — `makakoo update --reinfect` timed out.{suffix}",
        )
        return 3
    except OSError as e:
        _append_journal(
            journal,
            f"- [[Makakoo OS]] auto-update FAILED — could not start `makakoo update --reinfect`: {e}.",
        )
        return 4

    combined = (result.stdout or "") + "\n" + (result.stderr or "")
    (_state_dir() / "last-output.txt").write_text(combined[-20000:], encoding="utf-8")

    if result.returncode != 0:
        detail = " ".join((result.stderr or result.stdout or "").strip().split())[:500]
        _append_journal(
            journal,
            f"- [[Makakoo OS]] auto-update FAILED (exit {result.returncode}). "
            f"Run `makakoo update --reinfect` manually. {detail}",
        )
        return 1

    delta = _extract_delta(combined)
    if delta:
        before, after = delta
        _append_journal(
            journal,
            f"- [[Makakoo OS]] auto-updated: {before} → {after}. "
            f"If the daemon is running, restart it with `makakoo daemon restart`.",
        )
        print(f"[{TASK_NAME}] updated: {before} -> {after}")
    else:
        print(f"[{TASK_NAME}] already up to date")
    return 0


if __name__ == "__main__":
    sys.exit(main())
