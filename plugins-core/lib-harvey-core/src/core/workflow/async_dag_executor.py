"""
AsyncDAGExecutor — asyncio-based DAG workflow runner.

Phase 1.5 deliverable. Replaces the sequential `WorkflowExecutor.execute_cycle`
with a true DAG executor: every tick, all steps whose dependencies are
satisfied run **concurrently** via asyncio.gather. Step results publish
to the ArtifactStore so any later step (in this workflow or another) can
read them by name. Events flow to the PersistentEventBus.

This is where the "worktree principle" gets real:

  - 3 independent research steps run in parallel (not sequentially)
  - Step 4 reads step 1's output by artifact name
  - Checkpoints survive restart
  - Events persist for cross-process observability

Wraps the existing WorkflowEngine — does NOT replace it. Old callsites
that use `execute_next_step` keep working. New code opts into the async
DAG executor.
"""

from __future__ import annotations

import asyncio
import logging
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from typing import Any, Callable, Dict, List, Optional, Tuple

from core.workflow.engine import (
    StepState,
    Workflow,
    WorkflowEngine,
    WorkflowState,
    WorkflowStep,
)
from core.orchestration.artifact_store import ArtifactStore, get_default_store
from core.orchestration.persistent_event_bus import (
    PersistentEventBus,
    get_default_bus,
)

log = logging.getLogger("harvey.workflow.async_dag")


# ─────────────────────────────────────────────────────────────────────
# Step result container
# ─────────────────────────────────────────────────────────────────────


@dataclass
class StepResult:
    step_id: str
    ok: bool
    output: Dict[str, Any]
    error: str = ""
    duration_s: float = 0.0
    artifact_id: Optional[str] = None


# ─────────────────────────────────────────────────────────────────────
# AsyncDAGExecutor
# ─────────────────────────────────────────────────────────────────────


