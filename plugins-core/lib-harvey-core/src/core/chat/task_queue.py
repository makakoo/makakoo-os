"""
HarveyChat Task Queue — persistent job execution with state tracking.

Enables background tasks, progress updates, and multi-turn refinement.
Each task has a state machine: queued → running → awaiting_input → completed/failed

Example:
    task = TaskQueue.create_task(channel, user_id, goal="generate image of owl")

    # Task can progress across multiple user messages:
    task.run(message, history)        # Start execution
    task.set_awaiting_input("need clarification on style")  # Ask user
    # User responds...
    task.resume(user_response)        # Continue from where we left off
    task.complete(result)             # Mark done
"""

import asyncio
import json
import logging
import os
import sqlite3
import time
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

log = logging.getLogger("harveychat.task_queue")


class TaskState(Enum):
    """Task lifecycle states."""
    QUEUED = "queued"
    RUNNING = "running"
    AWAITING_INPUT = "awaiting_input"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"


@dataclass
class Task:
    """Individual task with persistent state."""
    id: str
    channel: str
    user_id: str
    goal: str
    state: TaskState = TaskState.QUEUED
    created_at: float = field(default_factory=time.time)
    started_at: Optional[float] = None
    completed_at: Optional[float] = None

    # Task context
    context: Dict[str, Any] = field(default_factory=dict)
    messages: List[Dict] = field(default_factory=list)  # [{"role": "user/assistant", "content": "..."}]

    # Progress tracking
    current_goal: str = ""  # What task is currently working on
    awaiting_input_prompt: str = ""  # If awaiting_input, what are we asking for
    progress_messages: List[str] = field(default_factory=list)  # "Working on image...", "Generated, refining..."

    # Final result
    result: str = ""
    files_to_send: List[tuple] = field(default_factory=list)  # [("photo", "/path/to/file"), ...]
    error: str = ""

    def is_active(self) -> bool:
        """Task is running or awaiting input."""
        return self.state in (TaskState.RUNNING, TaskState.AWAITING_INPUT)

    def is_complete(self) -> bool:
        """Task is done (success or failure)."""
        return self.state in (TaskState.COMPLETED, TaskState.FAILED, TaskState.CANCELLED)

    def to_dict(self) -> Dict:
        """Serialize to JSON."""
        return {
            "id": self.id,
            "channel": self.channel,
            "user_id": self.user_id,
            "goal": self.goal,
            "state": self.state.value,
            "created_at": self.created_at,
            "started_at": self.started_at,
            "completed_at": self.completed_at,
            "context": self.context,
            "messages": self.messages,
            "current_goal": self.current_goal,
            "awaiting_input_prompt": self.awaiting_input_prompt,
            "progress_messages": self.progress_messages,
            "result": self.result,
            "files_to_send": self.files_to_send,
            "error": self.error,
        }

    @classmethod
    def from_dict(cls, data: Dict) -> "Task":
        """Deserialize from JSON."""
        task = cls(
            id=data["id"],
            channel=data["channel"],
            user_id=data["user_id"],
            goal=data["goal"],
        )
        task.state = TaskState(data.get("state", "queued"))
        task.created_at = data.get("created_at", time.time())
        task.started_at = data.get("started_at")
        task.completed_at = data.get("completed_at")
        task.context = data.get("context", {})
        task.messages = data.get("messages", [])
        task.current_goal = data.get("current_goal", "")
        task.awaiting_input_prompt = data.get("awaiting_input_prompt", "")
        task.progress_messages = data.get("progress_messages", [])
        task.result = data.get("result", "")
        task.files_to_send = data.get("files_to_send", [])
        task.error = data.get("error", "")
        return task


