"""
Coordinator — Main orchestrator for the Harvey OS multi-agent swarm.

Runs a 4-phase pipeline:
  1. Research  — 2 parallel researcher workers gather intel
  2. Synthesis — AntiLazyDelegator digests research, crafts impl spec
  3. Implementation — 1 worker executes the spec
  4. Verification — 1 worker validates the output

State persists to data/coordinator_tasks.json.
Events publish to the global EventBus.
"""

import json
import logging
import os
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional
from uuid import uuid4

from core.coordinator.anti_lazy import AntiLazyDelegator
from core.coordinator.scratchpad import Scratchpad
from core.coordinator.worker import Worker, WorkerRole
from core.events.event_stream import EventBus

log = logging.getLogger("harvey.coordinator")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
STATE_FILE = Path(HARVEY_HOME) / "data" / "coordinator_tasks.json"


@dataclass
class SwarmTask:
    """Tracks a single coordinator pipeline execution."""
    task_id: str = field(default_factory=lambda: uuid4().hex[:8])
    objective: str = ""
    status: str = "pending"  # pending|researching|synthesizing|implementing|verifying|complete|failed
    workers: Dict[str, Any] = field(default_factory=dict)
    scratchpad_dir: str = ""
    created_at: str = field(default_factory=lambda: datetime.now().isoformat())
    completed_at: str = ""

    def to_dict(self) -> dict:
        return asdict(self)


