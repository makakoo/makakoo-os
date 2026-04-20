"""
OlibiaAgent — Mascot-as-agent.

Phase 2 deliverable. Olibia is Harvey's guardian owl (see core/agent/mascot.py).
This wraps her as a first-class subagent with two modes:

  1. **Passive listener:** `start_listening()` subscribes to workflow.* and
     agent.*.failed events, wraps them in owl voice, and publishes to
     `olibia.commentary` on the event bus. This is how Olibia provides
     personality throughout every workflow without the workflow author
     having to explicitly call her.

  2. **Active step handler:** when a workflow step declares `agent="olibia",
     action="announce|celebrate|warn"`, Olibia runs as a normal DAG step
     and publishes a scripted message.

`start_listening()` is called automatically by AgentCoordinator on register.
"""

from __future__ import annotations

import logging
from typing import Any, Dict, Optional

from core.subagents.subagent import Subagent

log = logging.getLogger("harvey.subagent.olibia")


class OlibiaAgent(Subagent):
    NAME = "olibia"
    ACTIONS = ["announce", "celebrate", "warn", "greet"]
    DESCRIPTION = "Harvey's guardian owl mascot — personality + encouragement."

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        # Lazy import so tests that don't need mascot can skip it
        from core.agent.mascot import Olibia as _Olibia

        self.mascot = _Olibia
        self._subscribed = False
        self._commentary_count = 0

    # ─── Passive listener ────────────────────────────────────────

    def start_listening(self) -> None:
        if self._subscribed:
            return
        self.event_bus.subscribe("workflow.started", self._on_workflow_started)
        self.event_bus.subscribe("workflow.completed", self._on_workflow_completed)
        self.event_bus.subscribe("workflow.failed", self._on_workflow_failed)
        self.event_bus.subscribe("workflow.step.failed", self._on_step_failed)
        self._subscribed = True
        log.info("[olibia] listening to workflow.* events")

    def _on_workflow_started(self, event) -> None:
        name = (event.data or {}).get("workflow_name", "workflow")
        self._publish_commentary(self.mascot.progress(f"starting {name}"))

    def _on_workflow_completed(self, event) -> None:
        name = (event.data or {}).get("workflow_name", "workflow")
        self._publish_commentary(self.mascot.milestone(f"{name} done"))

    def _on_workflow_failed(self, event) -> None:
        reason = (event.data or {}).get("pause_reason", "something broke")
        self._publish_commentary(self.mascot.error(reason))

    def _on_step_failed(self, event) -> None:
        err = (event.data or {}).get("error", "step failed")
        self._publish_commentary(self.mascot.warning(err))

    def _publish_commentary(self, message: str) -> None:
        self._commentary_count += 1
        try:
            self.event_bus.publish(
                "olibia.commentary",
                source=self.name,
                message=message,
                count=self._commentary_count,
            )
        except Exception as e:
            log.debug(f"[olibia] commentary publish failed: {e}")

    def commentary_count(self) -> int:
        return self._commentary_count

    # ─── Active DAG step handler ─────────────────────────────────

    def execute(self, step, ctx: Dict) -> Dict[str, Any]:
        message = (
            ctx.get("message")
            or ctx.get("summary")
            or ctx.get("detail")
            or ""
        )
        action = step.action

        if action == "greet":
            wrapped = self.mascot.greeting()
        elif action == "announce":
            wrapped = self.mascot.progress(message or "working")
        elif action == "celebrate":
            wrapped = self.mascot.milestone(message or "milestone reached")
        elif action == "warn":
            wrapped = self.mascot.warning(message or "something to watch")
        else:
            wrapped = message or ""

        self._publish_commentary(wrapped)
        return {
            "ok": True,
            "action": action,
            "message": wrapped,
            "commentary_count": self._commentary_count,
        }


__all__ = ["OlibiaAgent"]
