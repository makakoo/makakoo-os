"""
failure_recovery.py — Phase 3 deliverable

FailureRecovery + CircuitBreaker: protects the swarm from cascading
failures. Each agent (identified by NAME) gets its own breaker. When an
agent's failure count hits `failure_threshold`, the breaker opens and
further calls to `allow()` return False until `reset_timeout_s` elapses,
at which point it moves to HALF_OPEN and lets one probe request through.

The class is designed to plug into two places:

  1. PersistentEventBus — subscribe to `agent.*.failed` and record
     failures automatically.
  2. AgentCoordinator — called before dispatching work to check
     `recovery.should_dispatch(agent_name)`.

Retry policy is separate and stateless: `retry_delay_s(attempt)` returns
exponential backoff, and `should_retry(attempt)` checks the max.

Exposed:
  - CircuitState (Enum)
  - CircuitBreaker (per-agent state machine)
  - FailureRecovery (registry + retry policy + event-bus integration)
"""

from __future__ import annotations

import logging
import threading
import time
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional

log = logging.getLogger("harvey.failure_recovery")


# ─── Circuit state machine ──────────────────────────────────────────


class CircuitState(Enum):
    CLOSED = "closed"         # normal; traffic passes
    OPEN = "open"             # tripped; traffic blocked
    HALF_OPEN = "half_open"   # probing; single trial call allowed


@dataclass
class CircuitBreaker:
    """
    Per-agent breaker. Thread-safe via an internal lock because event bus
    callbacks may fire from arbitrary threads.
    """

    name: str
    failure_threshold: int = 3
    reset_timeout_s: float = 30.0
    state: CircuitState = CircuitState.CLOSED
    failures: int = 0
    total_failures: int = 0
    total_successes: int = 0
    last_failure_at: float = 0.0
    opened_at: float = 0.0

    _lock: threading.RLock = field(default_factory=threading.RLock, repr=False)

    def record_success(self) -> None:
        with self._lock:
            self.total_successes += 1
            # Any success resets the failure counter and closes the breaker
            self.failures = 0
            if self.state != CircuitState.CLOSED:
                log.info(f"[breaker:{self.name}] closing (success probe)")
                self.state = CircuitState.CLOSED
                self.opened_at = 0.0

    def record_failure(self) -> None:
        with self._lock:
            self.failures += 1
            self.total_failures += 1
            self.last_failure_at = time.time()
            if self.state == CircuitState.HALF_OPEN:
                # Probe failed — re-open
                self._trip("half-open probe failed")
                return
            if (
                self.state == CircuitState.CLOSED
                and self.failures >= self.failure_threshold
            ):
                self._trip(f"{self.failures} consecutive failures")

    def _trip(self, reason: str) -> None:
        log.warning(f"[breaker:{self.name}] OPEN ({reason})")
        self.state = CircuitState.OPEN
        self.opened_at = time.time()

    def allow(self) -> bool:
        """
        Should a dispatch attempt be allowed? Handles the OPEN → HALF_OPEN
        transition automatically when reset_timeout_s has elapsed.
        """
        with self._lock:
            if self.state == CircuitState.CLOSED:
                return True
            if self.state == CircuitState.OPEN:
                if time.time() - self.opened_at >= self.reset_timeout_s:
                    log.info(f"[breaker:{self.name}] → HALF_OPEN (probe)")
                    self.state = CircuitState.HALF_OPEN
                    return True
                return False
            # HALF_OPEN
            return False  # only one probe in flight; subsequent calls blocked

    def force_close(self) -> None:
        with self._lock:
            self.state = CircuitState.CLOSED
            self.failures = 0
            self.opened_at = 0.0

    def status(self) -> Dict[str, Any]:
        with self._lock:
            return {
                "name": self.name,
                "state": self.state.value,
                "failures": self.failures,
                "total_failures": self.total_failures,
                "total_successes": self.total_successes,
                "failure_threshold": self.failure_threshold,
                "reset_timeout_s": self.reset_timeout_s,
                "last_failure_at": self.last_failure_at,
                "opened_at": self.opened_at,
                "seconds_until_half_open": max(
                    0.0,
                    self.reset_timeout_s - (time.time() - self.opened_at),
                ) if self.state == CircuitState.OPEN else 0.0,
            }


# ─── Recovery manager ───────────────────────────────────────────────


