#!/usr/bin/env python3
"""
SANCHO Task Registry — Defines, stores, and schedules proactive tasks.

Each ProactiveTask has:
  - A handler callable returning a result dict
  - An interval (minutes between runs)
  - A GateSystem of preconditions
  - Flags for read_only, brief output, and enabled state

The TaskRegistry manages registered tasks, checks eligibility
(interval + gates), runs tasks, and persists state to sancho_state.json.

Usage:
    registry = TaskRegistry()
    registry.register(ProactiveTask(
        name="wiki_lint",
        handler=handle_wiki_lint,
        interval_minutes=360,
        gates=GateSystem([time_gate(state, "wiki_lint", 6)]),
    ))

    for task in registry.eligible_tasks():
        result = registry.run_task(task)
"""

import json
import logging
import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from core.sancho.gates import GateSystem

log = logging.getLogger("harvey.sancho.tasks")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
STATE_FILE = Path(HARVEY_HOME) / "data" / "sancho_state.json"


@dataclass
class ProactiveTask:
    """A task that SANCHO can run proactively."""
    name: str
    handler: Callable[[], Dict[str, Any]]
    interval_minutes: int
    gates: GateSystem = field(default_factory=GateSystem)
    read_only: bool = True
    brief: bool = True
    enabled: bool = True


class TaskRegistry:
    """
    Registry of proactive tasks with state persistence.

    State layout in sancho_state.json:
        {
            "last_run": {"task_name": unix_timestamp, ...},
            "sessions_since": {"task_name": count, ...},
            "run_count": {"task_name": total_runs, ...}
        }
    """

    def __init__(self, state_file: Optional[Path] = None):
        self.state_file = state_file or STATE_FILE
        self.tasks: Dict[str, ProactiveTask] = {}
        self.state = self._load_state()

    def _load_state(self) -> dict:
        """Load persisted state from disk."""
        if self.state_file.exists():
            try:
                return json.loads(self.state_file.read_text())
            except (json.JSONDecodeError, OSError) as e:
                log.warning("Failed to load sancho state: %s", e)
        return {"last_run": {}, "sessions_since": {}, "run_count": {}}

    def _save_state(self) -> None:
        """Persist state to disk."""
        self.state_file.parent.mkdir(parents=True, exist_ok=True)
        self.state_file.write_text(json.dumps(self.state, indent=2))

    def register(self, task: ProactiveTask) -> None:
        """Register a task. Overwrites if name already exists."""
        self.tasks[task.name] = task
        log.debug("Registered task: %s (interval=%dm)", task.name, task.interval_minutes)

    def _interval_ok(self, task: ProactiveTask) -> bool:
        """Check if enough time has elapsed since the task's last run."""
        last = self.state.get("last_run", {}).get(task.name, 0)
        elapsed_min = (time.time() - last) / 60
        return elapsed_min >= task.interval_minutes

    def eligible_tasks(self) -> List[ProactiveTask]:
        """
        Return tasks that are enabled, past their interval, and pass all gates.
        """
        eligible = []
        for task in self.tasks.values():
            if not task.enabled:
                continue
            if not self._interval_ok(task):
                continue
            if not task.gates.all_pass():
                continue
            eligible.append(task)
        return eligible

    def run_task(self, task: ProactiveTask) -> Dict[str, Any]:
        """
        Execute a task's handler, update state, and return the result.

        Returns:
            {"name": str, "result": dict, "duration_sec": float, "success": bool}
        """
        start = time.time()
        try:
            result = task.handler()
            success = True
        except Exception as e:
            log.error("Task %s failed: %s", task.name, e)
            result = {"error": str(e)}
            success = False

        duration = round(time.time() - start, 2)

        # Update state
        self.state.setdefault("last_run", {})[task.name] = time.time()
        self.state.setdefault("sessions_since", {})[task.name] = 0
        run_count = self.state.setdefault("run_count", {}).get(task.name, 0)
        self.state["run_count"][task.name] = run_count + 1
        self._save_state()

        return {
            "name": task.name,
            "result": result,
            "duration_sec": duration,
            "success": success,
        }
