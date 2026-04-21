"""
Global grant rate-limit counter.

Separate file from the grant store so a corrupted counter can't poison
the grants (lope F7). Schema:

    {
      "window_start": "2026-04-21T09:30:00Z",
      "creates_in_window": 12
    }

Limits (from SPRINT.md §3 LD#14):

    max_active_grants_system_wide = 20
    max_create_ops_per_rolling_hour = 50

Both Python (this file) and Rust (`makakoo-core/src/capability/rate_limit.rs`)
read/write the same file via the same sidecar-lock protocol as the
grant store.
"""

from __future__ import annotations

import fcntl
import json
import logging
import os
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

log = logging.getLogger("core.capability.rate_limit")

MAX_ACTIVE_GRANTS = 20
MAX_CREATES_PER_HOUR = 50
WINDOW_SECONDS = 60 * 60


class RateLimitExceeded(Exception):
    """Raised when a grant create would exceed a global limit.

    Message shape is quotable by tool handlers — keep it short and
    actionable. Carries both counts so the caller can expose them.
    """

    def __init__(self, active: int, creates_in_window: int, reason: str):
        self.active = active
        self.creates_in_window = creates_in_window
        self.reason = reason
        super().__init__(reason)


@dataclass
class _WindowState:
    window_start: datetime
    creates_in_window: int


def default_rate_limit_path(home: Path | None = None) -> Path:
    """Canonical path: $MAKAKOO_HOME/state/perms_rate_limit.json."""
    if home is not None:
        return Path(home) / "state" / "perms_rate_limit.json"
    base = (
        os.environ.get("MAKAKOO_HOME")
        or os.environ.get("HARVEY_HOME")
        or os.path.expanduser("~/MAKAKOO")
    )
    return Path(base) / "state" / "perms_rate_limit.json"


def _now_utc() -> datetime:
    return datetime.now(tz=timezone.utc)


def _load(path: Path) -> _WindowState:
    """Tolerant load — missing or corrupt file resets to empty window."""
    if not path.exists():
        return _WindowState(window_start=_now_utc(), creates_in_window=0)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        ws = datetime.fromisoformat(data["window_start"].replace("Z", "+00:00"))
        if ws.tzinfo is None:
            ws = ws.replace(tzinfo=timezone.utc)
        return _WindowState(
            window_start=ws,
            creates_in_window=int(data["creates_in_window"]),
        )
    except (json.JSONDecodeError, KeyError, ValueError, TypeError) as e:
        log.warning(
            "corrupt perms_rate_limit.json at %s; resetting window (%s)",
            path,
            e,
        )
        return _WindowState(window_start=_now_utc(), creates_in_window=0)


def _save(path: Path, state: _WindowState) -> None:
    """Atomic write via tmp + rename. Caller holds the sidecar lock."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".json.tmp")
    payload = {
        "window_start": state.window_start.isoformat().replace("+00:00", "Z"),
        "creates_in_window": state.creates_in_window,
    }
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2)
        f.flush()
        os.fsync(f.fileno())
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def check_and_increment(
    active_grant_count: int,
    *,
    path: Path | None = None,
    now: datetime | None = None,
) -> None:
    """Raise RateLimitExceeded if creating a new grant would exceed limits.

    Otherwise increment the in-window create counter atomically.

    Callers pass `active_grant_count` from `UserGrantsFile.active_grants()`
    so this helper doesn't need to re-open the grant store.
    """
    if path is None:
        path = default_rate_limit_path()
    if now is None:
        now = _now_utc()

    if active_grant_count >= MAX_ACTIVE_GRANTS:
        raise RateLimitExceeded(
            active=active_grant_count,
            creates_in_window=0,
            reason=(
                f"rate limit: {active_grant_count} active grants "
                f"(max {MAX_ACTIVE_GRANTS}); revoke some or wait"
            ),
        )

    lock_path = path.with_suffix(".json.lock")
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with open(lock_path, "w", encoding="utf-8") as lock_fd:
        fcntl.flock(lock_fd.fileno(), fcntl.LOCK_EX)
        try:
            state = _load(path)
            window_age = now - state.window_start
            if window_age >= timedelta(seconds=WINDOW_SECONDS):
                state = _WindowState(window_start=now, creates_in_window=0)

            if state.creates_in_window >= MAX_CREATES_PER_HOUR:
                raise RateLimitExceeded(
                    active=active_grant_count,
                    creates_in_window=state.creates_in_window,
                    reason=(
                        f"rate limit: {state.creates_in_window} grants "
                        f"created in the last hour (max "
                        f"{MAX_CREATES_PER_HOUR}); wait a bit"
                    ),
                )

            state.creates_in_window += 1
            _save(path, state)
        finally:
            fcntl.flock(lock_fd.fileno(), fcntl.LOCK_UN)


def reset_for_tests(path: Path | None = None) -> None:
    """Test-only: wipe the counter file. DO NOT call from production code."""
    if path is None:
        path = default_rate_limit_path()
    if path.exists():
        path.unlink()
