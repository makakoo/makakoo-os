"""
Harvey Workflow Engine — Orchestrates complex multi-step tasks with agents.

Handles: multi-day tasks, agent handoffs, checkpointing, human feedback, failures.

Example workflow (image generation campaign):
  Step 1: Gather requirements (Harvey agent)
    - Ask user for style, dimensions, usage rights
    - Save answers to context
  Step 2: Generate variations (Image Gen agent)
    - Takes context from Step 1
    - Generates 3 variations
    - Saves to workflow state
  Step 3: Get user feedback (HarveyChat pause point)
    - Show variations to user
    - Wait for user selection
    - Resume workflow with choice
  Step 4: Upscale selected image (Processing agent)
    - Takes chosen variation
    - Upscales to 4K
    - Saves result
  Step 5: Archive (Storage agent)
    - Move to permanent storage
    - Update Brain metadata
    - Done

Each step is checkpointed. If any step fails, workflow resumes from last checkpoint.
"""

import json
import logging
import os
import sqlite3
import time
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

log = logging.getLogger("workflow.engine")


class WorkflowState(Enum):
    """Workflow lifecycle states."""
    DRAFT = "draft"  # Being defined
    QUEUED = "queued"  # Waiting to run
    RUNNING = "running"  # Currently executing
    PAUSED = "paused"  # Waiting for human feedback
    WAITING_RESOURCE = "waiting_resource"  # Waiting for external resource
    COMPLETED = "completed"  # Done (success)
    FAILED = "failed"  # Failed, won't retry
    CANCELLED = "cancelled"  # User cancelled


class StepState(Enum):
    """Individual workflow step states."""
    PENDING = "pending"  # Hasn't run yet
    RUNNING = "running"  # Currently executing
    CHECKPOINTED = "checkpointed"  # Completed and saved
    PAUSED = "paused"  # Waiting for input
    FAILED = "failed"  # Failed, requires intervention
    SKIPPED = "skipped"  # Skipped by logic


@dataclass
class WorkflowStep:
    """Individual step in a workflow."""
    id: str
    name: str
    agent: str  # Which agent to use (harvey, image-gen, processor, etc.)
    action: str  # What to do ("gather_requirements", "generate", "feedback", etc.)
    state: StepState = StepState.PENDING

    # Execution
    started_at: Optional[float] = None
    completed_at: Optional[float] = None

    # Data flow
    input_context: Dict[str, Any] = field(default_factory=dict)  # What step receives
    output_context: Dict[str, Any] = field(default_factory=dict)  # What step produces

    # Control flow
    pause_prompt: str = ""  # If paused, what are we asking for?
    error: str = ""  # If failed, why
    retry_count: int = 0
    max_retries: int = 1

    # Dependencies
    depends_on: List[str] = field(default_factory=list)  # step IDs this depends on

    def is_complete(self) -> bool:
        return self.state in (StepState.CHECKPOINTED, StepState.SKIPPED)

    def is_blocked(self) -> bool:
        return self.state in (StepState.PAUSED, StepState.FAILED, StepState.RUNNING)

    def to_dict(self) -> Dict:
        return {
            "id": self.id,
            "name": self.name,
            "agent": self.agent,
            "action": self.action,
            "state": self.state.value,
            "started_at": self.started_at,
            "completed_at": self.completed_at,
            "input_context": self.input_context,
            "output_context": self.output_context,
            "pause_prompt": self.pause_prompt,
            "error": self.error,
            "retry_count": self.retry_count,
            "max_retries": self.max_retries,
            "depends_on": self.depends_on,
        }

    @classmethod
    def from_dict(cls, data: Dict) -> "WorkflowStep":
        step = cls(
            id=data["id"],
            name=data["name"],
            agent=data["agent"],
            action=data["action"],
        )
        step.state = StepState(data.get("state", "pending"))
        step.started_at = data.get("started_at")
        step.completed_at = data.get("completed_at")
        step.input_context = data.get("input_context", {})
        step.output_context = data.get("output_context", {})
        step.pause_prompt = data.get("pause_prompt", "")
        step.error = data.get("error", "")
        step.retry_count = data.get("retry_count", 0)
        step.max_retries = data.get("max_retries", 1)
        step.depends_on = data.get("depends_on", [])
        return step


