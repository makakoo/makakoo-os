"""
TaskMasterAgent — Coordinator-style agent that tracks other agents' progress.

Phase 2 deliverable. Unlike the other specialized agents, TaskMaster doesn't
do the real work — it subscribes to the event bus and maintains a progress
dict keyed by step_id. Useful for:

  - Status reports during long workflows
  - Stall detection (no events for N seconds)
  - Aggregate health for HarveyChat responses

`start_monitoring()` is called automatically by AgentCoordinator when
the agent is registered.
"""

from __future__ import annotations

import logging
import threading
import time
from typing import Any, Dict, List, Optional

from core.subagents.subagent import Subagent

log = logging.getLogger("harvey.subagent.task_master")


class TaskMasterAgent(Subagent):
    NAME = "task_master"
    ACTIONS = ["report_status", "wait_for_steps"]
    DESCRIPTION = "Tracks other agents' progress via event bus."

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self._progress: Dict[str, str] = {}
        self._last_event_at: float = 0.0
        self._lock = threading.RLock()
        self._subscribed = False

    # ─── Passive listener (called by coordinator on register) ────

    def start_monitoring(self) -> None:
        if self._subscribed:
            return
        self.event_bus.subscribe("agent.*.started", self._on_started)
        self.event_bus.subscribe("agent.*.completed", self._on_completed)
        self.event_bus.subscribe("agent.*.failed", self._on_failed)
        self._subscribed = True
        log.info("[task_master] monitoring agent.* events")

    def _on_started(self, event) -> None:
        if event.source == self.name:
            return  # don't track our own handle() calls
        step_id = (event.data or {}).get("step_id", "?")
        with self._lock:
            self._progress[step_id] = "running"
            self._last_event_at = time.time()

    def _on_completed(self, event) -> None:
        if event.source == self.name:
            return
        step_id = (event.data or {}).get("step_id", "?")
        with self._lock:
            self._progress[step_id] = "completed"
            self._last_event_at = time.time()

    def _on_failed(self, event) -> None:
        if event.source == self.name:
            return
        step_id = (event.data or {}).get("step_id", "?")
        with self._lock:
            self._progress[step_id] = "failed"
            self._last_event_at = time.time()

    # ─── Read-only accessors ─────────────────────────────────────

    def snapshot(self) -> Dict[str, str]:
        with self._lock:
            return dict(self._progress)

    def last_event_age_seconds(self) -> float:
        with self._lock:
            if self._last_event_at == 0.0:
                return float("inf")
            return time.time() - self._last_event_at

    # ─── DAG step handler ────────────────────────────────────────

    def execute(self, step, ctx: Dict) -> Dict[str, Any]:
        if step.action == "report_status":
            snap = self.snapshot()
            return {
                "ok": True,
                "progress": snap,
                "count": len(snap),
                "running": sum(1 for v in snap.values() if v == "running"),
                "completed": sum(1 for v in snap.values() if v == "completed"),
                "failed": sum(1 for v in snap.values() if v == "failed"),
                "last_event_age_s": self.last_event_age_seconds(),
            }

        if step.action == "wait_for_steps":
            targets: List[str] = ctx.get("wait_for_step_ids", [])
            timeout = float(ctx.get("timeout_seconds", 30.0))
            deadline = time.time() + timeout

            while time.time() < deadline:
                with self._lock:
                    done = all(
                        self._progress.get(t) in ("completed", "failed")
                        for t in targets
                    )
                if done:
                    return {
                        "ok": True,
                        "waited_for": targets,
                        "final": self.snapshot(),
                    }
                time.sleep(0.05)

            return {
                "ok": False,
                "error": "timeout waiting for steps",
                "waited_for": targets,
                "final": self.snapshot(),
            }

        return {"ok": False, "error": f"unknown action: {step.action}"}


__all__ = ["TaskMasterAgent"]
