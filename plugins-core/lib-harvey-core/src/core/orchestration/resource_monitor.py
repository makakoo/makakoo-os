"""
resource_monitor.py — Phase 3 deliverable

ResourceMonitor: samples CPU + memory via psutil, tracks running agent
count, and returns scale recommendations for the coordinator. Designed
to be called by the IntelligentRouter or a scaling loop before spawning
a team, and also pollable during a long-running workflow.

Hysteresis is built in: scale_down only triggers when two consecutive
samples agree, to prevent oscillation.

psutil is a soft dependency — if it's not installed, CPU/memory come
back as 0.0 and the monitor degrades to agent-count-only decisions.

Exposed:
  - ResourceSnapshot (dataclass)
  - ScaleDecision (dataclass)
  - ResourceMonitor
"""

from __future__ import annotations

import logging
import time
from collections import deque
from dataclasses import dataclass, field
from typing import Any, Deque, Dict, Optional

log = logging.getLogger("harvey.resource_monitor")

try:
    import psutil
    _HAS_PSUTIL = True
except ImportError:
    psutil = None  # type: ignore
    _HAS_PSUTIL = False
    log.warning("psutil not installed — CPU/memory metrics will be zero")


# ─── Data model ─────────────────────────────────────────────────────


@dataclass
class ResourceSnapshot:
    cpu_percent: float               # 0-100, whole system
    mem_percent: float               # 0-100, whole system
    running_agents: int              # currently registered subagents
    in_flight_steps: int             # steps the DAG executor is running
    timestamp: float = field(default_factory=time.time)

    def is_loaded(self, cpu_ceiling: float, mem_ceiling: float) -> bool:
        return self.cpu_percent >= cpu_ceiling or self.mem_percent >= mem_ceiling

    def is_idle(self, cpu_floor: float, mem_floor: float) -> bool:
        return self.cpu_percent < cpu_floor and self.mem_percent < mem_floor


@dataclass
class ScaleDecision:
    action: str                      # "scale_up" | "scale_down" | "hold"
    target: int                      # recommended parallelism after action
    current: int                     # parallelism before action
    reason: str
    snapshot: Optional[ResourceSnapshot] = None


# ─── Monitor ────────────────────────────────────────────────────────


