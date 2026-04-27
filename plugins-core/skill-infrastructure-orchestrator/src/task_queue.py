"""
File-based priority queue for task orchestration.
Implements atomic enqueue/dequeue with exactly-once semantics.
"""

import json
import os
import shutil
import time
import uuid
from pathlib import Path
from typing import Optional


class TaskQueue:
    """File-based priority queue with atomic operations."""

    QUEUE_STATES = ["incoming", "running", "completed", "failed", "blocked"]

    def __init__(self, base_path: str = None):
        if base_path is None:
            import os
            _harvey_home = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
            base_path = os.path.join(_harvey_home, "data", "orchestrator", "queues")
        self.base = Path(base_path)
        for state in self.QUEUE_STATES:
            (self.base / state).mkdir(parents=True, exist_ok=True)

    def _atomic_write(self, path: Path, data: dict) -> None:
        """Atomic write: temp file + rename."""
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, indent=2))
        tmp.rename(path)

    def _atomic_read(self, path: Path) -> dict:
        """Read task from file."""
        return json.loads(path.read_text())

    def _move_task(self, task_id: str, from_state: str, to_state: str) -> bool:
        """Move task file between queue states atomically."""
        src = self.base / from_state / f"{task_id}.json"
        dst = self.base / to_state / f"{task_id}.json"

        if not src.exists():
            return False

        # Use processing suffix to avoid conflicts
        processing = self.base / from_state / f"{task_id}.processing"
        shutil.move(str(src), str(processing))
        shutil.move(str(processing), str(dst))
        return True

    def enqueue(self, task: dict) -> str:
        """
        Add task to incoming queue atomically.
        Returns the task_id.
        """
        if "task_id" not in task:
            task["task_id"] = str(uuid.uuid4())

        task_id = task["task_id"]
        task_path = self.base / "incoming" / f"{task_id}.json"
        self._atomic_write(task_path, task)
        return task_id

    def dequeue(self) -> Optional[dict]:
        """
        Pop highest priority task from incoming queue.
        Moves to running/. Returns None if empty.
        """
        incoming = self.base / "incoming"
        tasks = []

        for task_file in incoming.glob("*.json"):
            if task_file.suffix == ".processing":
                continue
            try:
                task = self._atomic_read(task_file)
                tasks.append((task["priority"], task_file.name, task))
            except (json.JSONDecodeError, KeyError):
                # Skip malformed files
                continue

        if not tasks:
            return None

        # Sort by priority (higher = more urgent)
        tasks.sort(key=lambda x: (-x[0], x[1]))
        _, filename, task = tasks[0]
        task_id = filename.replace(".json", "")

        if self._move_task(task_id, "incoming", "running"):
            task["status"] = "running"
            task["started_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            return task

        return None

    def complete(self, task_id: str, result: Optional[dict] = None) -> bool:
        """
        Mark task as completed. Optionally provide result.
        """
        task_file = self.base / "running" / f"{task_id}.json"
        if not task_file.exists():
            return False

        task = self._atomic_read(task_file)
        task["status"] = "completed"
        task["completed_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        if result is not None:
            task["result"] = result

        dst = self.base / "completed" / f"{task_id}.json"
        self._atomic_write(dst, task)
        task_file.unlink()
        return True

    def fail(self, task_id: str, error: Optional[str] = None) -> bool:
        """
        Mark task as failed with optional error message.
        """
        task_file = self.base / "running" / f"{task_id}.json"
        if not task_file.exists():
            return False

        task = self._atomic_read(task_file)
        task["status"] = "failed"
        task["completed_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        if error is not None:
            task["result"] = {"error": error}

        dst = self.base / "failed" / f"{task_id}.json"
        self._atomic_write(dst, task)
        task_file.unlink()
        return True

    def block(self, task_id: str) -> bool:
        """
        Move task to blocked state (dependencies not satisfied).
        """
        task_file = self.base / "incoming" / f"{task_id}.json"
        if not task_file.exists():
            return False

        task = self._atomic_read(task_file)
        task["status"] = "blocked"

        dst = self.base / "blocked" / f"{task_id}.json"
        self._atomic_write(dst, task)
        task_file.unlink()
        return True

    def unblock(self, task_id: str) -> bool:
        """
        Move blocked task back to incoming for re-evaluation.
        """
        task_file = self.base / "blocked" / f"{task_id}.json"
        if not task_file.exists():
            return False

        task = self._atomic_read(task_file)
        task["status"] = "pending"

        dst = self.base / "incoming" / f"{task_id}.json"
        self._atomic_write(dst, task)
        task_file.unlink()
        return True

    def get_task(self, task_id: str, state: str = None) -> Optional[dict]:
        """Get task by ID from specified state (or any state if not specified)."""
        if state:
            path = self.base / state / f"{task_id}.json"
            if path.exists():
                return self._atomic_read(path)
            return None

        for s in self.QUEUE_STATES:
            path = self.base / s / f"{task_id}.json"
            if path.exists():
                return self._atomic_read(path)
        return None

    def list_tasks(self, state: str) -> list[dict]:
        """List all tasks in a given state."""
        tasks = []
        state_dir = self.base / state
        if not state_dir.exists():
            return tasks

        for task_file in state_dir.glob("*.json"):
            if task_file.suffix == ".processing":
                continue
            try:
                tasks.append(self._atomic_read(task_file))
            except (json.JSONDecodeError, KeyError):
                continue
        return tasks