class Coordinator:
    """
    Orchestrates a 4-phase worker swarm for complex tasks.

    Each phase feeds structured input to the next via the AntiLazyDelegator,
    preventing raw-output pass-through between agents.
    """

    def __init__(self, model: str = ""):
        self.model = model
        self.bus = EventBus.instance()
        self._tasks: Dict[str, SwarmTask] = {}
        self._load_state()

    # ── State persistence ─────────────────────────────────────

    def _load_state(self) -> None:
        """Load task history from disk."""
        if STATE_FILE.exists():
            try:
                data = json.loads(STATE_FILE.read_text(encoding="utf-8"))
                for entry in data:
                    task = SwarmTask(**entry)
                    self._tasks[task.task_id] = task
            except (json.JSONDecodeError, TypeError) as e:
                log.warning("Failed to load coordinator state: %s", e)

    def _save_state(self) -> None:
        """Persist task state to disk."""
        STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
        data = [t.to_dict() for t in self._tasks.values()]
        STATE_FILE.write_text(json.dumps(data, indent=2), encoding="utf-8")

    def _emit(self, topic: str, task: SwarmTask, **kwargs) -> None:
        """Publish an event to the bus."""
        self.bus.publish(
            f"coordinator.{topic}",
            source="coordinator",
            task_id=task.task_id,
            objective=task.objective,
            status=task.status,
            **kwargs,
        )

    # ── Pipeline ──────────────────────────────────────────────

    def execute(self, objective: str) -> SwarmTask:
        """
        Execute the full 4-phase swarm pipeline.

        Args:
            objective: What the swarm should accomplish.

        Returns:
            SwarmTask with all results and status.
        """
        task = SwarmTask(objective=objective)
        scratch = Scratchpad(task.task_id)
        task.scratchpad_dir = str(scratch.base_dir)
        self._tasks[task.task_id] = task
        self._save_state()

        log.info("Coordinator starting task %s: %s", task.task_id, objective)
        self._emit("started", task)

        try:
            # Phase 1: Research (2 workers in parallel)
            task.status = "researching"
            self._save_state()
            self._emit("phase.research", task)
            research_results = self._phase_research(task, scratch, objective)
            task.workers["research"] = research_results

            # Phase 2: Synthesis
            task.status = "synthesizing"
            self._save_state()
            self._emit("phase.synthesis", task)
            synthesis_result = self._phase_synthesis(task, scratch, objective)
            task.workers["synthesis"] = synthesis_result

            # Phase 3: Implementation
            task.status = "implementing"
            self._save_state()
            self._emit("phase.implementation", task)
            impl_result = self._phase_implementation(task, scratch, objective)
            task.workers["implementation"] = impl_result

            # Phase 4: Verification
            task.status = "verifying"
            self._save_state()
            self._emit("phase.verification", task)
            verify_result = self._phase_verification(task, scratch, objective)
            task.workers["verification"] = verify_result

            # Done
            task.status = "complete"
            task.completed_at = datetime.now().isoformat()
            self._save_state()
            self._emit("completed", task)
            self._log_to_brain(task)
            log.info("Coordinator task %s completed", task.task_id)

        except Exception as e:
            task.status = "failed"
            task.completed_at = datetime.now().isoformat()
            task.workers["error"] = str(e)
            self._save_state()
            self._emit("failed", task, error=str(e))
            log.error("Coordinator task %s failed: %s", task.task_id, e)

        return task

    def _phase_research(
        self, task: SwarmTask, scratch: Scratchpad, objective: str
    ) -> List[dict]:
        """Spawn 2 researcher workers in parallel."""
        workers = [
            Worker(
                role=WorkerRole.RESEARCHER,
                task_id=task.task_id,
                instructions=(
                    f"Research the following objective thoroughly. "
                    f"Focus on FACTUAL information, existing approaches, "
                    f"key constraints, and potential pitfalls.\n\n"
                    f"Objective: {objective}\n\n"
                    f"Angle: Explore the problem space, prior art, and constraints."
                ),
                scratchpad=scratch,
                worker_id=f"researcher-a-{task.task_id[:8]}",
                model=self.model,
            ),
            Worker(
                role=WorkerRole.RESEARCHER,
                task_id=task.task_id,
                instructions=(
                    f"Research the following objective thoroughly. "
                    f"Focus on IMPLEMENTATION details, tools, libraries, "
                    f"architecture patterns, and concrete approaches.\n\n"
                    f"Objective: {objective}\n\n"
                    f"Angle: Explore solutions, implementation strategies, and trade-offs."
                ),
                scratchpad=scratch,
                worker_id=f"researcher-b-{task.task_id[:8]}",
                model=self.model,
            ),
        ]

        results = []
        with ThreadPoolExecutor(max_workers=2) as pool:
            futures = {pool.submit(w.execute): w for w in workers}
            for future in as_completed(futures):
                results.append(future.result())

        return results

    def _phase_synthesis(
        self, task: SwarmTask, scratch: Scratchpad, objective: str
    ) -> dict:
        """Digest research and craft implementation instructions."""
        delegator = AntiLazyDelegator(model=self.model)

        # Read both research outputs
        research_a = scratch.read(f"researcher-a-{task.task_id[:8]}.md") or ""
        research_b = scratch.read(f"researcher-b-{task.task_id[:8]}.md") or ""
        combined_research = (
            f"## Research A (Problem Space)\n{research_a}\n\n"
            f"## Research B (Implementation)\n{research_b}"
        )

        # Digest into structured findings
        findings = delegator.digest_findings(combined_research)
        scratch.write("findings_digest.md", findings)

        # Craft specific implementation instructions
        impl_instructions = delegator.craft_instructions(
            findings_digest=findings,
            objective=objective,
            target_role="implementer",
        )
        scratch.write("impl_instructions.md", impl_instructions)

        return {
            "status": "completed",
            "findings_path": "findings_digest.md",
            "instructions_path": "impl_instructions.md",
        }

    def _phase_implementation(
        self, task: SwarmTask, scratch: Scratchpad, objective: str
    ) -> dict:
        """Spawn 1 implementer worker with crafted instructions."""
        instructions = scratch.read("impl_instructions.md") or ""
        worker = Worker(
            role=WorkerRole.IMPLEMENTER,
            task_id=task.task_id,
            instructions=instructions,
            scratchpad=scratch,
            model=self.model,
        )
        return worker.execute()

    def _phase_verification(
        self, task: SwarmTask, scratch: Scratchpad, objective: str
    ) -> dict:
        """Spawn 1 verifier worker to validate the implementation."""
        impl_output = scratch.read("implementer.md") or ""
        findings = scratch.read("findings_digest.md") or ""

        worker = Worker(
            role=WorkerRole.VERIFIER,
            task_id=task.task_id,
            instructions=(
                f"Verify the following implementation against the objective "
                f"and research findings.\n\n"
                f"## Objective\n{objective}\n\n"
                f"## Key Findings\n{findings}\n\n"
                f"## Implementation Output\n{impl_output}\n\n"
                f"Check for:\n"
                f"1. Correctness — does it achieve the objective?\n"
                f"2. Completeness — are there gaps or missing pieces?\n"
                f"3. Quality — is it well-structured and maintainable?\n"
                f"4. Risks — any potential issues or edge cases?\n\n"
                f"Produce a verification report with PASS/FAIL verdict "
                f"and specific issues if any."
            ),
            scratchpad=scratch,
            model=self.model,
        )
        return worker.execute()

    # ── Brain logging ─────────────────────────────────────────

    def _log_to_brain(self, task: SwarmTask) -> None:
        """Log completed task to today's Brain journal."""
        today = datetime.now().strftime("%Y_%m_%d")
        journal_path = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"
        journal_path.parent.mkdir(parents=True, exist_ok=True)

        entry = (
            f"- [[Coordinator]] swarm task `{task.task_id}` completed: "
            f"{task.objective}\n"
            f"  - Status: {task.status}\n"
            f"  - Scratchpad: `{task.scratchpad_dir}`\n"
        )
        with open(journal_path, "a", encoding="utf-8") as f:
            f.write(entry)

    # ── Task management ───────────────────────────────────────

    def list_tasks(self) -> List[SwarmTask]:
        """Return all tracked tasks."""
        return list(self._tasks.values())

    def get_task(self, task_id: str) -> Optional[SwarmTask]:
        """Get a specific task by ID."""
        return self._tasks.get(task_id)

    def cancel_task(self, task_id: str) -> bool:
        """
        Cancel a task (sets status to failed).

        Returns True if the task was found and cancelled.
        Note: does not interrupt running workers — they will
        finish but results will be ignored.
        """
        task = self._tasks.get(task_id)
        if not task:
            return False
        if task.status in ("complete", "failed"):
            return False
        task.status = "failed"
        task.completed_at = datetime.now().isoformat()
        task.workers["cancel_reason"] = "cancelled by user"
        self._save_state()
        self._emit("cancelled", task)
        return True
