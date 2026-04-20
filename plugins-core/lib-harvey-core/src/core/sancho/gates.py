#!/usr/bin/env python3
"""
SANCHO Gate System — Generalized precondition gates for proactive tasks.

Gates are composable checks that must all pass before a task can run.
Each gate is a simple callable returning bool, wrapped in a Gate dataclass
for introspection and status reporting.

Built-in factory functions cover the common patterns:
  - time_gate:    minimum hours since last execution
  - session_gate: minimum sessions since last execution
  - lock_gate:    filesystem lock prevents concurrent runs

Usage:
    gs = GateSystem([
        time_gate(state, "dream", min_hours=4),
        session_gate(state, "dream", min_sessions=3),
        lock_gate("/tmp/dream.lock"),
    ])

    if gs.all_pass():
        run_the_thing()
    else:
        print(gs.status())  # Which gates failed and why
"""

import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Dict, List, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


@dataclass
class Gate:
    """A single precondition check."""
    name: str
    check: Callable[[], bool]
    description: str = ""

    def passes(self) -> bool:
        """Run the check. Returns True if the gate allows execution."""
        try:
            return self.check()
        except Exception:
            return False


class GateSystem:
    """
    Ordered collection of gates that must all pass.

    Gates are evaluated in order. Short-circuits on first failure
    when using all_pass(), but status() evaluates all for diagnostics.
    """

    def __init__(self, gates: Optional[List[Gate]] = None):
        self.gates: List[Gate] = gates or []

    def add(self, gate: Gate) -> "GateSystem":
        """Add a gate. Returns self for chaining."""
        self.gates.append(gate)
        return self

    def all_pass(self) -> bool:
        """True if every gate passes."""
        return all(g.passes() for g in self.gates)

    def status(self) -> Dict[str, dict]:
        """
        Evaluate all gates and return a diagnostic dict.

        Returns:
            {"gate_name": {"passed": bool, "description": str}, ...}
        """
        result = {}
        for g in self.gates:
            result[g.name] = {
                "passed": g.passes(),
                "description": g.description,
            }
        return result

    def __len__(self) -> int:
        return len(self.gates)

    def __repr__(self) -> str:
        passed = sum(1 for g in self.gates if g.passes())
        return f"GateSystem({passed}/{len(self.gates)} pass)"


# ═══════════════════════════════════════════════════════════════════
#  Factory functions
# ═══════════════════════════════════════════════════════════════════


def time_gate(state: dict, key: str, min_hours: float) -> Gate:
    """
    Gate that requires at least `min_hours` since the last run.

    Args:
        state:     Shared state dict (loaded from sancho_state.json).
        key:       Task key to look up in state["last_run"].
        min_hours: Minimum hours between runs.
    """
    def check() -> bool:
        last_run = state.get("last_run", {}).get(key, 0)
        elapsed_hours = (time.time() - last_run) / 3600
        return elapsed_hours >= min_hours

    return Gate(
        name=f"time:{key}",
        check=check,
        description=f"At least {min_hours}h since last {key} run",
    )


def session_gate(state: dict, key: str, min_sessions: int) -> Gate:
    """
    Gate that requires at least `min_sessions` since the last run.

    Args:
        state:        Shared state dict.
        key:          Task key to look up in state["sessions_since"].
        min_sessions: Minimum session count.
    """
    def check() -> bool:
        sessions = state.get("sessions_since", {}).get(key, 0)
        return sessions >= min_sessions

    return Gate(
        name=f"sessions:{key}",
        check=check,
        description=f"At least {min_sessions} sessions since last {key}",
    )


def lock_gate(lock_path: str) -> Gate:
    """
    Gate that fails if a lock file exists (prevents concurrent runs).

    Args:
        lock_path: Filesystem path to the lock file.
    """
    def check() -> bool:
        return not Path(lock_path).exists()

    return Gate(
        name=f"lock:{Path(lock_path).name}",
        check=check,
        description=f"No active lock at {lock_path}",
    )


