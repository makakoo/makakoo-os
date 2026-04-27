#!/usr/bin/env python3
"""
Harvey OS Hook System — Tool interception and lifecycle events.

Inspired by pi-mono's beforeToolCall/afterToolCall pattern.
Hooks intercept tool execution at key points, enabling:
  - Blocking dangerous commands before execution
  - Auto-logging tool results to Brain journal
  - Redacting secrets from output
  - Triggering wiki compilation after Brain writes
  - Rate limiting tool calls
  - Auditing all agent actions

Usage:
    from core.hooks.hooks import HookManager, Hook

    hooks = HookManager()

    @hooks.before("bash")
    def block_rm(context):
        if "rm -rf" in context.args.get("command", ""):
            return HookResult(block=True, reason="Destructive command blocked")

    @hooks.after("*")
    def log_all(context):
        hooks.emit("tool_completed", tool=context.tool_name, result=context.result)

    # In agent loop:
    result = hooks.run_before("bash", args={"command": "ls"})
    if result.blocked:
        print(f"Blocked: {result.reason}")
    else:
        output = execute_tool("bash", args)
        hooks.run_after("bash", args=args, result=output)
"""

import fnmatch
import logging
import os
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

log = logging.getLogger("harvey.hooks")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


# ═══════════════════════════════════════════════════════════════
#  Data Types
# ═══════════════════════════════════════════════════════════════

@dataclass
class HookContext:
    """Context passed to hook functions."""
    tool_name: str
    args: Dict[str, Any]
    agent: str = "harvey"
    timestamp: float = field(default_factory=time.time)
    # Set by after-hooks:
    result: Any = None
    is_error: bool = False
    duration_ms: float = 0


@dataclass
class HookResult:
    """Result from a before-hook. Controls whether the tool executes."""
    block: bool = False
    reason: str = ""
    modified_args: Optional[Dict[str, Any]] = None  # Override args before execution


@dataclass
class AfterHookResult:
    """Result from an after-hook. Can modify tool output."""
    modified_result: Any = None  # Override result
    suppress_log: bool = False   # Skip auto-logging for this call


@dataclass
class Hook:
    """A registered hook function."""
    pattern: str          # Tool name pattern ("bash", "write", "*")
    phase: str            # "before" or "after"
    fn: Callable          # The hook function
    priority: int = 0     # Higher = runs first
    name: str = ""        # Human-readable name


# ═══════════════════════════════════════════════════════════════
#  Hook Manager
# ═══════════════════════════════════════════════════════════════

