#!/usr/bin/env python3
"""
Task Linker — Sprint 6

Links tasks/work items to goals. When spawning a sub-agent for a task,
injects the full goal ancestry context so the agent understands WHY.

Task is stored as a Brain page:
data/Brain/pages/tasks/{YYYY}/{task_id}.md

Each task has:
- goal_id: which goal this belongs to
- title, description, state
- assigned_to: agent/user
- created_at, updated_at
"""

from __future__ import annotations

import os
import re
import uuid
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from pathlib import Path
from typing import Optional, List, Dict, Any

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

BRAIN_DIR = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / "data" / "Brain"
TASKS_DIR = BRAIN_DIR / "pages" / "tasks"


# ---------------------------------------------------------------------------
# GoalHierarchy — imported or minimal stub
# ---------------------------------------------------------------------------

class GoalHierarchy:
    """
    Provides goal ancestry context for tasks.

    In production this is the real GoalHierarchy from goal_hierarchy.py.
    This stub provides get_context_for_task() for standalone use.
    """

    def get_context_for_task(self, goal_id: str, task_title: str) -> str:
        """
        Return a formatted context string showing the goal ancestry.

        When goal_hierarchy.py exists, this delegates to the real implementation.
        """
        return (
            f"Goal: {goal_id}\n"
            f"Task: {task_title}\n"
            f"This task is part of the goal hierarchy above."
        )

    def get_goal_ancestry(self, goal_id: str) -> List[Dict[str, str]]:
        """
        Return list of {id, title, level} for the goal and its ancestors.
        Stub returns the goal_id itself.
        """
        return [{"id": goal_id, "title": goal_id, "level": "goal"}]


# Try to import the real GoalHierarchy if it exists
try:
    from .goal_hierarchy import GoalHierarchy as _RealGoalHierarchy

    class GoalHierarchy(_RealGoalHierarchy):
        """Real GoalHierarchy — inherits get_context_for_task from goal_hierarchy.py."""
        pass

except ImportError:
    pass  # Use the stub above


# ---------------------------------------------------------------------------
# Enums & Dataclasses
# ---------------------------------------------------------------------------


class TaskState(Enum):
    TODO = "todo"
    IN_PROGRESS = "in_progress"
    DONE = "done"
    BLOCKED = "blocked"


@dataclass
class Task:
    """One task linked to a goal."""
    id: str
    goal_id: str
    title: str
    description: str = ""
    state: TaskState = TaskState.TODO
    assigned_to: Optional[str] = None

    created_at: str = field(default_factory=lambda: datetime.now().isoformat())
    updated_at: str = field(default_factory=lambda: datetime.now().isoformat())
    created_by: str = "harvey"

    # For sub-agent injection
    context: str = ""  # Pre-computed goal context

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "goal_id": self.goal_id,
            "title": self.title,
            "description": self.description,
            "state": self.state.value,
            "assigned_to": self.assigned_to,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "context": self.context,
        }

    @classmethod
    def from_dict(cls, d: dict) -> Task:
        return cls(
            id=d["id"],
            goal_id=d["goal_id"],
            title=d["title"],
            description=d.get("description", ""),
            state=TaskState(d.get("state", "todo")),
            assigned_to=d.get("assigned_to"),
            created_at=d.get("created_at", datetime.now().isoformat()),
            updated_at=d.get("updated_at", datetime.now().isoformat()),
            context=d.get("context", ""),
        )


# ---------------------------------------------------------------------------
# TaskLinker
# ---------------------------------------------------------------------------


