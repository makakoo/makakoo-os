#!/usr/bin/env python3
"""
Goal Tracker — Sprint 6

Tracks goals with metadata, state, ownership, and linkage to tasks.
Goals are the top-level objectives that all agent work traces back to.

Based on Paperclip's goal schema.
Goals are stored in Brain as pages.
"""

import os
import uuid
import tempfile
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from pathlib import Path
from typing import Optional, List, Dict, Any

BRAIN_DIR = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / "data" / "Brain"


class GoalState(Enum):
    ACTIVE = "active"
    COMPLETED = "completed"
    ABANDONED = "abandoned"
    BLOCKED = "blocked"


class GoalPriority(Enum):
    CRITICAL = "critical"   # P0
    HIGH = "high"           # P1
    MEDIUM = "medium"       # P2
    LOW = "low"             # P3


@dataclass
class Goal:
    """One goal."""
    id: str                      # UUID
    title: str                   # Human-readable title
    description: str             # What this goal is about
    state: GoalState = GoalState.ACTIVE
    priority: GoalPriority = GoalPriority.MEDIUM

    # Hierarchy
    parent_id: Optional[str] = None  # Parent goal (if any)
    child_ids: List[str] = field(default_factory=list)  # Sub-goals

    # Metadata
    created_at: str = field(default_factory=lambda: datetime.now().isoformat())
    updated_at: str = field(default_factory=lambda: datetime.now().isoformat())
    created_by: str = "harvey"

    # Ownership
    owner: Optional[str] = None  # Agent or user responsible

    # Task linkage
    task_ids: List[str] = field(default_factory=list)  # Linked tasks

    # Progress
    progress_pct: float = 0.0   # 0.0 to 100.0

    # Notes
    notes: str = ""

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "title": self.title,
            "description": self.description,
            "state": self.state.value,
            "priority": self.priority.value,
            "parent_id": self.parent_id,
            "child_ids": self.child_ids,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "created_by": self.created_by,
            "owner": self.owner,
            "task_ids": self.task_ids,
            "progress_pct": self.progress_pct,
            "notes": self.notes,
        }

    @classmethod
    def from_dict(cls, d: dict) -> "Goal":
        """Reconstruct a Goal from a dict (e.g. parsed from page properties)."""
        # Handle state — might be a string or GoalState
        state_val = d.get("state", "active")
        if isinstance(state_val, str):
            state = GoalState(state_val)
        else:
            state = state_val

        priority_val = d.get("priority", "medium")
        if isinstance(priority_val, str):
            priority = GoalPriority(priority_val)
        else:
            priority = priority_val

        return cls(
            id=d["id"],
            title=d.get("title", ""),
            description=d.get("description", ""),
            state=state,
            priority=priority,
            parent_id=d.get("parent_id"),
            child_ids=d.get("child_ids", []),
            created_at=d.get("created_at", datetime.now().isoformat()),
            updated_at=d.get("updated_at", datetime.now().isoformat()),
            created_by=d.get("created_by", "harvey"),
            owner=d.get("owner"),
            task_ids=d.get("task_ids", []),
            progress_pct=float(d.get("progress_pct", 0.0)),
            notes=d.get("notes", ""),
        )


