#!/usr/bin/env python3
"""
SANCHO Proactive Engine — Autonomous task scheduling for Harvey OS.

SANCHO ("the right moment") runs proactive maintenance and enrichment
tasks when conditions are met. It registers built-in tasks at init,
checks eligibility on each tick, runs what's due, logs results to the
Brain journal, and publishes events to the EventBus.

Usage:
    from core.sancho import Sancho

    sancho = Sancho()
    results = sancho.tick()       # Run all eligible tasks
    results = sancho.run_once()   # Same as tick, convenience alias

    # Or as CLI:
    python3 -m core.sancho.engine
"""

import logging
import os
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional

from core.sancho.gates import GateSystem, active_hours_gate, lock_gate, session_gate, time_gate
from core.sancho.handlers import (
    handle_daily_briefing,
    handle_dream,
    handle_dynamic_checklist,
    handle_graph_rebuild,
    handle_gym_classify,
    handle_gym_hypothesize,
    handle_gym_lope_gate,
    handle_gym_morning_report,
    handle_index_rebuild,
    handle_mascot_patrol,
    handle_wiki_lint,
    handle_memory_consolidation,
    handle_memory_promotion,
    handle_superbrain_sync_embed,
)
from core.sancho.tasks import ProactiveTask, TaskRegistry

log = logging.getLogger("harvey.sancho")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
LOCK_DIR = Path(HARVEY_HOME) / "tmp"