class AsyncDAGExecutor:
    """
    Run a Workflow as a DAG: parallelize all ready steps.

    Usage:

        engine = WorkflowEngine()
        executor = AsyncDAGExecutor(engine)
        executor.register_handler("researcher", "search", handler_fn)
        wf = engine.create_workflow("research", steps=[...])
        result = await executor.run_workflow(wf)
    """

    def __init__(
        self,
        engine: Optional[WorkflowEngine] = None,
        artifact_store: Optional[ArtifactStore] = None,
        event_bus: Optional[PersistentEventBus] = None,
        max_concurrent_steps: int = 8,
        failure_recovery: Optional[Any] = None,
    ):
        """
        Args:
          failure_recovery: Optional FailureRecovery instance. When set,
            `_run_step()` consults `should_dispatch(agent_name)` before
            dispatching the step. If the circuit breaker for that agent
            is OPEN, the step is marked FAILED with a breaker-blocked
            error — it never reaches the handler. Breaker state still
            updates automatically via the event bus subscription.
        """
        self.engine = engine or WorkflowEngine()
        self.artifact_store = artifact_store or get_default_store()
        self.event_bus = event_bus or get_default_bus()
        self.max_concurrent_steps = max_concurrent_steps
        self.failure_recovery = failure_recovery

        # Thread pool for sync step handlers (most handlers are sync because
        # they wrap existing Harvey tools)
        self._pool = ThreadPoolExecutor(
            max_workers=max_concurrent_steps,
            thread_name_prefix="harvey-dag",
        )

    # ─── Handler registration (delegates to engine) ──────────────

    def register_handler(
        self, agent: str, action: str, handler: Callable
    ) -> None:
        self.engine.register_handler(agent, action, handler)

    # ─── Public API ──────────────────────────────────────────────

    async def run_workflow(self, wf: Workflow) -> Workflow:
        """
        Run a workflow to completion (or until paused/failed/deadlocked).

        Each tick:
          1. Find all READY steps (deps satisfied, not complete, not running)
          2. Run them concurrently via asyncio.gather
          3. Apply results to workflow state
          4. Publish artifacts, emit events, checkpoint
          5. Loop until no more ready steps
        """
        if wf.state == WorkflowState.DRAFT:
            wf.state = WorkflowState.RUNNING
            wf.started_at = time.time()

        self._emit("workflow.started", wf)

        try:
            while True:
                ready = self._find_ready_steps(wf)
                if not ready:
                    break

                # Cap concurrent steps per tick
                batch = ready[: self.max_concurrent_steps]

                log.info(
                    f"[dag] workflow {wf.id} running {len(batch)} step(s) "
                    f"concurrently: {[s.id for s in batch]}"
                )

                # Mark all as RUNNING before we start (so a concurrent
                # find_ready call wouldn't re-pick them)
                for step in batch:
                    step.state = StepState.RUNNING
                    step.started_at = time.time()
                self.engine.save_workflow(wf)

                # Spawn each step as an asyncio task
                tasks = [
                    asyncio.create_task(self._run_step(step, wf))
                    for step in batch
                ]
                results: List[StepResult] = await asyncio.gather(*tasks)

                # Apply results
                for result in results:
                    self._apply_result(wf, result)

                self.engine.save_workflow(wf)

                # If any step failed and strategy is pause, stop the loop
                if wf.state in (WorkflowState.FAILED, WorkflowState.PAUSED):
                    log.info(
                        f"[dag] workflow {wf.id} stopped: {wf.state.value}"
                    )
                    break

            # Determine final state
            self._finalize_workflow(wf)

        except Exception as e:
            log.exception(f"[dag] workflow {wf.id} crashed")
            wf.state = WorkflowState.FAILED
            wf.pause_reason = f"Executor crash: {e}"
            self.engine.save_workflow(wf)
            self._emit("workflow.crashed", wf, error=str(e))

        self._emit("workflow." + wf.state.value, wf)
        return wf

    async def run_by_id(self, wf_id: str) -> Optional[Workflow]:
        wf = self.engine.get_workflow(wf_id)
        if wf is None:
            return None
        return await self.run_workflow(wf)

    async def run_ready_workflows(self) -> List[str]:
        """Run every QUEUED or RUNNING workflow currently in the DB."""
        cursor = self.engine.db.execute(
            "SELECT id FROM workflows WHERE state IN (?, ?)",
            (WorkflowState.QUEUED.value, WorkflowState.RUNNING.value),
        )
        wf_ids = [row[0] for row in cursor.fetchall()]

        executed: List[str] = []
        for wf_id in wf_ids:
            wf = self.engine.get_workflow(wf_id)
            if wf is None:
                continue
            await self.run_workflow(wf)
            executed.append(wf_id)
        return executed

    def close(self) -> None:
        self._pool.shutdown(wait=True)

    # ─── Core step execution ─────────────────────────────────────

    def _find_ready_steps(self, wf: Workflow) -> List[WorkflowStep]:
        """
        All steps whose (a) state is PENDING, (b) intra-workflow deps are
        complete, and (c) cross-workflow artifact deps are available.
        """
        ready: List[WorkflowStep] = []
        for step in wf.steps:
            if step.state != StepState.PENDING:
                continue
            # Intra-workflow dep check
            if not wf.can_start_step(step):
                continue
            # Cross-workflow artifact dep check
            reads = (step.input_context or {}).get("reads_artifacts") or []
            if reads and not all(self.artifact_store.exists(n) for n in reads):
                log.debug(
                    f"[dag] step {step.id} blocked on artifacts: {reads}"
                )
                continue
            ready.append(step)
        return ready

    async def _run_step(
        self, step: WorkflowStep, wf: Workflow
    ) -> StepResult:
        """Run a single step handler. Sync handlers are offloaded to the pool."""
        start = time.time()

        # Build input context: workflow-wide context + resolved artifacts
        input_ctx = dict(wf.context)
        reads = (step.input_context or {}).get("reads_artifacts") or []
        if reads:
            resolved: Dict[str, Any] = {}
            for name in reads:
                art = self.artifact_store.get(name)
                if art is not None:
                    resolved[name] = art.payload
            input_ctx["resolved_artifacts"] = resolved
        # Preserve original step.input_context entries too
        input_ctx.update(step.input_context or {})
        # Inject workflow_id for structured logging context enrichment
        input_ctx["workflow_id"] = wf.id
        step.input_context = input_ctx

        handler = self.engine._handlers.get((step.agent, step.action))
        if handler is None:
            return StepResult(
                step_id=step.id,
                ok=False,
                output={},
                error=f"No handler for {step.agent}/{step.action}",
                duration_s=time.time() - start,
            )

        # Circuit breaker gate: if failure_recovery is wired and this
        # agent's breaker is OPEN, skip dispatch entirely.
        if self.failure_recovery is not None:
            try:
                if not self.failure_recovery.should_dispatch(step.agent):
                    log.warning(
                        f"[dag] step {step.id} blocked — "
                        f"{step.agent} breaker OPEN"
                    )
                    self._emit(
                        "workflow.step.blocked", wf,
                        step_id=step.id, agent=step.agent,
                        reason="circuit_breaker_open",
                    )
                    return StepResult(
                        step_id=step.id,
                        ok=False,
                        output={},
                        error=f"circuit breaker open for {step.agent}",
                        duration_s=time.time() - start,
                    )
            except Exception as e:
                # Never let the breaker check itself crash the step
                log.warning(f"[dag] failure_recovery check raised: {e}")

        self._emit(
            "workflow.step.started", wf, step_id=step.id, agent=step.agent
        )

        loop = asyncio.get_event_loop()
        try:
            if asyncio.iscoroutinefunction(handler):
                output = await handler(step, input_ctx)
            else:
                output = await loop.run_in_executor(
                    self._pool, handler, step, input_ctx
                )
        except Exception as e:
            log.exception(f"[dag] step {step.id} handler raised")
            return StepResult(
                step_id=step.id,
                ok=False,
                output={},
                error=f"{type(e).__name__}: {e}",
                duration_s=time.time() - start,
            )

        output = output or {}
        if not isinstance(output, dict):
            output = {"result": output}

        # Publish step output as an artifact
        artifact_name = f"{wf.id}:{step.id}"
        producer = f"workflow:{wf.id}"
        try:
            artifact_id = self.artifact_store.publish(
                name=artifact_name,
                payload=output,
                producer=producer,
                depends_on=[f"{wf.id}:{d}" for d in step.depends_on],
            )
        except Exception as e:
            log.warning(f"[dag] artifact publish failed for {artifact_name}: {e}")
            artifact_id = None

        return StepResult(
            step_id=step.id,
            ok=True,
            output=output,
            duration_s=time.time() - start,
            artifact_id=artifact_id,
        )

    def _apply_result(self, wf: Workflow, result: StepResult) -> None:
        step = wf.get_step(result.step_id)
        if step is None:
            return

        if result.ok:
            wf.mark_step_complete(step, result.output)
            log.info(
                f"[dag] step {step.id} ok in {result.duration_s:.2f}s "
                f"(artifact={result.artifact_id})"
            )
            self._emit(
                "workflow.step.completed",
                wf,
                step_id=step.id,
                duration_s=result.duration_s,
                artifact_id=result.artifact_id,
            )
        else:
            wf.fail_step(step, result.error)
            log.warning(f"[dag] step {step.id} FAILED: {result.error}")
            self._emit(
                "workflow.step.failed",
                wf,
                step_id=step.id,
                error=result.error,
            )
            if wf.failure_strategy == "pause":
                wf.state = WorkflowState.PAUSED
                wf.pause_reason = f"Step {step.id} failed: {result.error}"
            elif wf.failure_strategy == "skip":
                step.state = StepState.SKIPPED

    def _finalize_workflow(self, wf: Workflow) -> None:
        """Set COMPLETED/FAILED state based on step states."""
        if wf.state in (WorkflowState.FAILED, WorkflowState.PAUSED):
            return  # already set

        all_terminal = all(
            s.state
            in (StepState.CHECKPOINTED, StepState.SKIPPED, StepState.FAILED)
            for s in wf.steps
        )
        any_failed = any(s.state == StepState.FAILED for s in wf.steps)
        any_pending = any(s.state == StepState.PENDING for s in wf.steps)

        if all_terminal and not any_failed:
            wf.state = WorkflowState.COMPLETED
            wf.completed_at = time.time()
        elif any_failed:
            wf.state = WorkflowState.FAILED
        elif any_pending:
            # Deadlock: pending steps that can never run
            wf.state = WorkflowState.PAUSED
            wf.pause_reason = "Deadlock: pending steps have unmet dependencies"

        self.engine.save_workflow(wf)

    def _emit(self, topic: str, wf: Workflow, **extra) -> None:
        try:
            self.event_bus.publish(
                topic,
                source=f"dag_executor",
                workflow_id=wf.id,
                workflow_name=wf.name,
                state=wf.state.value,
                **extra,
            )
        except Exception as e:
            log.debug(f"[dag] event emit failed: {e}")


__all__ = ["AsyncDAGExecutor", "StepResult"]
