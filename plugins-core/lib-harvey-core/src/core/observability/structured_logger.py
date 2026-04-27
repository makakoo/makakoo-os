"""
structured_logger.py — Phase 4 deliverable

JSON log formatter + context variables for the swarm. Every log record
gets enriched with the active workflow_id, step_id, and agent_id from
contextvars, so a line written deep inside a subagent's tool call is
still queryable by workflow.

Usage:

    from core.observability.structured_logger import (
        configure_json_logging, bind_context, log_context
    )
    configure_json_logging(level=logging.INFO)

    with log_context(workflow_id="wf_abc", step_id="r1", agent_id="researcher"):
        log.info("searching brain", extra={"query": "diffusion"})
    # → {"ts": "...", "level": "INFO", "workflow_id": "wf_abc",
    #    "step_id": "r1", "agent_id": "researcher", "msg": "searching brain",
    #    "query": "diffusion"}

Exposed:
  JsonFormatter
  configure_json_logging(level)
  bind_context(**kwargs)        — set context vars (returns tokens for unbind)
  log_context(**kwargs)         — context manager that auto-unbinds
"""

from __future__ import annotations

import contextvars
import json
import logging
import sys
import time
import traceback
from contextlib import contextmanager
from typing import Any, Dict, Iterator, Optional


# ─── Context variables ──────────────────────────────────────────────

_workflow_id: contextvars.ContextVar[Optional[str]] = contextvars.ContextVar(
    "harvey_workflow_id", default=None,
)
_step_id: contextvars.ContextVar[Optional[str]] = contextvars.ContextVar(
    "harvey_step_id", default=None,
)
_agent_id: contextvars.ContextVar[Optional[str]] = contextvars.ContextVar(
    "harvey_agent_id", default=None,
)
_extra_context: contextvars.ContextVar[Dict[str, Any]] = contextvars.ContextVar(
    "harvey_extra", default={},
)


def bind_context(
    workflow_id: Optional[str] = None,
    step_id: Optional[str] = None,
    agent_id: Optional[str] = None,
    **extra: Any,
) -> Dict[str, contextvars.Token]:
    """
    Set context variables for the current execution scope. Returns a
    dict of tokens you can pass to `unbind_context()` to restore prior
    values — or use `log_context()` for automatic scoping.
    """
    tokens: Dict[str, contextvars.Token] = {}
    if workflow_id is not None:
        tokens["workflow_id"] = _workflow_id.set(workflow_id)
    if step_id is not None:
        tokens["step_id"] = _step_id.set(step_id)
    if agent_id is not None:
        tokens["agent_id"] = _agent_id.set(agent_id)
    if extra:
        current = dict(_extra_context.get() or {})
        current.update(extra)
        tokens["extra"] = _extra_context.set(current)
    return tokens


def unbind_context(tokens: Dict[str, contextvars.Token]) -> None:
    """Restore prior values. Tokens come from `bind_context`."""
    for key, tok in tokens.items():
        if key == "workflow_id":
            _workflow_id.reset(tok)
        elif key == "step_id":
            _step_id.reset(tok)
        elif key == "agent_id":
            _agent_id.reset(tok)
        elif key == "extra":
            _extra_context.reset(tok)


@contextmanager
def log_context(
    workflow_id: Optional[str] = None,
    step_id: Optional[str] = None,
    agent_id: Optional[str] = None,
    **extra: Any,
) -> Iterator[None]:
    """
    Context manager form. Restores prior values on exit even if the
    body raises.
    """
    tokens = bind_context(
        workflow_id=workflow_id, step_id=step_id, agent_id=agent_id, **extra
    )
    try:
        yield
    finally:
        unbind_context(tokens)


def current_context() -> Dict[str, Any]:
    """Return the active context vars as a dict (for tests, introspection)."""
    out: Dict[str, Any] = {}
    if (v := _workflow_id.get()) is not None:
        out["workflow_id"] = v
    if (v := _step_id.get()) is not None:
        out["step_id"] = v
    if (v := _agent_id.get()) is not None:
        out["agent_id"] = v
    extra = _extra_context.get() or {}
    if extra:
        out.update(extra)
    return out


# ─── JSON formatter ─────────────────────────────────────────────────


# Standard LogRecord attributes we must NOT copy into the structured
# payload (they're either already captured or would dump implementation
# details).
_LOG_RECORD_BUILTINS = frozenset({
    "name", "msg", "args", "levelname", "levelno", "pathname", "filename",
    "module", "exc_info", "exc_text", "stack_info", "lineno", "funcName",
    "created", "msecs", "relativeCreated", "thread", "threadName",
    "processName", "process", "taskName", "message",
})


class JsonFormatter(logging.Formatter):
    """
    Logging formatter that emits one JSON object per record.

    Merged into every record:
      - ts           ISO-8601 UTC timestamp
      - level        INFO/WARNING/...
      - logger       logger name
      - msg          formatted message
      - workflow_id  from contextvar (if set)
      - step_id      from contextvar (if set)
      - agent_id     from contextvar (if set)
      - any `extra={...}` kwargs passed to the log call
      - exception + traceback if exc_info is set
    """

    def format(self, record: logging.LogRecord) -> str:
        payload: Dict[str, Any] = {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(record.created)),
            "level": record.levelname,
            "logger": record.name,
            "msg": record.getMessage(),
        }

        # Merge context vars
        ctx = current_context()
        for k, v in ctx.items():
            payload.setdefault(k, v)

        # Any `extra={...}` keys get merged (LogRecord stores them as attrs)
        for attr, value in record.__dict__.items():
            if attr in _LOG_RECORD_BUILTINS:
                continue
            if attr.startswith("_"):
                continue
            if attr in payload:
                continue
            try:
                json.dumps(value)  # must be serializable
                payload[attr] = value
            except (TypeError, ValueError):
                payload[attr] = repr(value)

        if record.exc_info:
            etype, evalue, etb = record.exc_info
            payload["exception"] = f"{etype.__name__}: {evalue}"
            payload["traceback"] = "".join(
                traceback.format_exception(etype, evalue, etb)
            )

        return json.dumps(payload, default=str)


def configure_json_logging(
    level: int = logging.INFO,
    stream=None,
    replace_handlers: bool = True,
) -> logging.Handler:
    """
    Install a JSON StreamHandler on the root logger. Returns the handler
    so callers can unhook it later.

    If `replace_handlers=True` (default), existing handlers on the root
    logger are removed first — useful in tests and CLI entry points.
    """
    root = logging.getLogger()
    if replace_handlers:
        for h in list(root.handlers):
            root.removeHandler(h)

    handler = logging.StreamHandler(stream or sys.stderr)
    handler.setFormatter(JsonFormatter())
    handler.setLevel(level)
    root.addHandler(handler)
    root.setLevel(level)
    return handler


__all__ = [
    "JsonFormatter",
    "configure_json_logging",
    "bind_context",
    "unbind_context",
    "log_context",
    "current_context",
]
