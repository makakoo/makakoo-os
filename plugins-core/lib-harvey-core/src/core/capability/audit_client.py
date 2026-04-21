"""
Python audit-log writer.

Mirrors the Rust `makakoo-core::capability::audit::AuditEntry` schema
byte-for-byte so a downstream `makakoo audit` or `jq` pipeline sees a
single consistent stream regardless of which runtime emitted the
entry. See `spec/CAPABILITIES.md §3` for the full schema and rotation
policy.

**Concurrency.** JSONL is line-delimited; POSIX `O_APPEND` guarantees
each `write()` is atomic up to PIPE_BUF (512 bytes minimum on macOS +
Linux; our lines are ≈ 300 bytes typical). We therefore do not take a
lock — multi-writer append is safe by construction. If a line ever
exceeds PIPE_BUF, the writer falls back to a short `fcntl.flock()`
around the append.

**Rotation.** Not this client's job. The Rust `AuditLog::append` rotates
at 100 MB; Python writes piggy-back on whatever file is currently
named `audit.jsonl` and let the next Rust caller rotate.
"""

from __future__ import annotations

import fcntl
import json
import logging
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Literal, Optional

from core.capability.user_grants import escape_audit_field

log = logging.getLogger("core.capability.audit_client")

#: Keep in sync with `spec/CAPABILITIES.md §3` and the Rust
#: `AuditResult` enum (serde `rename_all = "lowercase"`).
AuditResultLiteral = Literal["allowed", "denied", "error"]

#: Baseline plugin attribution when `HARVEY_PLUGIN` env var is unset.
#: Phase E.3 wires concrete values in bridge.py, agent-harveychat
#: bootstrap, and global_bootstrap v12.
DEFAULT_PLUGIN = "harvey-agent"


def default_audit_path(home: Path | None = None) -> Path:
    """Canonical path: $MAKAKOO_HOME/logs/audit.jsonl."""
    if home is not None:
        return Path(home) / "logs" / "audit.jsonl"
    base = (
        os.environ.get("MAKAKOO_HOME")
        or os.environ.get("HARVEY_HOME")
        or os.path.expanduser("~/MAKAKOO")
    )
    return Path(base) / "logs" / "audit.jsonl"


def _caller_plugin() -> str:
    """Resolve the plugin field for the caller.

    Priority: `HARVEY_PLUGIN` env var (set by bridge.py, the Telegram
    adapter bootstrap, and per-CLI global_bootstrap fragments) → the
    legacy agent-internal default.
    """
    return escape_audit_field(
        os.environ.get("HARVEY_PLUGIN") or DEFAULT_PLUGIN,
        max_len=80,
    )


def _iso8601_utc(dt: datetime) -> str:
    dt_utc = dt.astimezone(timezone.utc)
    return dt_utc.isoformat().replace("+00:00", "Z")


def log_audit(
    *,
    verb: str,
    scope_requested: str,
    scope_granted: Optional[str],
    result: AuditResultLiteral,
    plugin: Optional[str] = None,
    plugin_version: str = "0.3.0",
    duration_ms: Optional[int] = None,
    bytes_in: Optional[int] = None,
    bytes_out: Optional[int] = None,
    correlation_id: Optional[str] = None,
    audit_path: Path | None = None,
    ts: Optional[datetime] = None,
) -> None:
    """Append one audit entry. Safe to call on every capability check.

    Never raises — on I/O error we log to the Python logger and drop
    the entry. The audit log is a best-effort record; an unwritable
    entry must not break the tool call.

    `scope_granted`:
      * for user-grant matches, pass the grant id (e.g. `g_20260421_…`)
      * for baseline matches, pass `"baseline:<root>"` so audit queries
        can tell the layers apart
      * for denied writes, pass `None`
    """
    path = audit_path if audit_path is not None else default_audit_path()
    ts = ts if ts is not None else datetime.now(tz=timezone.utc)

    entry: dict = {
        "ts": _iso8601_utc(ts),
        "plugin": plugin if plugin is not None else _caller_plugin(),
        "plugin_version": plugin_version,
        "verb": verb,
        "scope_requested": scope_requested,
        "scope_granted": scope_granted,
        "result": result,
    }
    if duration_ms is not None:
        entry["duration_ms"] = int(duration_ms)
    if bytes_in is not None:
        entry["bytes_in"] = int(bytes_in)
    if bytes_out is not None:
        entry["bytes_out"] = int(bytes_out)
    if correlation_id is not None:
        entry["correlation_id"] = correlation_id

    # Serialise once; retry-free writes keep the hot path fast.
    try:
        line = json.dumps(entry, ensure_ascii=False)
    except (TypeError, ValueError) as e:  # pragma: no cover
        log.warning("audit serialize failed (dropping entry): %s", e)
        return

    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        data = (line + "\n").encode("utf-8")
        # O_APPEND is POSIX-atomic for PIPE_BUF-sized writes; fall back
        # to a short lock for oversized entries.
        fd = os.open(
            str(path),
            os.O_WRONLY | os.O_CREAT | os.O_APPEND,
            0o644,
        )
        try:
            if len(data) > 512:  # POSIX PIPE_BUF minimum
                fcntl.flock(fd, fcntl.LOCK_EX)
                try:
                    os.write(fd, data)
                finally:
                    fcntl.flock(fd, fcntl.LOCK_UN)
            else:
                os.write(fd, data)
        finally:
            os.close(fd)
    except OSError as e:
        log.warning("audit write failed at %s: %s (dropping entry)", path, e)


def log_fs_write(
    *,
    requested_path: str,
    resolved_path: Optional[str],
    scope_granted: Optional[str],
    result: AuditResultLiteral,
    duration_ms: Optional[int] = None,
    correlation_id: Optional[str] = None,
) -> None:
    """Convenience wrapper for `fs/write` verbs.

    `requested_path` is what the caller *asked* to write — logged
    verbatim so audit queries can see exactly what the agent tried.
    `resolved_path` is the realpath we would actually write to when
    allowed (also logged; helps spot symlink-resolution surprises).
    """
    entry_scope = requested_path
    if resolved_path and resolved_path != requested_path:
        entry_scope = f"{requested_path} -> {resolved_path}"
    log_audit(
        verb="fs/write",
        scope_requested=entry_scope,
        scope_granted=scope_granted,
        result=result,
        duration_ms=duration_ms,
        correlation_id=correlation_id,
    )