def active_hours_gate(
    start: str,
    end: str,
    task_name: str = "",
    *,
    now_provider: Optional[Callable[[], "datetime"]] = None,
) -> Gate:
    """Gate that only passes during a wall-clock window.

    Used to suppress noisy outbound tasks (Telegram alerts, email drafts)
    outside a "polite" window. When the current time falls outside the
    window, the gate emits a `sancho.gate.suppressed` event before
    returning False — so visibility is preserved (you can see what
    SANCHO would have run if the gate hadn't blocked it). Silent
    swallowing is the wrong design.

    Args:
        start: HH:MM string in 24h format, e.g. "08:00"
        end:   HH:MM string in 24h format, e.g. "22:00"
        task_name: Optional task identifier for the suppression event payload.
        now_provider: Test injection point. Defaults to datetime.now.

    Window semantics:
        - start < end: simple window (e.g. 08:00–22:00)
        - start > end: wraps midnight (e.g. 22:00–06:00)
        - start == end: always passes (degenerate "window of zero" → always-on)

    Example:
        registry.register(ProactiveTask(
            name="daily_briefing",
            handler=handle_daily_briefing,
            interval_minutes=480,
            gates=GateSystem([
                time_gate(state, "daily_briefing", min_hours=8),
                active_hours_gate("07:00", "22:00", task_name="daily_briefing"),
            ]),
        ))
    """
    from datetime import datetime as _dt

    def _parse_hhmm(s: str) -> tuple:
        h, m = s.split(":")
        return int(h), int(m)

    start_h, start_m = _parse_hhmm(start)
    end_h, end_m = _parse_hhmm(end)
    start_minutes = start_h * 60 + start_m
    end_minutes = end_h * 60 + end_m

    def _now_minutes() -> int:
        now = (now_provider or _dt.now)()
        return now.hour * 60 + now.minute

    def _in_window() -> bool:
        cur = _now_minutes()
        if start_minutes == end_minutes:
            return True  # always-on
        if start_minutes < end_minutes:
            return start_minutes <= cur < end_minutes
        # Wraps midnight
        return cur >= start_minutes or cur < end_minutes

    def check() -> bool:
        if _in_window():
            return True
        # Emit suppression event for visibility — operators can see what
        # was due-but-blocked. Best-effort, never fatal.
        try:
            from core.events.event_stream import EventBus
            EventBus.instance().publish(
                "sancho.gate.suppressed",
                source="active_hours_gate",
                task=task_name,
                gate="active_hours",
                window=f"{start}-{end}",
                now_minutes=_now_minutes(),
            )
        except Exception:
            pass
        return False

    return Gate(
        name=f"active_hours:{task_name}" if task_name else "active_hours",
        check=check,
        description=(
            f"Current time must be within {start}–{end} "
            f"(wraps midnight: {start_minutes > end_minutes})"
        ),
    )


def artifact_gate(store, artifact_name: str) -> Gate:
    """
    Gate that passes only when a named artifact exists in the ArtifactStore.

    Enables dependency-gated scheduling: a SANCHO task can wait until
    another workflow publishes its output artifact.

    Args:
        store:         An ArtifactStore instance (from
                       core.orchestration.artifact_store).
        artifact_name: The artifact name to wait for.

    Example:
        from core.orchestration.artifact_store import get_default_store
        store = get_default_store()
        registry.register(ProactiveTask(
            name="synthesize_research",
            handler=handle_synthesis,
            interval_minutes=60,
            gates=GateSystem([
                artifact_gate(store, "research_campaign:final_extraction"),
            ]),
        ))
    """
    def check() -> bool:
        try:
            return store.exists(artifact_name)
        except Exception:
            return False

    return Gate(
        name=f"artifact:{artifact_name}",
        check=check,
        description=f"Artifact '{artifact_name}' must be published",
    )
