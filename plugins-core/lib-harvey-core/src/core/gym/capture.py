"""
Layer 1 of Harvey's Mascot GYM — error capture.

Three independent capture sources funnel into one schema:
  - bash:   harvey-wrap shell wrapper (bin/harvey-wrap)
  - tool:   Claude Code Stop hook (~/.claude/hooks/harvey-gym-error.js)
  - python: @log_errors decorator on Python entrypoints
  - sancho: SANCHO task failures (internal)

All writes land in data/errors/YYYY-MM-DD/<source>.jsonl.
Writes are append-only, atomic per-line (flush + fsync), concurrency-safe
for the "multiple writers into the same file" case on local disk.

Schema v1.0:
    {
        "schema_version": "1.0",
        "ts":              ISO 8601 UTC,
        "source":          "bash" | "tool" | "python" | "sancho",
        "cmd":             truncated command / tool-name / function-name (<= 512 chars),
        "cwd":             working dir with $HOME redacted,
        "stderr":          truncated stderr / error message (<= 2048 chars, $HOME redacted),
        "exit_code":       int (process) | None (python exception),
        "agent":           "harvey" | "olibia" | "sancho" | ...,
        "skill_in_scope":  best-effort skill name (e.g., "meta/caveman-voice") or None,
        "error_class":     None at capture time — filled by Layer 2 classifier,
        "raw":             {...} — any source-specific fields (stack trace, tool id, ...)
    }

Design notes:
  - Capture is hot-path. No LLM calls. No network. No expensive imports.
  - Failures in capture MUST NOT break the calling code. Every write is
    wrapped; on failure we log to stderr and move on. Losing an error
    record is always better than crashing the thing that produced it.
  - PII scrubbing: $HOME paths get replaced; everything else is the caller's
    responsibility. This is not a redaction engine, just a floor.
"""

from __future__ import annotations

import functools
import json
import os
import sys
import traceback
from datetime import datetime, timezone
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Dict, Optional

SCHEMA_VERSION = "1.0"
MAX_CMD = 512
MAX_STDERR = 2048

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
ERRORS_DIR = Path(HARVEY_HOME) / "data" / "errors"
HOME = os.path.expanduser("~")


class ErrorSource(str, Enum):
    BASH = "bash"
    TOOL = "tool"
    PYTHON = "python"
    SANCHO = "sancho"
    MANUAL_FLAG = "manual_flag"  # Sebastian-flagged wrong response via `harvey flag`


def _today_utc() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%d")


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _redact(text: Optional[str]) -> str:
    if not text:
        return ""
    return text.replace(HOME, "$HOME")


def _truncate(text: Optional[str], limit: int) -> str:
    if not text:
        return ""
    if len(text) <= limit:
        return text
    return text[: limit - 15] + "...[truncated]"


def _log_file(source: str, day: Optional[str] = None) -> Path:
    day = day or _today_utc()
    directory = ERRORS_DIR / day
    directory.mkdir(parents=True, exist_ok=True)
    return directory / f"{source}.jsonl"


def _infer_agent() -> str:
    return os.environ.get("HARVEY_AGENT", "harvey")


def _infer_skill() -> Optional[str]:
    return os.environ.get("HARVEY_SKILL_IN_SCOPE") or None


def _write_line(path: Path, record: Dict[str, Any]) -> None:
    """
    Append a single JSON line atomically.

    Opens in append mode, writes the JSON + newline in one system call
    (Python usually buffers, so we flush + fsync to get the atomic line
    guarantee that jsonl readers depend on).
    """
    line = json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n"
    with open(path, "a", encoding="utf-8") as f:
        f.write(line)
        f.flush()
        try:
            os.fsync(f.fileno())
        except OSError:
            pass


def log_error(
    source: str,
    cmd: Optional[str] = None,
    stderr: Optional[str] = None,
    exit_code: Optional[int] = None,
    cwd: Optional[str] = None,
    agent: Optional[str] = None,
    skill_in_scope: Optional[str] = None,
    raw: Optional[Dict[str, Any]] = None,
    error_class: Optional[str] = None,
) -> bool:
    """
    Log a single error to the GYM error funnel.

    Returns True on successful write, False on capture failure. Never
    raises — capture failures are silent (by design: we never break the
    caller to record an error).
    """
    try:
        if source not in (s.value for s in ErrorSource):
            print(f"gym.capture: unknown source {source!r}", file=sys.stderr)
            return False

        record: Dict[str, Any] = {
            "schema_version": SCHEMA_VERSION,
            "ts": _now_iso(),
            "source": source,
            "cmd": _truncate(_redact(cmd), MAX_CMD),
            "cwd": _redact(cwd or os.getcwd()),
            "stderr": _truncate(_redact(stderr), MAX_STDERR),
            "exit_code": exit_code,
            "agent": agent or _infer_agent(),
            "skill_in_scope": skill_in_scope or _infer_skill(),
            "error_class": error_class,  # usually None; manual flags pre-label "skill"
            "raw": raw or {},
        }

        _write_line(_log_file(source), record)
        return True
    except Exception as exc:
        print(f"gym.capture: write failed: {exc!r}", file=sys.stderr)
        return False


def log_errors(
    skill: Optional[str] = None,
    agent: Optional[str] = None,
) -> Callable:
    """
    Decorator for Python skill entrypoints.

    Captures any exception raised by the wrapped function, logs it to the
    GYM funnel under source="python", then re-raises. Decorator is
    transparent — no swallowing, no return-value change.

    Usage:
        @log_errors(skill="meta/caveman-voice")
        def caveman_translate(text):
            ...
    """
    def decorator(func: Callable) -> Callable:
        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            try:
                return func(*args, **kwargs)
            except Exception as exc:
                stack = traceback.format_exc()
                log_error(
                    source=ErrorSource.PYTHON.value,
                    cmd=f"{func.__module__}.{func.__qualname__}",
                    stderr=f"{type(exc).__name__}: {exc}",
                    exit_code=None,
                    agent=agent,
                    skill_in_scope=skill,
                    raw={"traceback": stack},
                )
                raise
        return wrapper
    return decorator
