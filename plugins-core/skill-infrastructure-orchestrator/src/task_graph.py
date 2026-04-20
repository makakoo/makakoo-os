"""
Task dependency graph with DAG support.
Implements Kahn's algorithm for topological sort and failure propagation.
"""

import json
import uuid
from collections import defaultdict, deque
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional


@dataclass
class TaskNode:
    """Represents a task node in the dependency graph."""
    task_id: str
    parent_id: Optional[str]
    description: str
    agent_type: str
    model: str
    priority: int
    dependencies: list[str] = field(default_factory=list)
    payload: dict = field(default_factory=dict)
    status: str = "pending"  # pending, running, completed, failed, blocked
    result: Optional[dict] = None
    created_at: str = ""
    started_at: Optional[str] = None
    completed_at: Optional[str] = None

    def to_dict(self) -> dict:
        return {
            "task_id": self.task_id,
            "parent_id": self.parent_id,
            "description": self.description,
            "agent_type": self.agent_type,
            "model": self.model,
            "priority": self.priority,
            "dependencies": self.dependencies,
            "payload": self.payload,
            "status": self.status,
            "result": self.result,
            "created_at": self.created_at,
            "started_at": self.started_at,
            "completed_at": self.completed_at,
        }

    @classmethod
    def from_dict(cls, data: dict) -> "TaskNode":
        return cls(
            task_id=data["task_id"],
            parent_id=data.get("parent_id"),
            description=data["description"],
            agent_type=data.get("agent_type", "general-purpose"),
            model=data.get("model", "minimax:M2"),
            priority=data.get("priority", 5),
            dependencies=data.get("dependencies", []),
            payload=data.get("payload", {}),
            status=data.get("status", "pending"),
            result=data.get("result"),
            created_at=data.get("created_at", ""),
            started_at=data.get("started_at"),
            completed_at=data.get("completed_at"),
        )