class GoalTracker:
    """Manages goals stored in Brain."""

    def __init__(self, goals_dir: Optional[Path] = None):
        self.goals_dir = goals_dir or (BRAIN_DIR / "pages" / "goals")
        self._goals_cache: Dict[str, Goal] = {}
        self._ensure_dir()

    def _ensure_dir(self):
        self.goals_dir.mkdir(parents=True, exist_ok=True)

    def _goal_file_path(self, goal_id: str, *, created_at: Optional[str] = None) -> Path:
        """Path to a goal's Brain page file.

        Stored at: goals/YYYY/goal_{id}.md

        Pass created_at for goals being saved (so we use the actual year).
        Without it, uses current year as fallback — callers handle cross-year search.
        """
        if created_at:
            year = created_at[:4]
        else:
            year = str(datetime.now().year)
        return self.goals_dir / year / f"goal_{goal_id}.md"

    def _parse_logseq_page(self, content: str) -> Dict[str, Any]:
        """Parse a Brain page into structured data.

        Handles the format:
          - Key:: value
          - ## Section
          body text
        """
        result = {}
        current_section = None
        section_body = []

        for line in content.splitlines():
            stripped = line.strip()

            # Property line: - Key:: value
            if stripped.startswith("- "):
                # Find the :: separator
                idx = stripped.find("::")
                if idx != -1:
                    key = stripped[2:idx].strip()
                    value = stripped[idx + 2 :].strip()
                    # Handle wikilinks like [[goal:xxx]]
                    result[key.lower()] = value
                    current_section = key.lower()
                    section_body = []
                continue

            # Markdown section header: ## Section
            if stripped.startswith("## "):
                # Save previous section
                if current_section:
                    result[current_section] = "\n".join(section_body).strip()
                current_section = stripped[3:].lower()
                section_body = []
                continue

            # Continuation of section body
            if current_section:
                section_body.append(stripped)

        # Save last section
        if current_section:
            result[current_section] = "\n".join(section_body).strip()

        return result

    def _format_logseq_page(self, goal: Goal) -> str:
        """Format a Goal as a Brain page."""
        lines = [
            "- Goal ID:: " + goal.id,
            "- Title:: " + goal.title,
            "- State:: " + goal.state.value,
            "- Priority:: " + goal.priority.value,
            "- Parent:: " + ("[[goal:" + goal.parent_id + "]]" if goal.parent_id else ""),
            "- Created:: " + goal.created_at,
            "- Owner:: " + (goal.owner or ""),
            "- Progress:: " + str(goal.progress_pct) + "%",
            "",
            "## Description",
            goal.description,
            "",
            "## Notes",
            goal.notes,
            "",
            "## Children",
        ]

        if goal.child_ids:
            for cid in goal.child_ids:
                lines.append("- [[goal:" + cid + "]]")
        else:
            lines.append("(none)")

        lines.extend(["", "## Tasks"])

        if goal.task_ids:
            for tid in goal.task_ids:
                lines.append("- [[task:" + tid + "]]")
        else:
            lines.append("(none)")

        lines.append("")

        return "\n".join(lines)

    def _save_goal(self, goal: Goal) -> None:
        """Save a goal to disk as a Brain page (atomic write)."""
        file_path = self._goal_file_path(goal.id, created_at=goal.created_at)
        file_path.parent.mkdir(parents=True, exist_ok=True)

        content = self._format_logseq_page(goal)

        # Atomic write: temp file + os.replace()
        fd, tmp_path_str = tempfile.mkstemp(
            dir=file_path.parent, suffix=".tmp", prefix=".goal_"
        )
        try:
            with os.fdopen(fd, "w") as f:
                f.write(content)
            os.replace(tmp_path_str, str(file_path))
        except Exception:
            # Clean up temp file if replace failed
            try:
                os.unlink(tmp_path_str)
            except OSError:
                pass
            raise

        # Update cache
        self._goals_cache[goal.id] = goal

    def _load_goal(self, goal_id: str) -> Optional[Goal]:
        """Load a goal from disk."""
        file_path = self._goal_file_path(goal_id)
        if not file_path.exists():
            # Try all year subdirs
            for year_dir in self.goals_dir.iterdir():
                if year_dir.is_dir():
                    candidate = year_dir / f"goal_{goal_id}.md"
                    if candidate.exists():
                        file_path = candidate
                        break
            else:
                return None

        try:
            content = file_path.read_text()
        except OSError:
            return None

        parsed = self._parse_logseq_page(content)

        # Build child_ids from children section links
        child_ids = []
        if "children" in parsed:
            import re
            child_ids = re.findall(r"goal:([a-f0-9-]+)", parsed["children"])

        # Build task_ids from tasks section links
        task_ids = []
        if "tasks" in parsed:
            import re
            task_ids = re.findall(r"task:([a-f0-9-]+)", parsed["tasks"])

        data = dict(parsed)
        data["id"] = goal_id
        data["child_ids"] = child_ids
        data["task_ids"] = task_ids

        return Goal.from_dict(data)

    def create_goal(
        self,
        title: str,
        description: str = "",
        priority: GoalPriority = GoalPriority.MEDIUM,
        parent_id: Optional[str] = None,
        owner: Optional[str] = None,
    ) -> Goal:
        """Create a new goal."""
        goal = Goal(
            id=str(uuid.uuid4()),
            title=title,
            description=description,
            priority=priority,
            parent_id=parent_id,
            owner=owner,
        )

        # If parent_id, add self to parent's child_ids
        if parent_id:
            parent = self.get_goal(parent_id)
            if parent:
                parent = Goal.from_dict(parent.to_dict())
                parent.child_ids = list(parent.child_ids)
                parent.child_ids.append(goal.id)
                parent.updated_at = datetime.now().isoformat()
                self._save_goal(parent)
            else:
                goal.parent_id = None

        self._save_goal(goal)
        return goal

    def get_goal(self, goal_id: str) -> Optional[Goal]:
        """Get a goal by ID."""
        # Check cache first
        if goal_id in self._goals_cache:
            return self._goals_cache[goal_id]

        goal = self._load_goal(goal_id)
        if goal:
            self._goals_cache[goal_id] = goal
        return goal

    def update_goal(self, goal_id: str, **kwargs) -> Optional[Goal]:
        """Update goal fields."""
        goal_data = self.get_goal(goal_id)
        if not goal_data:
            return None

        goal = Goal.from_dict(goal_data.to_dict())
        for key, value in kwargs.items():
            if hasattr(goal, key):
                setattr(goal, key, value)

        goal.updated_at = datetime.now().isoformat()
        self._save_goal(goal)
        return goal

    def complete_goal(self, goal_id: str) -> Optional[Goal]:
        """Mark a goal as completed."""
        return self.update_goal(goal_id, state=GoalState.COMPLETED, progress_pct=100.0)

    def add_child_goal(self, parent_id: str, child_id: str) -> None:
        """Add a child goal to a parent."""
        parent_data = self.get_goal(parent_id)
        if not parent_data:
            return

        parent = Goal.from_dict(parent_data.to_dict())
        if child_id not in parent.child_ids:
            parent.child_ids = list(parent.child_ids)
            parent.child_ids.append(child_id)
            parent.updated_at = datetime.now().isoformat()
            self._save_goal(parent)

    def link_task(self, goal_id: str, task_id: str) -> None:
        """Link a task to a goal."""
        goal_data = self.get_goal(goal_id)
        if not goal_data:
            return

        goal = Goal.from_dict(goal_data.to_dict())
        if task_id not in goal.task_ids:
            goal.task_ids = list(goal.task_ids)
            goal.task_ids.append(task_id)
            goal.updated_at = datetime.now().isoformat()
            self._save_goal(goal)

    def get_ancestry(self, goal_id: str) -> List[Goal]:
        """Get the full ancestry chain from this goal to root.

        Returns [goal, parent, grandparent, ...] ending at root goal.
        """
        ancestry = []
        current_id = goal_id
        visited = set()

        while current_id:
            if current_id in visited:
                break  # Prevent infinite loop on cycles
            visited.add(current_id)

            goal_data = self.get_goal(current_id)
            if not goal_data:
                break
            goal = Goal.from_dict(goal_data.to_dict())
            ancestry.append(goal)
            current_id = goal.parent_id

        return ancestry

    def get_descendants(self, goal_id: str) -> List[Goal]:
        """Get all descendant goals (children, grandchildren, etc)."""
        descendants = []
        visited = set()

        def walk(current_id: str):
            if current_id in visited:
                return
            visited.add(current_id)

            goal_data = self.get_goal(current_id)
            if not goal_data:
                return
            goal = Goal.from_dict(goal_data.to_dict())

            for child_id in goal.child_ids:
                descendants.append(goal)
                walk(child_id)

        walk(goal_id)
        return descendants

    def get_active_goals(self) -> List[Goal]:
        """Get all active goals sorted by priority."""
        active = []
        for year_dir in self.goals_dir.iterdir():
            if not year_dir.is_dir():
                continue
            for page_file in year_dir.glob("goal_*.md"):
                goal_id = page_file.stem.replace("goal_", "")
                goal_data = self.get_goal(goal_id)
                if goal_data and goal_data.state == GoalState.ACTIVE:
                    active.append(Goal.from_dict(goal_data.to_dict()))

        # Sort by priority (critical first)
        priority_order = {
            GoalPriority.CRITICAL: 0,
            GoalPriority.HIGH: 1,
            GoalPriority.MEDIUM: 2,
            GoalPriority.LOW: 3,
        }

        def sort_key(g: Goal):
            return (priority_order.get(g.priority, 999), g.created_at)

        active.sort(key=sort_key)
        return active
