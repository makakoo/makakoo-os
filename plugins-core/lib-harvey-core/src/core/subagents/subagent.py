"""
Subagent — Base class for Harvey's in-process specialized agents (Phase 2).

Every subagent is a step handler for the AsyncDAGExecutor. The executor
calls `agent.handle(step, ctx)`, which wraps `execute()` with:

  - Event emission (agent.{name}.started / completed / failed)
  - Exception → event + re-raise (so DAG executor sees the failure)

Subagents coordinate via the ArtifactStore + PersistentEventBus built in
Phase 1.5. They don't import Harvey tools directly — tools are injected
via a `tools: Dict[str, Callable]` so unit tests can mock them.
"""

from __future__ import annotations

import logging
from typing import Any, Callable, Dict, List, Optional

from core.orchestration.artifact_store import ArtifactStore, get_default_store
from core.orchestration.persistent_event_bus import (
    PersistentEventBus,
    get_default_bus,
)
from core.security.access_control import (
    AccessDenied,
    get_default_access_control,
)
from core.security.audit_log import get_default_audit_log
from core.observability.structured_logger import log_context

log = logging.getLogger("harvey.subagent")


# ─────────────────────────────────────────────────────────────────────
# Lazy default tools proxy
# ─────────────────────────────────────────────────────────────────────
#
# We don't want to import core.agent.harvey_agent at module load time
# because that module pulls in requests, tool registries, etc. Instead,
# DEFAULT_TOOLS is a lazy proxy: it imports TOOL_DISPATCH on first use.


class _LazyToolDict:
    """A Mapping-like proxy that resolves TOOL_DISPATCH on first access."""

    def __init__(self):
        self._cache: Optional[Dict[str, Callable]] = None

    def _resolve(self) -> Dict[str, Callable]:
        if self._cache is None:
            try:
                from core.agent.harvey_agent import TOOL_DISPATCH
                self._cache = dict(TOOL_DISPATCH)
            except Exception as e:
                log.warning(f"[subagent] default tool dispatch unavailable: {e}")
                self._cache = {}
        return self._cache

    def __getitem__(self, key: str) -> Callable:
        return self._resolve()[key]

    def get(self, key: str, default: Any = None) -> Any:
        return self._resolve().get(key, default)

    def __contains__(self, key: str) -> bool:
        return key in self._resolve()

    def __iter__(self):
        return iter(self._resolve())

    def __len__(self) -> int:
        return len(self._resolve())

    def keys(self):
        return self._resolve().keys()


DEFAULT_TOOLS = _LazyToolDict()


# ─────────────────────────────────────────────────────────────────────
# Subagent base class
# ─────────────────────────────────────────────────────────────────────