class TaskGraph:
    """
    Directed Acyclic Graph (DAG) for task dependencies.
    Supports Kahn's algorithm for topological sort and failure propagation.
    """

    def __init__(self, state_path: str = None):
        if state_path is None:
            import os
            _harvey_home = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
            state_path = os.path.join(_harvey_home, "data", "orchestrator", "state")
        self.state_path = Path(state_path)
        self.state_path.mkdir(parents=True, exist_ok=True)

        self.nodes: dict[str, TaskNode] = {}
        # edges: task_id -> [dependent_task_ids] (who depends on this task)
        self.edges: dict[str, list[str]] = defaultdict(list)
        # reverse_edges: task_id -> [dependency_task_ids] (what this task depends on)
        self.reverse_edges: dict[str, list[str]] = defaultdict(list)

        self._load_state()

    def _state_file(self) -> Path:
        return self.state_path / "task_graph.json"

    def _load_state(self) -> None:
        """Load graph state from disk."""
        state_file = self._state_file()
        if state_file.exists():
            try:
                data = json.loads(state_file.read_text())
                for task_data in data.get("nodes", []):
                    node = TaskNode.from_dict(task_data)
                    self.nodes[node.task_id] = node
                self.edges = defaultdict(list, data.get("edges", {}))
                self.reverse_edges = defaultdict(list, data.get("reverse_edges", {}))
            except (json.JSONDecodeError, KeyError):
                pass

    def _save_state(self) -> None:
        """Persist graph state to disk."""
        state_file = self._state_file()
        tmp = state_file.with_suffix(".tmp")
        data = {
            "nodes": [n.to_dict() for n in self.nodes.values()],
            "edges": dict(self.edges),
            "reverse_edges": dict(self.reverse_edges),
        }
        tmp.write_text(json.dumps(data, indent=2))
        tmp.rename(state_file)

    def add_task(self, task: dict) -> str:
        """
        Add a task node and its dependencies to the graph.
        Returns the task_id.
        """
        if isinstance(task, dict):
            if "task_id" not in task:
                task["task_id"] = str(uuid.uuid4())
            node = TaskNode.from_dict(task)
        else:
            node = task

        self.nodes[node.task_id] = node

        # Build edges
        for dep_id in node.dependencies:
            self.edges[dep_id].append(node.task_id)
            self.reverse_edges[node.task_id].append(dep_id)

        self._save_state()
        return node.task_id

    def get_runnable(self) -> list[str]:
        """
        Return task_ids where all dependencies are satisfied (status=completed).
        Excludes tasks that are already running, completed, or failed.
        """
        runnable = []
        for task_id, node in self.nodes.items():
            if node.status != "pending":
                continue

            deps_satisfied = all(
                self.nodes[dep_id].status == "completed"
                for dep_id in node.dependencies
            )
            if deps_satisfied:
                runnable.append(task_id)

        # Sort by priority (higher = more urgent)
        runnable.sort(
            key=lambda tid: (-self.nodes[tid].priority, self.nodes[tid].created_at)
        )
        return runnable

    def topological_sort(self) -> list[str]:
        """
        Kahn's algorithm for topological sort.
        Returns tasks in dependency order (parents before children).
        """
        # Compute in-degree (number of dependencies)
        in_degree = {tid: len(node.dependencies) for tid, node in self.nodes.items()}

        # Start with nodes that have no dependencies
        queue = deque([tid for tid, deg in in_degree.items() if deg == 0])
        result = []

        while queue:
            task_id = queue.popleft()
            result.append(task_id)

            # Reduce in-degree for dependents
            for dependent_id in self.edges[task_id]:
                in_degree[dependent_id] -= 1
                if in_degree[dependent_id] == 0:
                    queue.append(dependent_id)

        # If result doesn't include all nodes, there's a cycle
        if len(result) != len(self.nodes):
            raise ValueError("Cycle detected in task graph")

        return result

    def notify_completed(self, task_id: str, result: Optional[dict] = None) -> list[str]:
        """
        Mark task as completed and unblock any dependents.
        Returns list of newly unblocked task_ids.
        """
        if task_id not in self.nodes:
            return []

        node = self.nodes[task_id]
        node.status = "completed"
        if result:
            node.result = result

        unblocked = []
        for dependent_id in self.edges[task_id]:
            dep_node = self.nodes[dependent_id]
            # Check if all dependencies are now satisfied
            if dep_node.status == "blocked":
                all_deps_done = all(
                    self.nodes[d].status == "completed"
                    for d in dep_node.dependencies
                )
                if all_deps_done:
                    dep_node.status = "pending"
                    unblocked.append(dependent_id)

        self._save_state()
        return unblocked

    def notify_failed(self, task_id: str, error: Optional[str] = None) -> list[str]:
        """
        Mark task as failed and propagate failure to all dependents.
        Returns list of task_ids that were marked as failed.
        """
        if task_id not in self.nodes:
            return []

        node = self.nodes[task_id]
        node.status = "failed"
        if error:
            node.result = {"error": error}

        failed = []
        # BFS through dependents
        queue = deque(self.edges[task_id])
        while queue:
            dep_id = queue.popleft()
            if dep_id not in self.nodes:
                continue
            dep_node = self.nodes[dep_id]
            if dep_node.status not in ("completed", "failed"):
                dep_node.status = "failed"
                dep_node.result = {"error": f"Dependency {task_id} failed", "original_error": error}
                failed.append(dep_id)
                queue.extend(self.edges[dep_id])

        self._save_state()
        return failed

    def get_dependents(self, task_id: str) -> list[str]:
        """Get all tasks that directly or transitively depend on this task."""
        dependents = []
        queue = deque(self.edges[task_id])
        visited = set()

        while queue:
            dep_id = queue.popleft()
            if dep_id in visited:
                continue
            visited.add(dep_id)
            dependents.append(dep_id)
            queue.extend(self.edges[dep_id])

        return dependents

    def update_status(self, task_id: str, status: str) -> bool:
        """Update task status."""
        if task_id in self.nodes:
            self.nodes[task_id].status = status
            self._save_state()
            return True
        return False