class FailureRecovery:
    """
    Central registry of per-agent breakers + retry policy + event bus glue.

    Typical wiring:

        recovery = FailureRecovery(event_bus=bus, coordinator=coord)
        recovery.start_listening()  # auto-record failures/successes
        ...
        if recovery.should_dispatch("researcher"):
            executor.dispatch(...)
    """

    def __init__(
        self,
        event_bus=None,
        coordinator=None,
        failure_threshold: int = 3,
        reset_timeout_s: float = 30.0,
        max_retries: int = 3,
        base_retry_delay_s: float = 1.0,
        max_retry_delay_s: float = 30.0,
        auto_restart_subprocess: bool = False,
    ):
        """
        Args:
          event_bus: PersistentEventBus (optional). If provided,
            `start_listening()` will subscribe to agent.*.failed + completed.
          coordinator: AgentCoordinator (optional). Used to look up
            registered agents and optionally re-register after restart.
          failure_threshold: consecutive failures to trip a breaker.
          reset_timeout_s: OPEN → HALF_OPEN probe delay.
          max_retries: max retry attempts per dispatched task.
          base_retry_delay_s: first retry delay; doubles each attempt.
          max_retry_delay_s: cap on retry delay.
          auto_restart_subprocess: if True, route crashed subprocess
            agents through AgentLifecycle.spawn() on breaker open.
            Default False since Phase 2 agents are in-process.
        """
        self.event_bus = event_bus
        self.coordinator = coordinator
        self.failure_threshold = int(failure_threshold)
        self.reset_timeout_s = float(reset_timeout_s)
        self.max_retries = int(max_retries)
        self.base_retry_delay_s = float(base_retry_delay_s)
        self.max_retry_delay_s = float(max_retry_delay_s)
        self.auto_restart_subprocess = bool(auto_restart_subprocess)

        self._breakers: Dict[str, CircuitBreaker] = {}
        self._lock = threading.RLock()
        self._subscribed = False

    # ── Breaker access ──

    def get_breaker(self, agent_name: str) -> CircuitBreaker:
        """Get or lazily create the breaker for `agent_name`."""
        with self._lock:
            if agent_name not in self._breakers:
                self._breakers[agent_name] = CircuitBreaker(
                    name=agent_name,
                    failure_threshold=self.failure_threshold,
                    reset_timeout_s=self.reset_timeout_s,
                )
            return self._breakers[agent_name]

    def should_dispatch(self, agent_name: str) -> bool:
        """Check breaker state before dispatching work."""
        return self.get_breaker(agent_name).allow()

    def record_success(self, agent_name: str) -> None:
        self.get_breaker(agent_name).record_success()

    def record_failure(self, agent_name: str) -> None:
        breaker = self.get_breaker(agent_name)
        was_closed = breaker.state == CircuitState.CLOSED
        breaker.record_failure()
        # If breaker just opened and we have a subprocess supervisor, restart
        if (
            was_closed
            and breaker.state == CircuitState.OPEN
            and self.auto_restart_subprocess
        ):
            self._try_subprocess_restart(agent_name)

    # ── Retry policy ──

    def should_retry(self, attempt: int) -> bool:
        """
        Should we retry a task that just failed?
        `attempt` is 1-indexed (1 = first retry after the initial failure).
        """
        return 1 <= attempt <= self.max_retries

    def retry_delay_s(self, attempt: int) -> float:
        """
        Exponential backoff: base * 2^(attempt-1), capped at max_retry_delay_s.
        """
        if attempt < 1:
            return 0.0
        delay = self.base_retry_delay_s * (2 ** (attempt - 1))
        return min(delay, self.max_retry_delay_s)

    # ── Event bus integration ──

    def start_listening(self) -> None:
        """
        Subscribe to agent.*.failed and agent.*.completed on the event bus
        and update breakers automatically. Idempotent.
        """
        if self._subscribed or self.event_bus is None:
            return
        self.event_bus.subscribe("agent.*.completed", self._on_completed)
        self.event_bus.subscribe("agent.*.failed", self._on_failed)
        self._subscribed = True
        log.info("[failure_recovery] listening on agent.*.completed/failed")

    def _on_completed(self, event) -> None:
        agent_name = self._agent_from_source_or_topic(event)
        if agent_name:
            self.record_success(agent_name)

    def _on_failed(self, event) -> None:
        agent_name = self._agent_from_source_or_topic(event)
        if agent_name:
            self.record_failure(agent_name)

    @staticmethod
    def _agent_from_source_or_topic(event) -> Optional[str]:
        """
        Extract the agent name from event.source (preferred) or parse the
        topic `agent.{name}.{verb}`.
        """
        source = getattr(event, "source", None)
        if source:
            return source
        topic = getattr(event, "type", "")
        parts = topic.split(".")
        if len(parts) >= 3 and parts[0] == "agent":
            return parts[1]
        return None

    # ── Subprocess restart (Phase 3 bonus path) ──

    def _try_subprocess_restart(self, agent_name: str) -> None:
        """
        If the breaker opened and auto_restart_subprocess is on, ask
        AgentLifecycle to respawn the agent. Silently no-ops if
        AgentLifecycle isn't reachable or the agent isn't registered there.
        """
        try:
            from core.orchestration.agent_lifecycle import get_lifecycle
            lifecycle = get_lifecycle()
            if lifecycle.status(agent_name) is None:
                return  # not a subprocess agent
            log.warning(
                f"[failure_recovery] breaker open → restarting {agent_name}"
            )
            import asyncio
            loop = asyncio.get_event_loop() if asyncio.get_event_loop().is_running() else None
            if loop:
                loop.create_task(lifecycle.spawn(agent_name))
            else:
                asyncio.run(lifecycle.spawn(agent_name))
        except Exception as e:
            log.warning(f"[failure_recovery] subprocess restart failed: {e}")

    # ── Status ──

    def status(self) -> Dict[str, Any]:
        with self._lock:
            return {
                "breaker_count": len(self._breakers),
                "breakers": {
                    name: b.status() for name, b in self._breakers.items()
                },
                "max_retries": self.max_retries,
                "base_retry_delay_s": self.base_retry_delay_s,
                "max_retry_delay_s": self.max_retry_delay_s,
            }

    def tripped_agents(self) -> List[str]:
        with self._lock:
            return [
                name for name, b in self._breakers.items()
                if b.state == CircuitState.OPEN
            ]


__all__ = ["CircuitState", "CircuitBreaker", "FailureRecovery"]