class HookManager:
    """
    Central registry for tool lifecycle hooks.

    Hooks match tools by glob pattern:
      "*"     — matches all tools
      "bash"  — matches only bash
      "file_*" — matches file_read, file_write, etc.
    """

    def __init__(self):
        self._before_hooks: List[Hook] = []
        self._after_hooks: List[Hook] = []
        self._listeners: Dict[str, List[Callable]] = {}
        self._call_log: List[dict] = []
        self._max_log_size = 1000

    # ── Registration ──────────────────────────────────────────

    def before(self, pattern: str = "*", priority: int = 0, name: str = ""):
        """Decorator: register a before-hook for tool(s) matching pattern."""
        def decorator(fn: Callable):
            hook = Hook(
                pattern=pattern, phase="before", fn=fn,
                priority=priority, name=name or fn.__name__,
            )
            self._before_hooks.append(hook)
            self._before_hooks.sort(key=lambda h: -h.priority)
            log.debug("Registered before-hook: %s → %s", pattern, hook.name)
            return fn
        return decorator

    def after(self, pattern: str = "*", priority: int = 0, name: str = ""):
        """Decorator: register an after-hook for tool(s) matching pattern."""
        def decorator(fn: Callable):
            hook = Hook(
                pattern=pattern, phase="after", fn=fn,
                priority=priority, name=name or fn.__name__,
            )
            self._after_hooks.append(hook)
            self._after_hooks.sort(key=lambda h: -h.priority)
            log.debug("Registered after-hook: %s → %s", pattern, hook.name)
            return fn
        return decorator

    def register(self, phase: str, pattern: str, fn: Callable,
                 priority: int = 0, name: str = ""):
        """Programmatic hook registration (non-decorator)."""
        hook = Hook(
            pattern=pattern, phase=phase, fn=fn,
            priority=priority, name=name or fn.__name__,
        )
        if phase == "before":
            self._before_hooks.append(hook)
            self._before_hooks.sort(key=lambda h: -h.priority)
        elif phase == "after":
            self._after_hooks.append(hook)
            self._after_hooks.sort(key=lambda h: -h.priority)

    # ── Execution ─────────────────────────────────────────────

    def run_before(self, tool_name: str, args: Dict[str, Any],
                   agent: str = "harvey") -> HookResult:
        """
        Run all matching before-hooks. Returns combined result.

        If any hook blocks, execution stops and block result is returned.
        If a hook modifies args, subsequent hooks see the modified args.
        """
        ctx = HookContext(tool_name=tool_name, args=args, agent=agent)
        combined = HookResult()

        for hook in self._before_hooks:
            if not fnmatch.fnmatch(tool_name, hook.pattern):
                continue
            try:
                result = hook.fn(ctx)
                if result is None:
                    continue
                if isinstance(result, HookResult):
                    if result.block:
                        log.warning("Hook '%s' blocked %s: %s",
                                    hook.name, tool_name, result.reason)
                        self._log_call(tool_name, args, agent, blocked=True,
                                       blocked_by=hook.name, reason=result.reason)
                        return result
                    if result.modified_args:
                        ctx.args = result.modified_args
                        combined.modified_args = result.modified_args
            except Exception as e:
                log.error("Before-hook '%s' failed: %s", hook.name, e)

        return combined

    def run_after(self, tool_name: str, args: Dict[str, Any],
                  result: Any, is_error: bool = False,
                  duration_ms: float = 0, agent: str = "harvey") -> AfterHookResult:
        """
        Run all matching after-hooks. Can modify the result.
        """
        ctx = HookContext(
            tool_name=tool_name, args=args, agent=agent,
            result=result, is_error=is_error, duration_ms=duration_ms,
        )
        combined = AfterHookResult()

        for hook in self._after_hooks:
            if not fnmatch.fnmatch(tool_name, hook.pattern):
                continue
            try:
                after_result = hook.fn(ctx)
                if after_result is None:
                    continue
                if isinstance(after_result, AfterHookResult):
                    if after_result.modified_result is not None:
                        ctx.result = after_result.modified_result
                        combined.modified_result = after_result.modified_result
                    if after_result.suppress_log:
                        combined.suppress_log = True
            except Exception as e:
                log.error("After-hook '%s' failed: %s", hook.name, e)

        if not combined.suppress_log:
            self._log_call(tool_name, args, agent,
                           is_error=is_error, duration_ms=duration_ms)

        return combined

    # ── Event System ──────────────────────────────────────────

    def on(self, event: str, fn: Callable):
        """Subscribe to a named event."""
        self._listeners.setdefault(event, []).append(fn)

    def emit(self, event: str, **kwargs):
        """Emit a named event to all subscribers."""
        for fn in self._listeners.get(event, []):
            try:
                fn(**kwargs)
            except Exception as e:
                log.error("Event listener failed for '%s': %s", event, e)

    # ── Call Log ──────────────────────────────────────────────

    def _log_call(self, tool_name: str, args: dict, agent: str,
                  blocked: bool = False, blocked_by: str = "",
                  reason: str = "", is_error: bool = False,
                  duration_ms: float = 0):
        """Append to in-memory call log for auditing."""
        entry = {
            "ts": datetime.now().isoformat(),
            "tool": tool_name,
            "agent": agent,
            "blocked": blocked,
        }
        if blocked:
            entry["blocked_by"] = blocked_by
            entry["reason"] = reason
        if is_error:
            entry["error"] = True
        if duration_ms:
            entry["duration_ms"] = round(duration_ms, 1)

        self._call_log.append(entry)
        if len(self._call_log) > self._max_log_size:
            self._call_log = self._call_log[-self._max_log_size:]

    def get_call_log(self, last_n: int = 50) -> List[dict]:
        """Get recent call log entries."""
        return self._call_log[-last_n:]

    def stats(self) -> dict:
        """Get hook execution statistics."""
        return {
            "before_hooks": len(self._before_hooks),
            "after_hooks": len(self._after_hooks),
            "event_listeners": sum(len(v) for v in self._listeners.values()),
            "calls_logged": len(self._call_log),
            "hooks": [
                {"name": h.name, "phase": h.phase, "pattern": h.pattern}
                for h in self._before_hooks + self._after_hooks
            ],
        }