class TaskLinker:
    """Manages task-to-goal linkage and context injection."""

    def __init__(
        self,
        tasks_dir: Optional[Path] = None,
        hierarchy: Optional[GoalHierarchy] = None,
    ):
        self.tasks_dir = tasks_dir or TASKS_DIR
        self.hierarchy = hierarchy or GoalHierarchy()
        self._tasks_cache: Dict[str, Task] = {}
        self._ensure_dir()

    def _ensure_dir(self) -> None:
        """Ensure the tasks directory exists."""
        self.tasks_dir.mkdir(parents=True, exist_ok=True)

    # ---------------------------------------------------------------------------
    # Task file path helpers
    # ---------------------------------------------------------------------------

    def _task_file_path(self, task_id: str) -> Path:
        """
        Path to task Brain page.

        Format: tasks/YYYY/{task_id}.md
        """
        # Extract year from task_id if it's a UUID, otherwise use current year
        # Task IDs are UUIDs so we use the created date or current year
        year = datetime.now().strftime("%Y")
        return self.tasks_dir / year / f"{task_id}.md"

    def _year_from_task_id(self, task_id: str) -> str:
        """
        Try to determine the year from a task file on disk.
        Falls back to current year if not found.
        """
        # Scan existing year directories
        if self.tasks_dir.exists():
            for year_dir in self.tasks_dir.iterdir():
                if year_dir.is_dir() and year_dir.name.isdigit():
                    potential = year_dir / f"{task_id}.md"
                    if potential.exists():
                        return year_dir.name
        return datetime.now().strftime("%Y")

    # ---------------------------------------------------------------------------
    # Load / Save (atomic)
    # ---------------------------------------------------------------------------

    def _load_task(self, task_id: str) -> Optional[Task]:
        """Load task from disk."""
        # Check cache first
        if task_id in self._tasks_cache:
            return self._tasks_cache[task_id]

        year = self._year_from_task_id(task_id)
        path = self.tasks_dir / year / f"{task_id}.md"

        if not path.exists():
            return None

        try:
            content = path.read_text()
            task = self._parse_logseq_page(content)
            if task:
                self._tasks_cache[task_id] = task
            return task
        except Exception:
            return None

    def _save_task(self, task: Task) -> None:
        """Save task to disk (atomic: temp file + os.replace)."""
        year = datetime.now().strftime("%Y")
        task_dir = self.tasks_dir / year
        task_dir.mkdir(parents=True, exist_ok=True)

        path = task_dir / f"{task.id}.md"
        temp_path = task_dir / f".{task.id}.md.tmp"

        content = self._render_logseq_page(task)

        # Atomic write
        temp_path.write_text(content)
        os.replace(temp_path, path)

        # Update cache
        self._tasks_cache[task.id] = task

    def _render_logseq_page(self, task: Task) -> str:
        """Render a Task as a Logseq page string."""
        lines = [
            "- Task ID:: {{",
            f"- Goal:: [[goal:{task.goal_id}]]",
            f"- Title:: {task.title}",
            f"- State:: {task.state.value}",
            f"- Assigned:: {task.assigned_to or 'unassigned'}",
            f"- Created:: {task.created_at}",
            "",
            "## Description",
            task.description or "_No description_",
            "",
            "## Goal Context",
            task.context,
        ]
        return "\n".join(lines)

    def _parse_logseq_page(self, content: str) -> Optional[Task]:
        """Parse a Logseq page string into a Task."""
        # Extract Task ID
        id_match = re.search(r"- Task ID::\s*(\S+)", content)
        goal_match = re.search(r"- Goal::\s*\[\[goal:([^\]]+)\]\]", content)
        title_match = re.search(r"- Title::\s*(.+)", content)
        state_match = re.search(r"- State::\s*(\S+)", content)
        assigned_match = re.search(r"- Assigned::\s*(.+)", content)
        created_match = re.search(r"- Created::\s*(.+)", content)

        if not id_match:
            return None

        # Extract context (everything after "## Goal Context")
        context = ""
        if "## Goal Context" in content:
            context_section = content.split("## Goal Context")[1]
            # Skip the "## Description" section if it comes before
            if "## Description" in context_section:
                parts = context_section.split("## Description")
                context = parts[0].strip()
            else:
                context = context_section.strip()

        # Extract description
        description = ""
        if "## Description" in content and "## Goal Context" in content:
            desc_section = content.split("## Description")[1].split("## Goal Context")[0]
            description = desc_section.strip()

        task = Task(
            id=id_match.group(1).strip(),
            goal_id=goal_match.group(1).strip() if goal_match else "",
            title=title_match.group(1).strip() if title_match else "",
            description=description,
            state=TaskState(state_match.group(1).strip()) if state_match else TaskState.TODO,
            assigned_to=assigned_match.group(1).strip() if assigned_match else None,
            created_at=created_match.group(1).strip() if created_match else datetime.now().isoformat(),
            context=context,
        )
        return task

    # ---------------------------------------------------------------------------
    # Public API
    # ---------------------------------------------------------------------------

    def create_task(
        self,
        goal_id: str,
        title: str,
        description: str = "",
        assigned_to: Optional[str] = None,
    ) -> Task:
        """
        Create a new task linked to a goal.

        1. Create Task with UUID
        2. Pre-compute context using hierarchy.get_context_for_task()
        3. Link task to goal via tracker.link_task() [stubbed]
        4. Save to disk
        5. Return Task
        """
        task_id = str(uuid.uuid4())

        # Pre-compute goal context
        context = self.hierarchy.get_context_for_task(goal_id, title)

        task = Task(
            id=task_id,
            goal_id=goal_id,
            title=title,
            description=description,
            state=TaskState.TODO,
            assigned_to=assigned_to,
            context=context,
        )

        # Link to goal tracker (stub — goal_tracker.py may implement this)
        self._link_task_to_goal(task.id, goal_id)

        # Save to disk
        self._save_task(task)

        return task

    def _link_task_to_goal(self, task_id: str, goal_id: str) -> None:
        """
        Stub for linking a task to a goal in goal_tracker.
        When goal_tracker.py exists, this delegates to it.
        """
        # In production, this would call goal_tracker.link_task(task_id, goal_id)
        pass

    def get_task(self, task_id: str) -> Optional[Task]:
        """Get a task by ID."""
        return self._load_task(task_id)

    def update_task(self, task_id: str, **kwargs) -> Optional[Task]:
        """Update task fields."""
        task = self._load_task(task_id)
        if not task:
            return None

        # Apply updates
        if "title" in kwargs:
            task.title = kwargs["title"]
        if "description" in kwargs:
            task.description = kwargs["description"]
        if "state" in kwargs:
            state_val = kwargs["state"]
            if isinstance(state_val, str):
                task.state = TaskState(state_val)
            else:
                task.state = state_val
        if "assigned_to" in kwargs:
            task.assigned_to = kwargs["assigned_to"]

        task.updated_at = datetime.now().isoformat()

        # Re-compute context if goal changed
        if "goal_id" in kwargs:
            task.goal_id = kwargs["goal_id"]
            task.context = self.hierarchy.get_context_for_task(task.goal_id, task.title)

        self._save_task(task)
        return task

    def complete_task(self, task_id: str) -> Optional[Task]:
        """Mark task as done."""
        return self.update_task(task_id, state=TaskState.DONE)

    def assign_task(self, task_id: str, agent_id: str) -> Optional[Task]:
        """Assign task to an agent."""
        return self.update_task(task_id, assigned_to=agent_id)

    def get_tasks_for_goal(self, goal_id: str) -> List[Task]:
        """
        Get all tasks linked to a goal.

        Walks tasks dir, loads tasks with matching goal_id.
        """
        tasks: List[Task] = []
        if not self.tasks_dir.exists():
            return tasks

        for year_dir in self.tasks_dir.iterdir():
            if not year_dir.is_dir() or not year_dir.name.isdigit():
                continue
            for task_file in year_dir.glob("*.md"):
                try:
                    content = task_file.read_text()
                    task = self._parse_logseq_page(content)
                    if task and task.goal_id == goal_id:
                        tasks.append(task)
                        self._tasks_cache[task.id] = task
                except Exception:
                    continue

        return tasks

    def get_agent_tasks(self, agent_id: str) -> List[Task]:
        """Get all tasks assigned to an agent."""
        tasks: List[Task] = []
        if not self.tasks_dir.exists():
            return tasks

        for year_dir in self.tasks_dir.iterdir():
            if not year_dir.is_dir() or not year_dir.name.isdigit():
                continue
            for task_file in year_dir.glob("*.md"):
                try:
                    content = task_file.read_text()
                    task = self._parse_logseq_page(content)
                    if task and task.assigned_to == agent_id:
                        tasks.append(task)
                        self._tasks_cache[task.id] = task
                except Exception:
                    continue

        return tasks

    def get_agent_context(self, agent_id: str) -> str:
        """
        Get a formatted context block for what an agent should work on.

        Shows: assigned tasks with goal context, so the agent knows
        WHY each task matters.

        Example:
        '''
        You have 3 tasks to work on:

        1. Implement user authentication (HIGH priority)
           Because: User accounts > Launch v1.0
           Status: IN_PROGRESS

        2. Add OAuth2 social login (MEDIUM priority)
           Because: User accounts > Launch v1.0
           Status: TODO
        '''
        """
        tasks = self.get_agent_tasks(agent_id)
        if not tasks:
            return f"No tasks assigned to {agent_id}."

        # Sort by state priority then by created_at
        state_order = {
            TaskState.BLOCKED: 0,
            TaskState.IN_PROGRESS: 1,
            TaskState.TODO: 2,
            TaskState.DONE: 3,
        }
        tasks_sorted = sorted(
            tasks,
            key=lambda t: (state_order.get(t.state, 99), t.created_at),
        )

        lines = [f"You have {len(tasks)} task(s) to work on:\n"]
        for i, task in enumerate(tasks_sorted, 1):
            if task.state == TaskState.DONE:
                continue  # Skip completed tasks in context
            status_str = task.state.value.replace("_", " ").upper()
            lines.append(f"{i}. {task.title} ({status_str})")
            lines.append(f"   Because: {task.goal_id}")
            lines.append(f"   Context: {task.context[:100]}..." if len(task.context) > 100 else f"   Context: {task.context}")
            lines.append("")

        return "\n".join(lines).strip()

    def inject_context_into_prompt(
        self,
        task_id: str,
        base_prompt: str,
    ) -> str:
        """
        Inject goal context into an agent's prompt.

        When spawning a sub-agent to work on a task, call this
        to prepend the goal context to the base prompt.
        """
        task = self._load_task(task_id)
        if not task:
            return base_prompt

        context_block = (
            f"\n{'='*60}\n"
            f"TASK CONTEXT\n"
            f"{'='*60}\n"
            f"Task ID: {task.id}\n"
            f"Goal: {task.goal_id}\n"
            f"Title: {task.title}\n"
            f"State: {task.state.value}\n"
            f"\n"
            f"Goal Context:\n"
            f"{task.context}\n"
            f"{'='*60}\n"
            f"END TASK CONTEXT\n"
            f"{'='*60}\n\n"
        )

        return context_block + base_prompt

    def list_tasks(
        self,
        state: Optional[TaskState] = None,
        goal_id: Optional[str] = None,
        assigned_to: Optional[str] = None,
    ) -> List[Task]:
        """
        List tasks with optional filters.
        """
        tasks: List[Task] = []
        if not self.tasks_dir.exists():
            return tasks

        for year_dir in self.tasks_dir.iterdir():
            if not year_dir.is_dir() or not year_dir.name.isdigit():
                continue
            for task_file in year_dir.glob("*.md"):
                try:
                    content = task_file.read_text()
                    task = self._parse_logseq_page(content)
                    if not task:
                        continue
                    if state is not None and task.state != state:
                        continue
                    if goal_id is not None and task.goal_id != goal_id:
                        continue
                    if assigned_to is not None and task.assigned_to != assigned_to:
                        continue
                    tasks.append(task)
                    self._tasks_cache[task.id] = task
                except Exception:
                    continue

        return tasks

    def delete_task(self, task_id: str) -> bool:
        """
        Delete a task. Returns True if deleted, False if not found.
        """
        year = self._year_from_task_id(task_id)
        path = self.tasks_dir / year / f"{task_id}.md"

        if not path.exists():
            return False

        try:
            path.unlink()
            if task_id in self._tasks_cache:
                del self._tasks_cache[task_id]
            return True
        except Exception:
            return False