class ResourceMonitor:
    """
    Lightweight resource governor. Does NOT spawn or kill agents by itself —
    it only returns recommendations. The caller decides whether to act.
    """

    def __init__(
        self,
        coordinator=None,
        executor=None,
        min_parallelism: int = 1,
        max_parallelism: int = 8,
        cpu_ceiling: float = 80.0,
        cpu_floor: float = 30.0,
        mem_ceiling: float = 85.0,
        mem_floor: float = 40.0,
        hysteresis_samples: int = 2,
    ):
        """
        Args:
          coordinator: AgentCoordinator — read agent_count from status().
          executor: AsyncDAGExecutor — read in_flight from its internal
            futures dict if exposed (optional).
          min_parallelism / max_parallelism: hard bounds on scaling.
          cpu_ceiling: scale down above this CPU%.
          cpu_floor: scale up is allowed below this CPU%.
          mem_ceiling / mem_floor: same for memory.
          hysteresis_samples: how many consecutive agreeing samples before
            a scale_down triggers. Scale_up fires immediately.
        """
        self.coordinator = coordinator
        self.executor = executor
        self.min_parallelism = max(1, int(min_parallelism))
        self.max_parallelism = max(self.min_parallelism, int(max_parallelism))
        self.cpu_ceiling = float(cpu_ceiling)
        self.cpu_floor = float(cpu_floor)
        self.mem_ceiling = float(mem_ceiling)
        self.mem_floor = float(mem_floor)
        self.hysteresis_samples = max(1, int(hysteresis_samples))

        self._history: Deque[ResourceSnapshot] = deque(maxlen=64)

    # ── Sampling ──

    def sample(self) -> ResourceSnapshot:
        """Take one snapshot. Pushes onto internal history for hysteresis."""
        cpu = 0.0
        mem = 0.0
        if _HAS_PSUTIL:
            try:
                # interval=None reads cached last value — non-blocking
                cpu = float(psutil.cpu_percent(interval=None))
                mem = float(psutil.virtual_memory().percent)
            except Exception as e:
                log.warning(f"psutil sample failed: {e}")

        running_agents = 0
        if self.coordinator is not None:
            try:
                running_agents = len(self.coordinator.list_agents())
            except Exception:
                pass

        in_flight = 0
        if self.executor is not None:
            # AsyncDAGExecutor doesn't expose a public method for this yet;
            # duck-type check a couple of common attribute names.
            for attr in ("_in_flight", "_running_steps", "_futures"):
                val = getattr(self.executor, attr, None)
                if val is not None:
                    try:
                        in_flight = len(val)
                        break
                    except TypeError:
                        pass

        snap = ResourceSnapshot(
            cpu_percent=cpu,
            mem_percent=mem,
            running_agents=running_agents,
            in_flight_steps=in_flight,
        )
        self._history.append(snap)
        return snap

    # ── Decisions ──

    def recommend_scaling(
        self,
        current_parallelism: int,
        snapshot: Optional[ResourceSnapshot] = None,
    ) -> ScaleDecision:
        """
        Decide whether to scale the next team's parallelism up, down, or hold.

        Rules:
          - If snap.is_loaded and parallelism > min → scale_down (needs N
            consecutive agreeing samples = hysteresis).
          - If snap.is_idle and parallelism < max → scale_up (fires on
            one sample, since ramping up under load is urgent).
          - Else hold.
        """
        if snapshot is None:
            snapshot = self.sample()

        current = max(self.min_parallelism, min(current_parallelism, self.max_parallelism))

        if snapshot.is_loaded(self.cpu_ceiling, self.mem_ceiling):
            if current > self.min_parallelism and self._consecutive_loaded():
                target = max(self.min_parallelism, current - 1)
                return ScaleDecision(
                    action="scale_down",
                    target=target,
                    current=current,
                    reason=(
                        f"CPU={snapshot.cpu_percent:.0f}% MEM={snapshot.mem_percent:.0f}% "
                        f">= ceiling for {self.hysteresis_samples} samples"
                    ),
                    snapshot=snapshot,
                )
            return ScaleDecision(
                action="hold",
                target=current,
                current=current,
                reason=(
                    f"loaded but already at min ({self.min_parallelism}) "
                    f"or hysteresis not satisfied"
                ),
                snapshot=snapshot,
            )

        if snapshot.is_idle(self.cpu_floor, self.mem_floor) and current < self.max_parallelism:
            target = min(self.max_parallelism, current + 1)
            return ScaleDecision(
                action="scale_up",
                target=target,
                current=current,
                reason=(
                    f"CPU={snapshot.cpu_percent:.0f}% MEM={snapshot.mem_percent:.0f}% "
                    f"< floor — headroom to scale"
                ),
                snapshot=snapshot,
            )

        return ScaleDecision(
            action="hold",
            target=current,
            current=current,
            reason="within normal operating band",
            snapshot=snapshot,
        )

    def _consecutive_loaded(self) -> bool:
        """Hysteresis: last N samples all loaded?"""
        if len(self._history) < self.hysteresis_samples:
            return False
        recent = list(self._history)[-self.hysteresis_samples:]
        return all(
            s.is_loaded(self.cpu_ceiling, self.mem_ceiling) for s in recent
        )

    # ── Accessors ──

    def history(self) -> list[ResourceSnapshot]:
        return list(self._history)

    def latest(self) -> Optional[ResourceSnapshot]:
        return self._history[-1] if self._history else None

    def summary(self) -> Dict[str, Any]:
        latest = self.latest()
        return {
            "psutil_available": _HAS_PSUTIL,
            "min_parallelism": self.min_parallelism,
            "max_parallelism": self.max_parallelism,
            "cpu_ceiling": self.cpu_ceiling,
            "cpu_floor": self.cpu_floor,
            "mem_ceiling": self.mem_ceiling,
            "mem_floor": self.mem_floor,
            "samples_taken": len(self._history),
            "latest": {
                "cpu_percent": latest.cpu_percent if latest else None,
                "mem_percent": latest.mem_percent if latest else None,
                "running_agents": latest.running_agents if latest else None,
                "in_flight_steps": latest.in_flight_steps if latest else None,
            } if latest else None,
        }


__all__ = ["ResourceSnapshot", "ScaleDecision", "ResourceMonitor"]