@dataclass
class Workflow:
    """Multi-step workflow orchestration."""
    id: str
    name: str
    description: str = ""
    state: WorkflowState = WorkflowState.DRAFT

    # Metadata
    created_at: float = field(default_factory=time.time)
    started_at: Optional[float] = None
    completed_at: Optional[float] = None

    # Structure
    steps: List[WorkflowStep] = field(default_factory=list)

    # Global context (shared across all steps)
    context: Dict[str, Any] = field(default_factory=dict)

    # Progress
    current_step_idx: int = 0

    # Control
    pause_reason: str = ""  # Why workflow is paused
    failure_strategy: str = "pause"  # "pause" or "skip" on step failure

    def get_step(self, step_id: str) -> Optional[WorkflowStep]:
        return next((s for s in self.steps if s.id == step_id), None)

    def get_current_step(self) -> Optional[WorkflowStep]:
        if 0 <= self.current_step_idx < len(self.steps):
            return self.steps[self.current_step_idx]
        return None

    def can_start_step(self, step: WorkflowStep) -> bool:
        """Check if step's dependencies are satisfied."""
        for dep_id in step.depends_on:
            dep = self.get_step(dep_id)
            if not dep or not dep.is_complete():
                return False
        return True

    def mark_step_complete(self, step: WorkflowStep, output: Dict[str, Any]):
        """Mark step as checkpointed with its output."""
        step.state = StepState.CHECKPOINTED
        step.completed_at = time.time()
        step.output_context = output
        # Merge output into global context
        self.context.update(output)

    def pause_at_step(self, step: WorkflowStep, prompt: str):
        """Pause workflow at step, waiting for user feedback."""
        self.state = WorkflowState.PAUSED
        step.state = StepState.PAUSED
        step.pause_prompt = prompt
        self.pause_reason = f"Step '{step.name}' paused: {prompt}"

    def fail_step(self, step: WorkflowStep, error: str):
        """Mark step as failed."""
        step.state = StepState.FAILED
        step.error = error
        step.retry_count += 1

        if step.retry_count >= step.max_retries:
            self.state = WorkflowState.FAILED

    def to_dict(self) -> Dict:
        return {
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "state": self.state.value,
            "created_at": self.created_at,
            "started_at": self.started_at,
            "completed_at": self.completed_at,
            "steps": [s.to_dict() for s in self.steps],
            "context": self.context,
            "current_step_idx": self.current_step_idx,
            "pause_reason": self.pause_reason,
            "failure_strategy": self.failure_strategy,
        }

    @classmethod
    def from_dict(cls, data: Dict) -> "Workflow":
        wf = cls(
            id=data["id"],
            name=data["name"],
            description=data.get("description", ""),
        )
        wf.state = WorkflowState(data.get("state", "draft"))
        wf.created_at = data.get("created_at", time.time())
        wf.started_at = data.get("started_at")
        wf.completed_at = data.get("completed_at")
        wf.steps = [WorkflowStep.from_dict(s) for s in data.get("steps", [])]
        wf.context = data.get("context", {})
        wf.current_step_idx = data.get("current_step_idx", 0)
        wf.pause_reason = data.get("pause_reason", "")
        wf.failure_strategy = data.get("failure_strategy", "pause")
        return wf


