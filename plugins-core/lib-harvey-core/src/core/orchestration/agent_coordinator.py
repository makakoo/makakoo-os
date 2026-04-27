"""
AgentCoordinator — Registry and lifecycle for Harvey's subagents.

Phase 2 deliverable. Single point of wiring between the subagent framework
(core/subagents/) and the AsyncDAGExecutor + shared infrastructure
(ArtifactStore + PersistentEventBus).

Responsibilities:

  1. **Register subagents.** `register(agent)` stores them by name.
  2. **Wire to the DAG executor.** For each of the agent's ACTIONS, calls
     `executor.register_handler(agent.name, action, agent.handle)`.
  3. **Activate passive listeners.** Agents with `start_monitoring()` or
     `start_listening()` methods (TaskMaster, Olibia) are auto-activated.
  4. **Convenience: register all 6 built-in agents** with one call via
     `register_all_default()`.
  5. **Aggregate status reporting.**

The coordinator is NOT a subprocess supervisor — all agents run in the
same Python process. Phase 3 will add a SubprocessCoordinator that uses
`AgentLifecycle.spawn()` for true isolation. The API here is designed so
that swap is a drop-in.
"""

from __future__ import annotations

import logging
import threading
from typing import TYPE_CHECKING, Dict, List, Optional

from core.orchestration.artifact_store import ArtifactStore, get_default_store
from core.orchestration.persistent_event_bus import (
    PersistentEventBus,
    get_default_bus,
)

if TYPE_CHECKING:
    from core.subagents.subagent import Subagent

log = logging.getLogger("harvey.agent_coordinator")


