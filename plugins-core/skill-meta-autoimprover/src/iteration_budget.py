#!/usr/bin/env python3
"""
Iteration Budget — Auto-Improver Core

Tracks tool-call iterations with refund support. execute_code and Bash
iterations are "free" — they don't count against the budget. This prevents
the agent from running out of iterations when doing heavy code execution.

Based on Hermes-Agent's IterationBudget class.
"""

import threading
from typing import Optional


class IterationBudget:
    """Thread-safe iteration counter with refund support."""

    def __init__(self, max_total: int):
        """Initialize with a max iteration budget."""
        self._max = max_total
        self._used = 0
        self._lock = threading.Lock()

    def consume(self) -> bool:
        """Try to consume one iteration. Returns True if allowed, False if exhausted."""
        with self._lock:
            if self._used >= self._max:
                return False
            self._used += 1
            return True

    def refund(self) -> None:
        """Refund one iteration (e.g. for execute_code which is "free")."""
        with self._lock:
            if self._used > 0:
                self._used -= 1

    def refund_all(self) -> None:
        """Refund all iterations (reset the counter)."""
        with self._lock:
            self._used = 0

    @property
    def remaining(self) -> int:
        """How many iterations are left."""
        with self._lock:
            return max(0, self._max - self._used)

    @property
    def exhausted(self) -> bool:
        """True if no iterations remaining."""
        with self._lock:
            return self._used >= self._max

    def should_stop(self) -> bool:
        """Alias for exhausted — returns True if we should stop."""
        return self.exhausted

    def reset(self, max_total: Optional[int] = None) -> None:
        """Reset the budget, optionally with a new max."""
        with self._lock:
            if max_total is not None:
                self._max = max_total
            self._used = 0


# Tools that are "free" — their iterations don't count against the budget
FREE_TOOLS = {"execute_code", "bash", "run_code", "python"}


def is_free_tool(tool_name: str) -> bool:
    """Returns True if this tool's iterations should be refunded."""
    return tool_name.lower() in FREE_TOOLS


def tool_name_from_call(tool_call: dict) -> str:
    """Extract tool name from a tool call dict."""
    # Handle various formats: {"name": "foo"} or {"function": {"name": "foo"}}
    if "function" in tool_call:
        return tool_call["function"].get("name", "")
    return tool_call.get("name", "")