class TaskQueue:
    """Persistent task queue with SQLite backend."""

    def __init__(self, db_path: str = None):
        if db_path is None:
            HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", str(Path.home() / "MAKAKOO")))
            db_path = str(HARVEY_HOME / "data" / "chat" / "tasks.db")

        Path(db_path).parent.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(db_path, check_same_thread=False)
        self.db.row_factory = sqlite3.Row
        self._init_schema()

        # In-memory task cache for active tasks
        self._cache: Dict[str, Task] = {}

    def _init_schema(self):
        """Create tables if needed."""
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                channel TEXT NOT NULL,
                user_id TEXT NOT NULL,
                goal TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at REAL NOT NULL,
                started_at REAL,
                completed_at REAL,
                context TEXT DEFAULT '{}',
                messages TEXT DEFAULT '[]',
                current_goal TEXT DEFAULT '',
                awaiting_input_prompt TEXT DEFAULT '',
                progress_messages TEXT DEFAULT '[]',
                result TEXT DEFAULT '',
                files_to_send TEXT DEFAULT '[]',
                error TEXT DEFAULT ''
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_user_active
                ON tasks(channel, user_id, state);

            CREATE INDEX IF NOT EXISTS idx_tasks_created
                ON tasks(created_at);
        """)
        self.db.commit()

    def create_task(
        self,
        channel: str,
        user_id: str,
        goal: str,
        context: Optional[Dict] = None,
    ) -> Task:
        """Create a new task."""
        import uuid
        task_id = f"task_{uuid.uuid4().hex[:12]}"

        task = Task(
            id=task_id,
            channel=channel,
            user_id=user_id,
            goal=goal,
            context=context or {},
        )

        self._save_task(task)
        self._cache[task_id] = task
        log.info(f"Created task {task_id}: {goal[:50]}...")
        return task

    def get_task(self, task_id: str) -> Optional[Task]:
        """Get task by ID (from cache or DB)."""
        # Check cache first
        if task_id in self._cache:
            return self._cache[task_id]

        # Load from DB
        row = self.db.execute(
            "SELECT * FROM tasks WHERE id = ?",
            (task_id,)
        ).fetchone()

        if not row:
            return None

        task = self._row_to_task(row)
        self._cache[task_id] = task
        return task

    def get_active_task(self, channel: str, user_id: str) -> Optional[Task]:
        """Get the active task for a user (if any)."""
        row = self.db.execute(
            "SELECT * FROM tasks WHERE channel = ? AND user_id = ? "
            "AND state IN (?, ?) "
            "ORDER BY created_at DESC LIMIT 1",
            (channel, user_id, TaskState.RUNNING.value, TaskState.AWAITING_INPUT.value),
        ).fetchone()

        if not row:
            return None

        return self._row_to_task(row)

    def update_task(self, task: Task):
        """Save task state to DB and cache."""
        self._save_task(task)
        self._cache[task.id] = task

    def set_running(self, task: Task):
        """Mark task as running."""
        task.state = TaskState.RUNNING
        task.started_at = time.time()
        self.update_task(task)

    def set_awaiting_input(self, task: Task, prompt: str):
        """Pause task waiting for user input."""
        task.state = TaskState.AWAITING_INPUT
        task.awaiting_input_prompt = prompt
        self.update_task(task)

    def set_completed(self, task: Task, result: str, files: Optional[List[tuple]] = None):
        """Mark task as completed successfully."""
        task.state = TaskState.COMPLETED
        task.result = result
        task.completed_at = time.time()
        if files:
            task.files_to_send = files
        self.update_task(task)

    def set_failed(self, task: Task, error: str):
        """Mark task as failed."""
        task.state = TaskState.FAILED
        task.error = error
        task.completed_at = time.time()
        self.update_task(task)

    def add_progress(self, task: Task, message: str):
        """Add a progress message."""
        task.progress_messages.append(message)
        self.update_task(task)

    def add_message(self, task: Task, role: str, content: str):
        """Add a message to task conversation."""
        task.messages.append({"role": role, "content": content})
        self.update_task(task)

    def _save_task(self, task: Task):
        """Persist task to database."""
        self.db.execute("""
            INSERT OR REPLACE INTO tasks (
                id, channel, user_id, goal, state, created_at, started_at, completed_at,
                context, messages, current_goal, awaiting_input_prompt, progress_messages,
                result, files_to_send, error
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """, (
            task.id,
            task.channel,
            task.user_id,
            task.goal,
            task.state.value,
            task.created_at,
            task.started_at,
            task.completed_at,
            json.dumps(task.context),
            json.dumps(task.messages),
            task.current_goal,
            task.awaiting_input_prompt,
            json.dumps(task.progress_messages),
            task.result,
            json.dumps(task.files_to_send),
            task.error,
        ))
        self.db.commit()

    def _row_to_task(self, row: sqlite3.Row) -> Task:
        """Convert DB row to Task object."""
        task = Task(
            id=row["id"],
            channel=row["channel"],
            user_id=row["user_id"],
            goal=row["goal"],
            state=TaskState(row["state"]),
            created_at=row["created_at"],
            started_at=row["started_at"],
            completed_at=row["completed_at"],
            context=json.loads(row["context"] or "{}"),
            messages=json.loads(row["messages"] or "[]"),
            current_goal=row["current_goal"] or "",
            awaiting_input_prompt=row["awaiting_input_prompt"] or "",
            progress_messages=json.loads(row["progress_messages"] or "[]"),
            result=row["result"] or "",
            files_to_send=json.loads(row["files_to_send"] or "[]"),
            error=row["error"] or "",
        )
        return task

    def close(self):
        """Close database."""
        self.db.close()