class AgentCoordinator:
    """Registry + lifecycle manager for in-process subagents."""

    def __init__(
        self,
        executor=None,
        artifact_store: Optional[ArtifactStore] = None,
        event_bus: Optional[PersistentEventBus] = None,
    ):
        """
        Args:
            executor: AsyncDAGExecutor instance (optional — you can also
                      register agents without wiring them to an executor,
                      useful for tests).
            artifact_store: Shared ArtifactStore. Defaults to singleton.
            event_bus: Shared PersistentEventBus. Defaults to singleton.
        """
        self.executor = executor
        self.artifact_store = artifact_store or get_default_store()
        self.event_bus = event_bus or get_default_bus()
        self._agents: Dict[str, Subagent] = {}
        # SPRINT-HARVEY-TICKETING Phase 4: per-agent concurrency limiters.
        # Populated at register() time when agent.__class__.MAX_CONCURRENCY
        # is set. BoundedSemaphore so release() on an un-acquired lock
        # raises instead of silently going negative. None entry means
        # "checked and no limit" (so we don't re-check on every register
        # call of the same name).
        self._concurrency_locks: Dict[str, threading.BoundedSemaphore] = {}

    # ─── Registration ────────────────────────────────────────────

    def register(self, agent: Subagent) -> Subagent:
        """
        Register an agent.

        - Injects shared artifact_store + event_bus if the agent didn't
          already get them (idempotent).
        - Wires each action from agent.actions() into the DAG executor.
        - Activates passive listeners (start_monitoring / start_listening).
        """
        if agent.name in self._agents:
            log.warning(
                f"[coordinator] overwriting existing agent: {agent.name}"
            )
        self._agents[agent.name] = agent

        # Make sure the agent uses our shared plumbing
        if agent.artifact_store is not self.artifact_store:
            agent.artifact_store = self.artifact_store
        if agent.event_bus is not self.event_bus:
            agent.event_bus = self.event_bus

        # Phase 4: wrap agent.handle with a concurrency lock if the agent
        # class declares MAX_CONCURRENCY. BoundedSemaphore is created
        # once per agent name so that re-registering the same name
        # reuses the existing lock (prevents two simultaneous image_gen
        # instances from each getting their own slot). Wrapping happens
        # BEFORE `register_handler` so the executor picks up the locked
        # version.
        max_concurrency = getattr(agent.__class__, "MAX_CONCURRENCY", None)
        if (
            isinstance(max_concurrency, int)
            and not isinstance(max_concurrency, bool)
            and max_concurrency > 0
        ):
            sem = self._concurrency_locks.get(agent.name)
            if sem is None:
                sem = threading.BoundedSemaphore(max_concurrency)
                self._concurrency_locks[agent.name] = sem
                log.info(
                    f"[coordinator] {agent.name}: wrapped handle() with "
                    f"BoundedSemaphore(max_concurrency={max_concurrency})"
                )
            original_handle = agent.handle

            def locked_handle(step, ctx, _orig=original_handle, _sem=sem):
                with _sem:
                    return _orig(step, ctx)

            agent.handle = locked_handle  # instance-level shadow

        # Wire actions as DAG step handlers
        if self.executor is not None:
            for action in agent.actions():
                self.executor.register_handler(
                    agent.name, action, agent.handle
                )
                log.debug(
                    f"[coordinator] wired {agent.name}/{action} → executor"
                )

        # Activate passive listeners (TaskMaster.start_monitoring, Olibia.start_listening)
        for method_name in ("start_monitoring", "start_listening"):
            method = getattr(agent, method_name, None)
            if callable(method):
                try:
                    method()
                except Exception as e:
                    log.warning(
                        f"[coordinator] {agent.name}.{method_name}() failed: {e}"
                    )

        log.info(
            f"[coordinator] registered {agent.name} "
            f"with {len(agent.actions())} action(s)"
        )
        return agent

    def register_all_default(self) -> Dict[str, Subagent]:
        """
        Convenience: register all Phase 2 built-in subagents with default
        config, plus the optional PiSubagent when PI_AGENT_ENABLED is set.
        Returns a dict of {name: agent}.

        This is what HarveyChat gateway calls at startup once we flip the
        switch from single-agent to swarm mode.
        """
        from core.subagents.image_gen_agent import ImageGenAgent
        from core.subagents.researcher_agent import ResearcherAgent
        from core.subagents.synthesizer_agent import SynthesizerAgent
        from core.subagents.storage_agent import StorageAgent
        from core.subagents.task_master_agent import TaskMasterAgent
        from core.subagents.olibia_agent import OlibiaAgent

        built_ins = [
            ImageGenAgent,
            ResearcherAgent,
            SynthesizerAgent,
            StorageAgent,
            TaskMasterAgent,
            OlibiaAgent,
        ]

        for cls in built_ins:
            try:
                self.register(
                    cls(
                        artifact_store=self.artifact_store,
                        event_bus=self.event_bus,
                    )
                )
            except Exception as e:
                log.error(
                    f"[coordinator] failed to register {cls.__name__}: {e}"
                )

        # Optional: PiSubagent. Registered only when `PI_AGENT_ENABLED=1`
        # AND the `pi` binary is on PATH. Failing silently on missing pi
        # matches the "no surprise errors on fresh installs" pattern.
        try:
            from core.subagents.pi_agent import PiSubagent

            pi_agent = PiSubagent(
                artifact_store=self.artifact_store,
                event_bus=self.event_bus,
            )
            if pi_agent.available():
                self.register(pi_agent)
            else:
                log.debug(
                    "[coordinator] PiSubagent not registered "
                    "(PI_AGENT_ENABLED unset or pi binary missing)"
                )
        except Exception as e:
            log.warning(f"[coordinator] PiSubagent probe failed: {e}")

        return dict(self._agents)

    def unregister(self, name: str) -> bool:
        """Remove an agent by name. Returns True if removed."""
        return self._agents.pop(name, None) is not None

    # ─── Accessors ───────────────────────────────────────────────

    def get(self, name: str) -> Optional[Subagent]:
        return self._agents.get(name)

    def list_agents(self) -> List[str]:
        return list(self._agents.keys())

    def all_agents(self) -> Dict[str, Subagent]:
        return dict(self._agents)

    def actions_map(self) -> Dict[str, List[str]]:
        """Return {agent_name: [actions]} for all registered agents."""
        return {name: ag.actions() for name, ag in self._agents.items()}

    def status(self) -> Dict:
        """Aggregate snapshot for health checks / diagnostics."""
        task_master = self._agents.get("task_master")
        progress = task_master.snapshot() if task_master else {}

        olibia = self._agents.get("olibia")
        commentary_count = olibia.commentary_count() if olibia else 0

        return {
            "agent_count": len(self._agents),
            "agents": sorted(self._agents.keys()),
            "actions_map": self.actions_map(),
            "artifact_count": self.artifact_store.count(),
            "event_count": self.event_bus.count(),
            "latest_seq": self.event_bus.latest_seq(),
            "progress": progress,
            "olibia_commentary": commentary_count,
        }


__all__ = ["AgentCoordinator"]