class Subagent:
    """
    Base class for in-process subagents.

    Override `NAME`, `ACTIONS`, and `execute()` in your subclass. The base
    class handles event emission, artifact read/write, and tool dispatch.
    """

    # Class-level metadata (override in subclass)
    NAME: str = "subagent"
    ACTIONS: List[str] = []
    DESCRIPTION: str = ""

    def __init__(
        self,
        name: Optional[str] = None,
        artifact_store: Optional[ArtifactStore] = None,
        event_bus: Optional[PersistentEventBus] = None,
        tools: Optional[Dict[str, Callable]] = None,
    ):
        self.name = name or self.NAME
        self.artifact_store = artifact_store or get_default_store()
        self.event_bus = event_bus or get_default_bus()
        self.tools = tools if tools is not None else DEFAULT_TOOLS

    # ─── Step handler entry point ────────────────────────────────

    def handle(self, step, ctx: Dict) -> Dict:
        """
        Called by AsyncDAGExecutor via register_handler. Wraps execute()
        with event emission, exception handling, and structured logging
        context (workflow_id / step_id / agent_id bound for the duration
        of the call so every log line inside execute() is queryable).
        """
        step_id = getattr(step, "id", "?")
        action = getattr(step, "action", "?")
        # Try to pull workflow_id from ctx — the DAG executor injects it
        # into ctx when resolving reads_artifacts against {wf_id}:{step_id}
        workflow_id = ctx.get("workflow_id", "") if isinstance(ctx, dict) else ""

        with log_context(
            workflow_id=workflow_id or None,
            step_id=step_id,
            agent_id=self.name,
        ):
            self.emit("started", step_id=step_id, action=action)
            try:
                result = self.execute(step, ctx)
            except Exception as e:
                log.exception(f"[{self.name}] execute raised")
                self.emit(
                    "failed",
                    step_id=step_id,
                    error=f"{type(e).__name__}: {e}",
                )
                raise

            result = result or {}
            if not isinstance(result, dict):
                result = {"result": result}

            self.emit(
                "completed",
                step_id=step_id,
                output_keys=list(result.keys()),
            )
            return result

    # ─── Override in subclass ────────────────────────────────────

    def execute(self, step, ctx: Dict) -> Dict:
        """Do the agent's work. Return a dict (becomes step output)."""
        raise NotImplementedError(
            f"Subagent {self.name} must implement execute()"
        )

    # ─── Metadata ────────────────────────────────────────────────

    def actions(self) -> List[str]:
        """Actions this subagent handles. Defaults to class ACTIONS attr."""
        return list(self.ACTIONS) or ["default"]

    # ─── Helpers ─────────────────────────────────────────────────

    def tool(self, name: str, args: Optional[Dict] = None) -> Any:
        """
        Invoke a Harvey tool by name. Returns the tool's result.

        Integration points:
          - Consults the default AgentAccessControl before the call.
            Permissive by default (no policy → pass-through), but once
            a policy is registered for this agent, the tool name must
            be on the allowlist, not on the denylist, and must pass
            the rate limit check — otherwise raises AccessDenied.
          - Records every call to the default AuditLog (opt-in; if no
            audit log is configured, this is a silent no-op).
        """
        fn = self.tools.get(name)
        if fn is None:
            raise KeyError(
                f"Tool not available to {self.name}: {name} "
                f"(known: {list(self.tools.keys())[:5]}...)"
            )

        # Access control (opt-in — permissive if no policy registered)
        ac = get_default_access_control()
        try:
            ac.check(self.name, name)
        except AccessDenied:
            audit = get_default_audit_log()
            if audit is not None:
                try:
                    audit.record_denial(
                        agent=self.name, tool=name,
                        reason="access_control_denied",
                    )
                except Exception:
                    pass
            raise

        # Call the tool
        try:
            result = fn(args or {})
        except Exception as e:
            audit = get_default_audit_log()
            if audit is not None:
                try:
                    audit.record_tool_call(
                        agent=self.name, tool=name,
                        outcome="error",
                        error=f"{type(e).__name__}: {e}",
                    )
                except Exception:
                    pass
            raise

        # Audit the successful call
        audit = get_default_audit_log()
        if audit is not None:
            try:
                audit.record_tool_call(
                    agent=self.name, tool=name, outcome="ok",
                )
            except Exception:
                pass

        return result

    def publish_artifact(
        self,
        name: str,
        payload: Any,
        depends_on: Optional[List[str]] = None,
    ) -> str:
        """Publish an ad-hoc artifact outside the DAG auto-publishing path."""
        return self.artifact_store.publish(
            name=name,
            payload=payload,
            producer=self.name,
            depends_on=depends_on or [],
        )

    def get_artifact(self, name: str) -> Any:
        """Fetch the payload of an artifact by name."""
        art = self.artifact_store.get(name)
        return art.payload if art else None

    def wait_for_artifact(
        self, name: str, timeout: float = 30.0
    ) -> Any:
        """Block until an artifact appears, then return its payload."""
        art = self.artifact_store.wait_for(name, timeout=timeout)
        return art.payload if art else None

    def emit(self, topic_suffix: str, **data) -> int:
        """Publish an event with the agent-scoped topic `agent.{name}.{suffix}`."""
        try:
            return self.event_bus.publish(
                f"agent.{self.name}.{topic_suffix}",
                source=self.name,
                **data,
            )
        except Exception as e:
            log.debug(f"[{self.name}] emit failed: {e}")
            return 0

    def __repr__(self) -> str:
        return f"<{self.__class__.__name__} name={self.name!r} actions={self.actions()}>"


__all__ = ["Subagent", "DEFAULT_TOOLS"]
