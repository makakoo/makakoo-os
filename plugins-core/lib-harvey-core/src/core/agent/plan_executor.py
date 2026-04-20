"""
PlanExecutor — walks a Plan produced by the Planner.

Takes a Plan + a spawn_fn (normally tool_spawn_subagent) and executes
each step in dependency order. Independent steps (same layer in the DAG)
run sequentially in this version — parallelism is a future enhancement
that would use asyncio.gather or a ThreadPoolExecutor.

Each step's result is passed into dependent steps as a `prior_results`
block so the subagent has context from its dependencies.

Contract:
  - spawn_fn(agent_name, task, action) -> str   (the string result)
  - Never raises — captures per-step failures as step results with an
    "error:" prefix so the caller can decide whether to continue or
    abort on first failure (configurable via fail_fast=True).
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Tuple

from .planner import Plan, PlanStep

log = logging.getLogger("harvey.plan_executor")


@dataclass
class StepResult:
    index: int
    role: str
    task: str
    output: str
    ok: bool
    error: str = ""


@dataclass
class PlanExecutionReport:
    plan: Plan
    results: List[StepResult] = field(default_factory=list)
    ok: bool = True
    error: str = ""

    def step_result(self, index: int) -> Optional[StepResult]:
        for r in self.results:
            if r.index == index:
                return r
        return None

    def successful_results(self) -> List[StepResult]:
        return [r for r in self.results if r.ok]

    def failed_results(self) -> List[StepResult]:
        return [r for r in self.results if not r.ok]


SpawnFn = Callable[..., str]
"""Contract: spawn_fn(agent_name, task, action='') -> result string.

This matches tool_spawn_subagent in harvey_agent.py. Tests inject a stub
that records invocations + returns canned strings.
"""


class PlanExecutor:
    """Walks a Plan in dependency order, invoking spawn_fn per step."""

    def __init__(self, spawn_fn: SpawnFn, fail_fast: bool = True):
        self.spawn_fn = spawn_fn
        self.fail_fast = fail_fast

    def execute(self, plan: Plan) -> PlanExecutionReport:
        """Run the plan. Returns a full execution report."""
        report = PlanExecutionReport(plan=plan)

        try:
            order = self._topological_order(plan)
        except ValueError as e:
            report.ok = False
            report.error = str(e)
            return report

        log.info(
            f"[executor] running plan: {len(plan.steps)} step(s), "
            f"order={[s.index for s in order]}, fail_fast={self.fail_fast}"
        )

        for step in order:
            dependency_outputs = self._collect_dependency_outputs(step, report)
            augmented_task = self._build_task_with_context(step, dependency_outputs)

            log.info(
                f"[executor] step {step.index} → {step.role}/{step.action or 'default'} "
                f"task={step.task[:80]!r}"
            )

            try:
                raw_output = self.spawn_fn(
                    agent_name=step.role,
                    task=augmented_task,
                    action=step.action,
                )
            except Exception as e:
                raw_output = f"spawn_fn error: {type(e).__name__}: {e}"

            is_error = self._looks_like_error(raw_output)
            result = StepResult(
                index=step.index,
                role=step.role,
                task=step.task,
                output=raw_output,
                ok=not is_error,
                error=raw_output if is_error else "",
            )
            report.results.append(result)

            if is_error and self.fail_fast:
                report.ok = False
                report.error = f"step {step.index} ({step.role}) failed: {raw_output[:200]}"
                log.warning(f"[executor] fail_fast: aborting after step {step.index}")
                return report

        # Overall ok if no step failed
        report.ok = all(r.ok for r in report.results)
        if not report.ok and not report.error:
            failed = report.failed_results()
            report.error = (
                f"{len(failed)} step(s) failed: "
                f"{[r.index for r in failed]}"
            )
        return report

    # ─── Ordering ───────────────────────────────────────────────

    def _topological_order(self, plan: Plan) -> List[PlanStep]:
        """Return a topological order (roots first, leaves last).

        The planner already enforces forward-only dependencies (a step
        can only depend on earlier indices), so we can walk in index order.
        This method exists as a defensive check + for parallelism hints.
        """
        by_index = {s.index: s for s in plan.steps}
        ordered = sorted(plan.steps, key=lambda s: s.index)
        # Validate: all dependency indices must exist
        for s in ordered:
            for dep in s.depends_on:
                if dep not in by_index:
                    raise ValueError(
                        f"step {s.index} depends on missing index {dep}"
                    )
                if dep >= s.index:
                    raise ValueError(
                        f"step {s.index} has forward dependency {dep}"
                    )
        return ordered

    # ─── Context propagation ────────────────────────────────────

    def _collect_dependency_outputs(
        self, step: PlanStep, report: PlanExecutionReport
    ) -> Dict[int, str]:
        """Look up the outputs of this step's declared dependencies."""
        outputs: Dict[int, str] = {}
        for dep in step.depends_on:
            dep_result = report.step_result(dep)
            if dep_result is not None and dep_result.ok:
                outputs[dep] = dep_result.output
        return outputs

    def _build_task_with_context(
        self, step: PlanStep, dep_outputs: Dict[int, str]
    ) -> str:
        """Inject prior-step outputs into the task text before passing to spawn."""
        if not dep_outputs:
            return step.task

        lines = [step.task, "", "--- context from prior steps ---"]
        for dep_idx in sorted(dep_outputs.keys()):
            truncated = dep_outputs[dep_idx][:2000]
            lines.append(f"[step {dep_idx}]: {truncated}")
        return "\n".join(lines)

    # ─── Error detection ────────────────────────────────────────

    @staticmethod
    def _looks_like_error(output: str) -> bool:
        """Heuristic: does the spawn_fn output look like an error string?

        tool_spawn_subagent's error contract is: any failure starts with
        'Tool spawn_subagent error:' or 'Tool spawn_subagent timeout:'
        or is prefixed with [ERROR] from the summarization logic.
        """
        if not output:
            return True
        prefix = output.lstrip()[:60].lower()
        return (
            prefix.startswith("tool spawn_subagent error")
            or prefix.startswith("tool spawn_subagent timeout")
            or prefix.startswith("[error]")
            or prefix.startswith("spawn_fn error")
        )
