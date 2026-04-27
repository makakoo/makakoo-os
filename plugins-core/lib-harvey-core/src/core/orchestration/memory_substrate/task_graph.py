"""
Task Graph — Task DAG tracking for agent orchestration.
"""
import uuid
import time
from dataclasses import dataclass, field
from typing import Optional


@dataclass
class TaskNode:
    task_id: str
    status: str = "pending"  # pending | running | complete | failed
    artifacts: list = field(default_factory=list)
    depends_on: list = field(default_factory=list)
    created_at: float = field(default_factory=time.time)
    completed_at: float = 0


class TaskGraphLayer:
    """Task DAG tracking for agent orchestration."""

    def __init__(self):
        self.tasks: dict[str, TaskNode] = {}

    def add_task(self, task_id: str, depends_on: list[str] = None) -> TaskNode:
        """Add a task to the graph."""
        node = TaskNode(
            task_id=task_id,
            depends_on=depends_on or [],
        )
        self.tasks[task_id] = node
        return node

    def start_task(self, task_id: str) -> bool:
        """Mark a task as running."""
        node = self.tasks.get(task_id)
        if not node:
            return False
        node.status = "running"
        return True

    def complete_task(self, task_id: str, artifacts: list[str] = None) -> bool:
        """Mark a task as complete."""
        node = self.tasks.get(task_id)
        if not node:
            return False
        node.status = "complete"
        node.completed_at = time.time()
        if artifacts:
            node.artifacts.extend(artifacts)
        return True

    def fail_task(self, task_id: str) -> bool:
        """Mark a task as failed."""
        node = self.tasks.get(task_id)
        if not node:
            return False
        node.status = "failed"
        return True

    def get_ready_tasks(self) -> list[str]:
        """Get tasks whose dependencies are all complete."""
        ready = []
        for task_id, node in self.tasks.items():
            if node.status != "pending":
                continue
            all_done = all(
                self.tasks.get(dep, TaskNode(task_id=dep, status="pending")).status == "complete"
                for dep in node.depends_on
            )
            if all_done:
                ready.append(task_id)
        return ready

    def get_task(self, task_id: str) -> Optional[TaskNode]:
        return self.tasks.get(task_id)