class WorkflowEngine:
    """Executes workflows with checkpointing and agent coordination."""

    def __init__(self, db_path: str = None):
        if db_path is None:
            HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", str(Path.home() / "MAKAKOO")))
            db_path = str(HARVEY_HOME / "data" / "workflow" / "workflows.db")

        Path(db_path).parent.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(db_path, check_same_thread=False)
        self.db.row_factory = sqlite3.Row
        self._init_schema()

        # In-memory cache
        self._workflows: Dict[str, Workflow] = {}

        # Step handlers registry
        self._handlers: Dict[tuple, Callable] = {}  # (agent, action) -> handler func

    def _init_schema(self):
        """Create tables if needed."""
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS workflows (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT DEFAULT '',
                state TEXT NOT NULL,
                created_at REAL NOT NULL,
                started_at REAL,
                completed_at REAL,
                data TEXT NOT NULL,
                parent_workflow_id TEXT
            );

            CREATE TABLE IF NOT EXISTS workflow_checkpoints (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workflow_id TEXT NOT NULL,
                step_id TEXT NOT NULL,
                checkpoint_at REAL NOT NULL,
                context TEXT NOT NULL,
                FOREIGN KEY(workflow_id) REFERENCES workflows(id)
            );

            CREATE INDEX IF NOT EXISTS idx_workflows_state
                ON workflows(state, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_checkpoints_workflow
                ON workflow_checkpoints(workflow_id);
        """)
        self.db.commit()

    def register_handler(self, agent: str, action: str, handler: Callable):
        """Register a handler for (agent, action) combination."""
        key = (agent, action)
        self._handlers[key] = handler
        log.info(f"Registered handler for {agent}/{action}")

    def create_workflow(
        self,
        name: str,
        description: str = "",
        steps: Optional[List[WorkflowStep]] = None,
    ) -> Workflow:
        """Create a new workflow."""
        import uuid
        wf_id = f"wf_{uuid.uuid4().hex[:12]}"

        wf = Workflow(
            id=wf_id,
            name=name,
            description=description,
            steps=steps or [],
        )

        self._save_workflow(wf)
        self._workflows[wf_id] = wf
        log.info(f"Created workflow {wf_id}: {name}")
        return wf

    def get_workflow(self, wf_id: str) -> Optional[Workflow]:
        """Get workflow (from cache or DB)."""
        if wf_id in self._workflows:
            return self._workflows[wf_id]

        row = self.db.execute(
            "SELECT data FROM workflows WHERE id = ?", (wf_id,)
        ).fetchone()

        if not row:
            return None

        wf = Workflow.from_dict(json.loads(row["data"]))
        self._workflows[wf_id] = wf
        return wf

    def save_workflow(self, wf: Workflow):
        """Persist workflow to DB."""
        self._save_workflow(wf)
        self._workflows[wf.id] = wf

    def start_workflow(self, wf: Workflow):
        """Start workflow execution."""
        wf.state = WorkflowState.RUNNING
        wf.started_at = time.time()
        self.save_workflow(wf)
        log.info(f"Started workflow {wf.id}")

    def execute_next_step(self, wf: Workflow) -> Optional[str]:
        """
        Execute the next available step.
        Returns: (step_id, result) or None if nothing to do.

        Respects:
        - Step dependencies
        - Pause points
        - Checkpoints
        """
        if wf.state == WorkflowState.PAUSED:
            log.info(f"Workflow {wf.id} is paused: {wf.pause_reason}")
            return None

        if wf.state in (WorkflowState.COMPLETED, WorkflowState.FAILED, WorkflowState.CANCELLED):
            log.info(f"Workflow {wf.id} is {wf.state.value}, no more steps")
            return None

        # Find next step to run
        for i, step in enumerate(wf.steps):
            if step.is_complete():
                continue  # Already done

            if step.is_blocked():
                continue  # Paused or failed

            if not wf.can_start_step(step):
                log.info(f"Step {step.id} blocked by dependencies")
                continue

            # Found step to execute
            log.info(f"Executing step {step.id}: {step.name} ({step.agent}/{step.action})")

            try:
                # Prepare input context for this step
                step.input_context = wf.context.copy()
                step.state = StepState.RUNNING
                step.started_at = time.time()
                self.save_workflow(wf)

                # Look up handler
                handler = self._handlers.get((step.agent, step.action))
                if not handler:
                    raise ValueError(f"No handler for {step.agent}/{step.action}")

                # Execute step
                result = handler(step, wf.context)

                # Check if step paused itself
                if step.state == StepState.PAUSED:
                    wf.pause_at_step(step, step.pause_prompt)
                    self.save_workflow(wf)
                    self._checkpoint(wf, step)
                    return step.id

                # Mark complete
                wf.mark_step_complete(step, result or {})
                self._checkpoint(wf, step)
                self.save_workflow(wf)
                log.info(f"Step {step.id} completed")

                return step.id

            except Exception as e:
                error_msg = str(e)
                log.error(f"Step {step.id} failed: {error_msg}")
                wf.fail_step(step, error_msg)

                if wf.failure_strategy == "pause":
                    wf.state = WorkflowState.PAUSED
                    wf.pause_reason = f"Step failed: {error_msg}"
                elif wf.failure_strategy == "skip":
                    step.state = StepState.SKIPPED
                    # Continue to next step

                self.save_workflow(wf)
                return None

        # All steps complete
        wf.state = WorkflowState.COMPLETED
        wf.completed_at = time.time()
        self.save_workflow(wf)
        log.info(f"Workflow {wf.id} completed")
        return None

    def resume_workflow(self, wf: Workflow, user_input: Optional[Dict] = None):
        """Resume paused workflow (user provided feedback)."""
        if wf.state != WorkflowState.PAUSED:
            log.warning(f"Workflow {wf.id} is not paused")
            return

        # Find paused step
        paused_step = next((s for s in wf.steps if s.state == StepState.PAUSED), None)
        if paused_step:
            # Feed user input into context
            if user_input:
                wf.context.update(user_input)
                paused_step.input_context.update(user_input)
            paused_step.state = StepState.CHECKPOINTED

        wf.state = WorkflowState.RUNNING
        wf.pause_reason = ""
        self.save_workflow(wf)
        log.info(f"Resumed workflow {wf.id}")

    def _checkpoint(self, wf: Workflow, step: WorkflowStep):
        """Save checkpoint after step completion."""
        self.db.execute(
            "INSERT INTO workflow_checkpoints (workflow_id, step_id, checkpoint_at, context) "
            "VALUES (?, ?, ?, ?)",
            (wf.id, step.id, time.time(), json.dumps(wf.context)),
        )
        self.db.commit()
        log.info(f"Checkpointed step {step.id} in workflow {wf.id}")

    def _save_workflow(self, wf: Workflow):
        """Persist workflow to database."""
        self.db.execute(
            "INSERT OR REPLACE INTO workflows "
            "(id, name, description, state, created_at, started_at, completed_at, data) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                wf.id,
                wf.name,
                wf.description,
                wf.state.value,
                wf.created_at,
                wf.started_at,
                wf.completed_at,
                json.dumps(wf.to_dict()),
            ),
        )
        self.db.commit()

    def close(self):
        """Close database."""
        self.db.close()