# ═══════════════════════════════════════════════════════════════
#  Built-in Hooks — Harvey defaults
# ═══════════════════════════════════════════════════════════════

def create_default_hooks() -> HookManager:
    """
    Create a HookManager with Harvey's default safety and logging hooks.

    Includes:
    - Dangerous command detection (before bash)
    - Secret redaction (after bash/exec)
    - Brain journal auto-logging (after significant operations)
    """
    hooks = HookManager()

    # ── Dangerous Command Blocker ─────────────────────────────
    @hooks.before("bash", priority=100, name="dangerous_command_guard")
    def guard_dangerous(ctx: HookContext) -> Optional[HookResult]:
        """Block destructive commands without explicit approval."""
        cmd = ctx.args.get("command", "")
        dangerous_patterns = [
            r"rm\s+-rf\s+/",
            r"mkfs\.",
            r"dd\s+if=.*of=/dev/",
            r":\(\)\{.*\|.*&\s*\}",  # fork bomb
            r">\s*/dev/sd[a-z]",
            r"chmod\s+-R\s+777\s+/",
            r"curl.*\|\s*(bash|sh|zsh)",
        ]
        import re
        for pattern in dangerous_patterns:
            if re.search(pattern, cmd, re.IGNORECASE):
                return HookResult(
                    block=True,
                    reason=f"Blocked destructive command: matches '{pattern}'"
                )
        return None

    # ── Secret Redaction ──────────────────────────────────────
    @hooks.after("*", priority=90, name="secret_redactor")
    def redact_secrets(ctx: HookContext) -> Optional[AfterHookResult]:
        """Redact potential secrets from tool output."""
        if not isinstance(ctx.result, str):
            return None

        import re
        patterns = [
            (r'(sk-[a-zA-Z0-9]{20,})', r'sk-***REDACTED***'),
            (r'(ghp_[a-zA-Z0-9]{36,})', r'ghp_***REDACTED***'),
            (r'(AKIA[A-Z0-9]{16})', r'AKIA***REDACTED***'),
            (r'(eyJ[a-zA-Z0-9_-]{50,})', r'***JWT_REDACTED***'),
        ]

        redacted = ctx.result
        changed = False
        for pattern, replacement in patterns:
            new = re.sub(pattern, replacement, redacted)
            if new != redacted:
                changed = True
                redacted = new

        if changed:
            log.info("Redacted secrets from %s output", ctx.tool_name)
            return AfterHookResult(modified_result=redacted)
        return None

    # ── Brain Auto-Logger ─────────────────────────────────────
    @hooks.after("*", priority=10, name="brain_auto_logger")
    def auto_log_significant(ctx: HookContext) -> None:
        """Log significant tool operations to Brain journal."""
        # Only log long-running or error operations
        if ctx.duration_ms < 5000 and not ctx.is_error:
            return None

        hooks.emit("significant_operation",
                    tool=ctx.tool_name,
                    duration_ms=ctx.duration_ms,
                    is_error=ctx.is_error,
                    agent=ctx.agent)
        return None

    return hooks


# Singleton for Harvey
_default_hooks: Optional[HookManager] = None


def get_hooks() -> HookManager:
    """Get or create the default Harvey hook manager."""
    global _default_hooks
    if _default_hooks is None:
        _default_hooks = create_default_hooks()
    return _default_hooks
