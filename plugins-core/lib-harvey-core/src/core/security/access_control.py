"""
access_control.py — Phase 4 deliverable

AgentAccessControl: per-agent tool allowlists + token-bucket rate limits.
The subagent framework's `tool()` method can delegate here before calling
the underlying tool, giving us a single choke point for:

  - Restricting which tools each agent may call (defense-in-depth if an
    agent is compromised or asked to do something out of scope)
  - Rate-limiting expensive tools (image gen, web search, LLM calls)
  - Enforcing per-agent, per-tool concurrency caps

Deliberately NOT a real ACL system. The goal is a thin layer that
catches bugs + accidents, not a zero-trust boundary.

Exposed:
  ToolPolicy         — per-agent tool allowlist + rate limits
  AgentAccessControl — registry + dispatch
"""

from __future__ import annotations

import logging
import threading
import time
from collections import defaultdict, deque
from dataclasses import dataclass, field
from typing import Any, Callable, Deque, Dict, Iterable, Optional

log = logging.getLogger("harvey.access_control")


class AccessDenied(RuntimeError):
    """Raised when an agent is not allowed to call a tool."""


@dataclass
class ToolPolicy:
    """
    Per-agent policy. An empty `allowed_tools` means "allow all"; any
    non-empty set restricts to just those tool names.
    """

    agent: str
    allowed_tools: frozenset = field(default_factory=frozenset)
    denied_tools: frozenset = field(default_factory=frozenset)
    # Token bucket: (capacity, refill_rate_per_sec)
    rate_limits: Dict[str, tuple[int, float]] = field(default_factory=dict)

    def allows(self, tool_name: str) -> bool:
        if tool_name in self.denied_tools:
            return False
        if self.allowed_tools and tool_name not in self.allowed_tools:
            return False
        return True


class _TokenBucket:
    """Simple thread-safe token bucket for rate limiting."""

    def __init__(self, capacity: int, refill_rate_per_sec: float):
        self.capacity = max(1, int(capacity))
        self.refill_rate = max(0.0001, float(refill_rate_per_sec))
        self._tokens = float(self.capacity)
        self._last_refill = time.time()
        self._lock = threading.Lock()

    def try_consume(self, n: float = 1.0) -> bool:
        with self._lock:
            now = time.time()
            elapsed = now - self._last_refill
            self._tokens = min(
                self.capacity, self._tokens + elapsed * self.refill_rate
            )
            self._last_refill = now
            if self._tokens >= n:
                self._tokens -= n
                return True
            return False

    def available(self) -> float:
        with self._lock:
            now = time.time()
            elapsed = now - self._last_refill
            return min(self.capacity, self._tokens + elapsed * self.refill_rate)


class AgentAccessControl:
    """
    Registry of per-agent tool policies + centralized check function.

    Typical wiring:
        ac = AgentAccessControl()
        ac.set_policy(ToolPolicy(
            agent="researcher",
            allowed_tools=frozenset({"brain_search", "superbrain_vector_search"}),
            rate_limits={"brain_search": (10, 2.0)},  # 10 burst, 2/sec
        ))
        ac.check("researcher", "brain_search")  # passes or raises AccessDenied
    """

    def __init__(self):
        self._policies: Dict[str, ToolPolicy] = {}
        self._buckets: Dict[tuple[str, str], _TokenBucket] = {}
        self._denials: Deque[dict] = deque(maxlen=256)
        self._counters: Dict[tuple[str, str], int] = defaultdict(int)
        self._lock = threading.RLock()

    # ── Policy management ──

    def set_policy(self, policy: ToolPolicy) -> None:
        with self._lock:
            self._policies[policy.agent] = policy
            # Instantiate token buckets for any declared rate limits
            for tool, (cap, rate) in policy.rate_limits.items():
                self._buckets[(policy.agent, tool)] = _TokenBucket(cap, rate)
            log.info(
                f"[access_control] policy set for {policy.agent}: "
                f"{len(policy.allowed_tools) or 'ALL'} allowed, "
                f"{len(policy.denied_tools)} denied, "
                f"{len(policy.rate_limits)} rate-limited"
            )

    def get_policy(self, agent: str) -> Optional[ToolPolicy]:
        return self._policies.get(agent)

    def remove_policy(self, agent: str) -> bool:
        with self._lock:
            removed = self._policies.pop(agent, None) is not None
            for key in list(self._buckets.keys()):
                if key[0] == agent:
                    del self._buckets[key]
            return removed

    # ── Enforcement ──

    def check(self, agent: str, tool: str) -> None:
        """
        Raise `AccessDenied` if the agent is not allowed to call the tool
        right now. Otherwise return None and increment the call counter.

        If no policy is registered for the agent, this is permissive
        (returns immediately). Phase 4 uses opt-in access control — a
        missing policy is not a denial.
        """
        policy = self._policies.get(agent)
        if policy is None:
            return
        if not policy.allows(tool):
            self._record_denial(agent, tool, "not_in_allowlist")
            raise AccessDenied(
                f"[{agent}] not allowed to call tool: {tool}"
            )
        bucket = self._buckets.get((agent, tool))
        if bucket is not None and not bucket.try_consume(1.0):
            self._record_denial(agent, tool, "rate_limited")
            raise AccessDenied(
                f"[{agent}] rate limit exceeded for tool: {tool}"
            )
        with self._lock:
            self._counters[(agent, tool)] += 1

    def allowed(self, agent: str, tool: str) -> bool:
        """Non-raising variant. Returns False on denial, True otherwise."""
        try:
            self.check(agent, tool)
            return True
        except AccessDenied:
            return False

    def wrap(
        self,
        agent: str,
        tool: str,
        fn: Callable[..., Any],
    ) -> Callable[..., Any]:
        """
        Return a wrapper around `fn` that runs the access check first.
        Handy for injecting into a subagent's tool dict.
        """
        def _wrapped(*args, **kwargs):
            self.check(agent, tool)
            return fn(*args, **kwargs)
        return _wrapped

    # ── Introspection ──

    def _record_denial(self, agent: str, tool: str, reason: str) -> None:
        with self._lock:
            self._denials.append({
                "agent": agent,
                "tool": tool,
                "reason": reason,
                "ts": time.time(),
            })
            log.warning(
                f"[access_control] DENY {agent} → {tool} ({reason})"
            )

    def recent_denials(self, n: int = 50) -> list[dict]:
        with self._lock:
            return list(self._denials)[-n:]

    def counters(self) -> Dict[str, int]:
        with self._lock:
            return {f"{a}/{t}": c for (a, t), c in self._counters.items()}

    def status(self) -> Dict[str, Any]:
        with self._lock:
            return {
                "policy_count": len(self._policies),
                "policies": sorted(self._policies.keys()),
                "bucket_count": len(self._buckets),
                "total_calls": sum(self._counters.values()),
                "total_denials": len(self._denials),
            }


# ── Module-level singleton ──

_default_access_control: Optional[AgentAccessControl] = None


def get_default_access_control() -> AgentAccessControl:
    """
    Get the module-level singleton AgentAccessControl.

    Permissive by default — no policies registered means every agent can
    call every tool. Call `set_policy()` on the returned instance to lock
    things down.
    """
    global _default_access_control
    if _default_access_control is None:
        _default_access_control = AgentAccessControl()
    return _default_access_control


def set_default_access_control(ac: AgentAccessControl) -> None:
    """Replace the singleton — useful for tests and per-process isolation."""
    global _default_access_control
    _default_access_control = ac


__all__ = [
    "AccessDenied",
    "ToolPolicy",
    "AgentAccessControl",
    "get_default_access_control",
    "set_default_access_control",
]