class Sancho:
    """
    The SANCHO proactive engine.

    Manages a TaskRegistry of built-in and custom tasks.
    On each tick(), runs eligible tasks, logs to the journal,
    and publishes events via the EventBus.
    """

    def __init__(self, state_file: Optional[Path] = None, subscribe_wake: bool = True):
        self.registry = TaskRegistry(state_file=state_file)
        # Tick lock: serializes concurrent ticks. A `sancho.wake` event
        # arriving while tick() is already running is coalesced — the
        # wake handler sets a `pending_wake` flag instead of blocking on
        # the lock, so we drain exactly one extra tick after the current
        # one finishes (no matter how many wakes queued up during it).
        self._tick_lock = threading.Lock()
        self._pending_wake = False
        self._register_builtins()
        if subscribe_wake:
            self._subscribe_wake_topic()

    def _subscribe_wake_topic(self) -> None:
        """Subscribe to `sancho.wake` on the EventBus for interrupt-driven ticks.

        Any component can call `EventBus.instance().publish("sancho.wake", ...)`
        to preempt the hourly OS schedule and trigger an immediate tick.
        Handler is non-blocking and serialized by `_tick_lock` — concurrent
        wake+tick scenarios are coalesced, not raced.

        Failure to subscribe is non-fatal; SANCHO still runs on its OS
        schedule, just without the dynamic-wake capability.
        """
        try:
            from core.events.event_stream import EventBus
            bus = EventBus.instance()
            bus.subscribe("sancho.wake", self._on_wake)
            log.info("SANCHO subscribed to sancho.wake (dynamic wakes enabled)")
        except Exception as e:
            log.debug(f"SANCHO wake subscription failed (non-fatal): {e}")

    def _on_wake(self, event) -> None:
        """Handle a `sancho.wake` event — preempt the hourly schedule.

        Runs on the EventBus dispatch thread. If a tick is already in
        progress, sets `_pending_wake` so the finishing tick triggers
        exactly one more pass (multiple wakes during a single tick
        coalesce to one). Otherwise, runs tick() immediately.

        Never raises — EventBus dispatch must not fail on handler errors.
        """
        try:
            source = getattr(event, "source", "") or ""
            log.info(f"SANCHO wake received (source={source!r})")

            if not self._tick_lock.acquire(blocking=False):
                # Tick already running — coalesce this wake
                self._pending_wake = True
                log.debug("SANCHO wake coalesced — tick already in progress")
                return

            try:
                self._pending_wake = False
                self._tick_unlocked()
                # Drain any wakes that arrived during the tick (at most one more)
                if self._pending_wake:
                    self._pending_wake = False
                    log.info("SANCHO draining pending wake after tick")
                    self._tick_unlocked()
            finally:
                self._tick_lock.release()
        except Exception as e:
            log.warning(f"SANCHO wake handler failed: {e}")

    def _register_builtins(self) -> None:
        """Register the built-in proactive tasks."""
        state = self.registry.state

        self.registry.register(ProactiveTask(
            name="dream",
            handler=handle_dream,
            interval_minutes=240,  # 4 hours
            gates=GateSystem([
                time_gate(state, "dream", min_hours=4),
                session_gate(state, "dream", min_sessions=3),
                lock_gate(str(LOCK_DIR / "dream.lock")),
            ]),
            read_only=False,
            brief=True,
        ))

        self.registry.register(ProactiveTask(
            name="wiki_lint",
            handler=handle_wiki_lint,
            interval_minutes=360,  # 6 hours
            gates=GateSystem([
                time_gate(state, "wiki_lint", min_hours=6),
            ]),
            read_only=True,
            brief=True,
        ))

        self.registry.register(ProactiveTask(
            name="index_rebuild",
            handler=handle_index_rebuild,
            interval_minutes=720,  # 12 hours
            gates=GateSystem([
                time_gate(state, "index_rebuild", min_hours=12),
            ]),
            read_only=False,
            brief=True,
        ))

        self.registry.register(ProactiveTask(
            name="daily_briefing",
            handler=handle_daily_briefing,
            interval_minutes=480,  # 8 hours
            gates=GateSystem([
                time_gate(state, "daily_briefing", min_hours=8),
            ]),
            read_only=True,
            brief=True,
        ))

        # Mascot Patrol — Pixel / Cinder / Ziggy / Glimmer run their
        # personality-matched quality checks. All four are read-only and
        # cheap (only scan recent edits + recent log tails), so we run
        # every 2 hours. Active-hours gate keeps the output aligned with
        # when Sebastian is awake to see the flavor lines.
        self.registry.register(ProactiveTask(
            name="mascot_patrol",
            handler=handle_mascot_patrol,
            interval_minutes=120,
            gates=GateSystem([
                time_gate(state, "mascot_patrol", min_hours=2),
                active_hours_gate("07:00", "23:00", task_name="mascot_patrol"),
            ]),
            read_only=True,
            brief=True,
        ))

        self.registry.register(ProactiveTask(
            name="memory_consolidation",
            handler=handle_memory_consolidation,
            interval_minutes=240,  # 4 hours
            gates=GateSystem([
                time_gate(state, "memory_consolidation", min_hours=4),
            ]),
            read_only=False,
            brief=True,
        ))

        self.registry.register(ProactiveTask(
            name="memory_promotion",
            handler=handle_memory_promotion,
            interval_minutes=1440,  # 24 hours — daily promotion sweep
            gates=GateSystem([
                time_gate(state, "memory_promotion", min_hours=20),
            ]),
            read_only=False,
            brief=False,
        ))

        self.registry.register(ProactiveTask(
            name="graph_rebuild",
            handler=handle_graph_rebuild,
            interval_minutes=360,  # 6 hours
            gates=GateSystem([
                time_gate(state, "graph_rebuild", min_hours=6),
            ]),
            read_only=False,
            brief=True,
        ))

        self.registry.register(ProactiveTask(
            name="superbrain_sync_embed",
            handler=handle_superbrain_sync_embed,
            interval_minutes=15,  # v4.1: 15 min — safe because sync_brain is
                                  # content-hash incremental (unchanged files
                                  # skip). Catches auto-memory writes from
                                  # any CLI within one interval.
            gates=GateSystem([
                time_gate(state, "superbrain_sync_embed", min_hours=0.2),  # ~12 min
            ]),
            read_only=False,
            brief=True,
        ))

        # H2/H4: Reactive HEARTBEAT.md evaluator. Cheap by design — the
        # handler short-circuits on hash-unchanged, so the 60m interval is
        # a safety floor, not a budget driver. Active hours gate keeps the
        # signal-emission window aligned with when Sebastian is around.
        self.registry.register(ProactiveTask(
            name="dynamic_checklist",
            handler=handle_dynamic_checklist,
            interval_minutes=60,
            gates=GateSystem([
                time_gate(state, "dynamic_checklist", min_hours=1),
                active_hours_gate("08:00", "22:00", task_name="dynamic_checklist"),
            ]),
            read_only=True,
            brief=True,
        ))

        # Harvey's Mascot GYM — Layer 2: classify + cluster today's errors.
        # Hourly, rules-based, no LLM, no writes outside data/errors/.
        # This is the input pipeline for the nightly hypothesis generator.
        self.registry.register(ProactiveTask(
            name="gym_classify",
            handler=handle_gym_classify,
            interval_minutes=60,
            gates=GateSystem([
                time_gate(state, "gym_classify", min_hours=0.9),
            ]),
            read_only=True,
            brief=True,
        ))

        # Harvey's Mascot GYM — Layer 3: nightly hypothesis generator.
        # Runs autoimprover + meta-harness over skill-class clusters.
        # SLOW (LLM + tmux sandbox). Cold path — once per night, 01:00–04:00.
        self.registry.register(ProactiveTask(
            name="gym_hypothesize",
            handler=handle_gym_hypothesize,
            interval_minutes=1440,  # 24 hours
            gates=GateSystem([
                time_gate(state, "gym_hypothesize", min_hours=23.5),
                active_hours_gate("01:00", "04:00", task_name="gym_hypothesize"),
            ]),
            read_only=False,
            brief=False,
        ))

        # Harvey's Mascot GYM — Layer 4: lope validation gate.
        # Reads pending hypotheses, runs lope validator pool, routes to
        # approved/rejected. Runs after gym_hypothesize each night.
        self.registry.register(ProactiveTask(
            name="gym_lope_gate",
            handler=handle_gym_lope_gate,
            interval_minutes=1440,
            gates=GateSystem([
                time_gate(state, "gym_lope_gate", min_hours=23.5),
                active_hours_gate("03:00", "06:00", task_name="gym_lope_gate"),
            ]),
            read_only=False,
            brief=False,
        ))

        # Harvey's Mascot GYM — Layer 4b: morning Brain journal rollup.
        # Summarizes last night's pipeline into an outliner block. Once
        # per day, early morning, idempotent within the day.
        self.registry.register(ProactiveTask(
            name="gym_morning_report",
            handler=handle_gym_morning_report,
            interval_minutes=1440,
            gates=GateSystem([
                time_gate(state, "gym_morning_report", min_hours=23.5),
                active_hours_gate("06:00", "09:00", task_name="gym_morning_report"),
            ]),
            read_only=False,
            brief=True,
        ))

    def tick(self) -> List[Dict]:
        """Run all eligible tasks and return their results.

        Serialized by `_tick_lock` so dynamic wakes via `sancho.wake`
        cannot race against a tick that's already in progress.
        Coalescing: wakes that arrive during an active tick are not
        dropped — they set `_pending_wake`, and the current tick's
        finisher drains exactly one more pass.

        For each task that runs:
          1. Execute the handler
          2. Log to today's Brain journal
          3. Publish events to EventBus
        """
        with self._tick_lock:
            self._pending_wake = False
            results = self._tick_unlocked()
            if self._pending_wake:
                self._pending_wake = False
                log.info("SANCHO draining pending wake after scheduled tick")
                results = results + self._tick_unlocked()
        return results

    def _tick_unlocked(self) -> List[Dict]:
        """Actual tick body. Assumes `_tick_lock` is held by the caller."""
        eligible = self.registry.eligible_tasks()
        if not eligible:
            log.debug("SANCHO tick: no eligible tasks")
            return []

        tick_start = time.time()
        timestamp = datetime.now().isoformat(timespec="seconds")
        results = []

        for task in eligible:
            log.info("SANCHO running: %s", task.name)
            result = self.registry.run_task(task)
            results.append(result)

            # Publish per-task event
            self._publish_event("sancho.task.completed", {
                "task": task.name,
                "success": result["success"],
                "duration_sec": result["duration_sec"],
            })

            # H4: dynamic_checklist gets a specialized signal event ONLY
            # when the LLM verdict is actionable. HEARTBEAT_OK responses
            # set suppressed=True so chat interfaces stay quiet.
            if task.name == "dynamic_checklist" and result.get("success"):
                payload = result.get("result", {})
                if payload.get("evaluated") and not payload.get("suppressed"):
                    self._publish_event("sancho.heartbeat.signal", {
                        "task": task.name,
                        "verdict": payload.get("verdict", ""),
                        "hash": payload.get("hash", ""),
                    })

        # Log to journal
        self._log_to_journal(timestamp, results)

        # Publish tick event
        self._publish_event("sancho.tick", {
            "tasks_run": len(results),
            "total_duration_sec": round(time.time() - tick_start, 2),
            "timestamp": timestamp,
        })

        return results

    def run_once(self) -> List[Dict]:
        """Convenience alias for tick()."""
        return self.tick()

    def _log_to_journal(self, timestamp: str, results: List[Dict]) -> None:
        """
        Append a brief SANCHO tick summary to today's Brain journal.

        Format:
            - SANCHO tick 2026-04-09T14:30:00
              - [dream] 3 pages updated, 1 pruned (2.4s)
              - [wiki_lint] 5 orphans, 2 missing (0.8s)
        """
        today = datetime.now().strftime("%Y_%m_%d")
        journal_path = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"
        journal_path.parent.mkdir(parents=True, exist_ok=True)

        lines = [f"- [[SANCHO]] tick {timestamp}"]
        for r in results:
            summary = self._brief_summary(r)
            lines.append(f"  - [{r['name']}] {summary} ({r['duration_sec']}s)")

        entry = "\n".join(lines) + "\n"

        with open(journal_path, "a") as f:
            f.write(entry)

    def _brief_summary(self, result: Dict) -> str:
        """Extract a one-line summary from a task result."""
        data = result.get("result", {})
        if not result.get("success"):
            return f"FAILED: {data.get('error', 'unknown')}"

        # Build summary from available keys
        parts = []
        for key, val in data.items():
            if key in ("status", "health", "timestamp"):
                continue
            if isinstance(val, (int, float)):
                parts.append(f"{val} {key.replace('_', ' ')}")
            elif isinstance(val, list) and len(val) <= 5:
                parts.append(f"{len(val)} {key.replace('_', ' ')}")
        return ", ".join(parts) if parts else data.get("status", "done")

    def print_status(self) -> None:
        """Pretty-print SANCHO status for CLI."""
        print(f"\n{'=' * 50}")
        print(f"  SANCHO — Proactive Engine")
        print(f"{'=' * 50}")
        for name, task in self.registry.tasks.items():
            interval_ok = self.registry._interval_ok(task)
            gates_pass = task.gates.all_pass()
            eligible = task.enabled and interval_ok and gates_pass
            icon = "✅" if eligible else "❌"
            last_run = self.registry.state.get(f"{name}_last_run", 0)
            if last_run:
                hours_ago = round((time.time() - last_run) / 3600, 1)
                last_str = f"{hours_ago}h ago"
            else:
                last_str = "never"
            print(f"  {icon} {name:<18} interval={task.interval_minutes}m  last={last_str}")
            gate_status = task.gates.status()
            for g_name, g_info in gate_status.items():
                g_icon = "✅" if g_info["passed"] else "❌"
                print(f"      {g_icon} {g_name}: {g_info.get('description', '')}")
        print(f"{'=' * 50}\n")

    def _publish_event(self, topic: str, data: dict) -> None:
        """Publish an event to the EventBus (best-effort, never blocks)."""
        try:
            from core.events.event_stream import EventBus
            bus = EventBus.instance()
            bus.publish(topic, source="sancho", **data)
        except Exception as e:
            log.debug("EventBus publish failed (non-fatal): %s", e)


# ═══════════════════════════════════════════════════════════════════
#  CLI entry point
# ═══════════════════════════════════════════════════════════════════

if __name__ == "__main__":
    import sys

    logging.basicConfig(level=logging.INFO, format="%(name)s | %(message)s")

    sancho = Sancho()

    if "--status" in sys.argv:
        for name, task in sancho.registry.tasks.items():
            interval_ok = sancho.registry._interval_ok(task)
            gates = task.gates.status()
            print(f"{name}: enabled={task.enabled} interval_ok={interval_ok} gates={gates}")
        sys.exit(0)

    results = sancho.tick()
    if results:
        for r in results:
            status = "OK" if r["success"] else "FAIL"
            print(f"  [{status}] {r['name']} ({r['duration_sec']}s)")
    else:
        print("SANCHO: no eligible tasks this tick")

    try:
        from core.terminal.gimmicks import maybe_gimmick
        maybe_gimmick("sancho")
    except Exception:
        pass
