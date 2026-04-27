#!/usr/bin/env python3
"""
Goal Hierarchy — Sprint 6

Manages the goal tree structure and traversal.
Injects goal ancestry context into agent prompts so sub-agents
understand the full "why" chain.

The insight: when a sub-agent works on a task, it should see:
"Working on: Implement user auth
Why: parent goal 'User accounts' → parent goal 'Launch v1.0'
Your role: You're implementing the auth module for the core platform."
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from enum import Enum
from typing import Optional

try:
    from .goal_tracker import Goal, GoalTracker, GoalState, GoalPriority
except ImportError:
    # goal_tracker.py not yet created — define minimal types for this module
    @dataclass
    class Goal:
        id: str = ""
        title: str = ""
        description: str = ""
        parent_id: Optional[str] = None
        priority: str = "MEDIUM"
        status: str = "active"
        progress: float = 0.0

        @property
        def state(self) -> str:
            return self.status

    class GoalState(Enum):
        ACTIVE = "active"
        COMPLETED = "completed"
        ARCHIVED = "archived"

    class GoalPriority(Enum):
        P0 = "P0"
        P1 = "P1"
        P2 = "P2"
        P3 = "P3"
        LOW = "LOW"
        MEDIUM = "MEDIUM"
        HIGH = "HIGH"

    class GoalTracker:
        """Minimal stub — replace when goal_tracker.py exists."""
        def __init__(self) -> None:
            self._goals: dict[str, Goal] = {}

        def get_goal(self, goal_id: str) -> Optional[Goal]:
            return self._goals.get(goal_id)

        def list_goals(self, **kwargs) -> list[Goal]:
            return list(self._goals.values())

        def add_goal(self, goal: Goal) -> None:
            self._goals[goal.id] = goal


# ---------------------------------------------------------------------------
# Priority helpers
# ---------------------------------------------------------------------------

_PRIORITY_LABELS = {
    "P0": "CRITICAL (P0)",
    "P1": "HIGH (P1)",
    "P2": "MEDIUM (P2)",
    "P3": "LOW (P3)",
    "CRITICAL": "CRITICAL (P0)",
    "HIGH": "HIGH (P1)",
    "MEDIUM": "MEDIUM (P2)",
    "LOW": "LOW (P3)",
}


def _priority_label(priority: str) -> str:
    return _PRIORITY_LABELS.get(priority.upper(), priority.upper())


def _is_high_priority(priority: str) -> bool:
    return priority.upper() in ("P0", "P1", "CRITICAL", "HIGH")


# ---------------------------------------------------------------------------
# GoalHierarchy
# ---------------------------------------------------------------------------


class GoalHierarchy:
    """Manages goal tree traversal and context injection."""

    def __init__(self, tracker: Optional[GoalTracker] = None):
        self.tracker = tracker or GoalTracker()

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _get_goal_or_raise(self, goal_id: str) -> Optional[Goal]:
        """Retrieve a goal by ID, or None if not found."""
        if hasattr(self.tracker, "get_goal"):
            return self.tracker.get_goal(goal_id)
        # Fallback for stub tracker
        if hasattr(self.tracker, "_goals"):
            return self.tracker._goals.get(goal_id)
        return None

    def _list_goals(self, **kwargs) -> list[Goal]:
        """List goals using the tracker interface."""
        if hasattr(self.tracker, "list_goals"):
            return self.tracker.list_goals(**kwargs)
        if hasattr(self.tracker, "_goals"):
            return list(self.tracker._goals.values())
        return []

    def _get_ancestry(self, goal_id: str, max_depth: int = 10) -> list[Goal]:
        """Walk the parent chain from goal_id to root. Returns [root, ..., parent, goal]."""
        ancestry: list[Goal] = []
        current = self._get_goal_or_raise(goal_id)
        visited: set[str] = set()

        while current is not None and len(ancestry) < max_depth:
            if current.id in visited:
                break  # Guard against cycles in data
            visited.add(current.id)
            ancestry.append(current)
            if current.parent_id is None:
                break
            current = self._get_goal_or_raise(current.parent_id)

        ancestry.reverse()  # root first
        return ancestry

    def _get_siblings(self, goal: Goal) -> list[Goal]:
        """Return other children of goal's parent (excluding goal itself)."""
        if goal.parent_id is None:
            return []
        siblings: list[Goal] = []
        for g in self._list_goals():
            if g.parent_id == goal.parent_id and g.id != goal.id:
                siblings.append(g)
        return siblings

    def _get_children(self, goal_id: str, completed_only: bool = False) -> list[Goal]:
        """Return direct children of a goal."""
        children: list[Goal] = []
        for g in self._list_goals():
            if g.parent_id == goal_id:
                if completed_only:
                    try:
                        status = g.status if hasattr(g, "status") else g.state
                        if status != "completed":
                            continue
                    except Exception:
                        pass
                children.append(g)
        return children

    def _progress_str(self, goal: Goal) -> str:
        """Format progress as a percentage string."""
        try:
            prog = float(goal.progress) if hasattr(goal, "progress") else 0.0
            return f"{prog:.0%}"
        except (ValueError, TypeError):
            return "0%"

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def get_context_for_goal(
        self,
        goal_id: str,
        include_subgoals: bool = False,
        max_depth: int = 10,
    ) -> str:
        """Generate a context string for a goal to inject into prompts.

        Shows: goal title, description, priority, ancestry chain,
        and optionally sub-goals.

        Example output:
        '''
        ## Goal Context

        Working on: Implement user authentication
        Priority: HIGH (P1)

        Why this matters:
        └── Launch v1.0 (root goal)
            └── User accounts
                └── Implement user authentication (THIS)

        Description:
        Add JWT-based authentication with OAuth2 social login.

        Sibling goals (same parent):
        - User profile management
        - Password reset flow

        Progress: 30%
        '''
        """
        goal = self._get_goal_or_raise(goal_id)
        if goal is None:
            return f"## Goal Context\n\nGoal '{goal_id}' not found in tracker."

        ancestry = self._get_ancestry(goal_id, max_depth=max_depth)
        chain_str = self.format_goal_chain(ancestry, current_goal_idx=len(ancestry) - 1)
        siblings = self._get_siblings(goal)
        children = self._get_children(goal_id) if include_subgoals else []

        lines = [
            "## Goal Context",
            "",
            f"**Working on:** {goal.title}",
            f"**Priority:** {_priority_label(goal.priority)}",
            "",
            "**Why this matters:**",
            chain_str,
            "",
        ]

        if goal.description:
            lines.append("**Description:**")
            lines.append(goal.description)
            lines.append("")

        if siblings:
            lines.append("**Sibling goals (same parent):**")
            for sib in siblings:
                status = ""
                try:
                    s = sib.status if hasattr(sib, "status") else sib.state
                    status = f" [{s}]"
                except Exception:
                    pass
                lines.append(f"- {sib.title}{status}")
            lines.append("")

        if children:
            lines.append("**Sub-goals:**")
            for child in children:
                done = "✓" if child.status == "completed" else "○"
                lines.append(f"- {done} {child.title}")
            lines.append("")

        prog = self._progress_str(goal)
        lines.append(f"**Progress:** {prog}")

        return "\n".join(lines)

    def get_context_for_task(
        self,
        task_id: str,
        goal_id: str,
    ) -> str:
        """Generate context for a task that's linked to a goal.

        More focused than get_context_for_goal — specifically for
        when the agent is working on a named task within a goal.
        """
        goal = self._get_goal_or_raise(goal_id)
        if goal is None:
            return f"## Task Context\n\nGoal '{goal_id}' not found in tracker."

        ancestry = self._get_ancestry(goal_id)
        parent_chain = " → ".join(g.title for g in ancestry)

        lines = [
            "## Task Context",
            "",
            f"**Task ID:** {task_id}",
            f"**Parent goal:** {goal.title}",
            f"**Goal chain:** {parent_chain}",
            "",
        ]

        if goal.description:
            lines.append("**Goal description:**")
            lines.append(goal.description)
            lines.append("")

        prog = self._progress_str(goal)
        lines.append(f"**Goal progress:** {prog}")

        return "\n".join(lines)

    def get_subgoal_context(
        self,
        parent_goal_id: str,
        completed_only: bool = False,
    ) -> str:
        """Get context listing all sub-goals of a parent."""
        parent = self._get_goal_or_raise(parent_goal_id)
        if parent is None:
            return f"## Sub-Goals\n\nParent goal '{parent_goal_id}' not found."

        children = self._get_children(parent_goal_id, completed_only=completed_only)

        if not children:
            status_str = " (completed only)" if completed_only else ""
            return f"## Sub-Goals of '{parent.title}'{status_str}\n\nNo sub-goals found."

        status_str = " (completed only)" if completed_only else ""

        lines = [
            f"## Sub-Goals of '{parent.title}'{status_str}",
            "",
        ]

        if completed_only:
            lines.append("Showing completed sub-goals only.\n")
        else:
            todo: list[Goal] = []
            done: list[Goal] = []
            for child in children:
                if child.status == "completed":
                    done.append(child)
                else:
                    todo.append(child)

            if todo:
                lines.append("**In progress / todo:**")
                for g in todo:
                    prog = self._progress_str(g)
                    lines.append(f"- ○ {g.title} [{prog}]")
                lines.append("")
            if done:
                lines.append("**Completed:**")
                for g in done:
                    lines.append(f"- ✓ {g.title}")
                lines.append("")

        return "\n".join(lines)

    def format_goal_chain(
        self,
        goals: list[Goal],
        current_goal_idx: int = -1,
    ) -> str:
        """Format a list of goals as a tree branch.

        goals = [root_goal, child, grandchild]
        current_goal_idx = 2 (last = current)

        Output:
        └── Launch v1.0 (root goal)
            └── User accounts
                └── Implement user authentication (THIS)
        """
        if not goals:
            return "(no goals)"

        lines: list[str] = []
        for i, goal in enumerate(goals):
            is_last = (i == len(goals) - 1)
            is_current = (i == current_goal_idx)

            if i == 0:
                # Root — no tree prefix
                prefix = "└── " if is_last else "├── "
            else:
                prefix = "    " * (i - 1)
                prefix += "└── " if is_last else "├── "

            label = f"{goal.title} (THIS)" if is_current else goal.title
            lines.append(f"{prefix}{label}")

        return "\n".join(lines)

    def build_task_intro(
        self,
        task_description: str,
        goal_id: str,
    ) -> str:
        """Generate an introductory context block for a sub-agent task.

        This is what gets prepended to a sub-agent's system prompt
        when it spawns to work on a goal-linked task.

        Produces 3-5 concise sentences.
        """
        goal = self._get_goal_or_raise(goal_id)
        if goal is None:
            return (
                f"You are working on: {task_description}\n\n"
                f"Goal '{goal_id}' not found in tracker — proceed with the task as described."
            )

        ancestry = self._get_ancestry(goal_id)

        if len(ancestry) == 1:
            why = f"This goal is at the top level: {ancestry[0].title}."
        else:
            parent_titles = [g.title for g in ancestry[:-1]]
            why = f"It feeds into: {' → '.join(parent_titles)}."

        lines = [
            f"You are working on: {task_description}.",
            why,
        ]

        if goal.description:
            # Truncate long descriptions to first sentence or ~100 chars
            desc = goal.description.strip()
            if len(desc) > 120:
                desc = desc[:117].rsplit(" ", 1)[0] + "..."
            lines.append(f"The goal is: {desc}")

        # Role framing
        if len(ancestry) >= 2:
            root = ancestry[0].title
            immediate_parent = ancestry[-2].title
            lines.append(
                f"Your role: implementing the {goal.title} piece "
                f"within '{immediate_parent}', which supports the top-level goal '{root}'."
            )
        else:
            lines.append(f"Your role: working on the top-level goal '{goal.title}'.")

        return " ".join(lines)

    def suggest_parent_goal(
        self,
        goal_title: str,
        description: str = "",
    ) -> Optional[str]:
        """Suggest a parent goal for a new goal based on title/description similarity.

        Uses keyword matching — if title contains words from an existing
        goal's title, suggest it as parent.

        Returns: parent goal ID or None.
        """
        # Extract significant words (length >= 3, not common stopwords)
        STOPWORDS = {
            "the", "and", "for", "with", "from", "this", "that", "using",
            "into", "from", "via", "our", "your", "make", "build", "create",
            "add", "new", "set", "use", "implement", "design", "plan",
        }

        def keywords(text: str) -> set[str]:
            words = re.findall(r"\b[a-zA-Z]{3,}\b", text.lower())
            return {w for w in words if w not in STOPWORDS}

        new_keywords = keywords(goal_title)

        best_score = 0
        best_id: Optional[str] = None

        for goal in self._list_goals():
            if goal.id == goal_title:  # Skip self
                continue
            existing_keywords = keywords(goal.title)
            overlap = new_keywords & existing_keywords
            if len(overlap) > best_score:
                best_score = len(overlap)
                best_id = goal.id

        # Require at least one meaningful keyword overlap
        if best_score >= 1:
            return best_id
        return None

    def validate_no_cycle(self, goal_id: str, new_parent_id: str) -> bool:
        """Verify that setting new_parent_id as parent of goal_id won't create a cycle.

        A cycle would be: goal_id → ... → new_parent_id → ... → goal_id

        Walks from new_parent_id up its ancestry — if we hit goal_id, it's a cycle.
        """
        if goal_id == new_parent_id:
            return False  # Direct self-loop is a cycle

        visited: set[str] = set()
        current_id: Optional[str] = new_parent_id

        while current_id is not None:
            if current_id in visited:
                # Already seen — something wrong in data, treat as cycle
                return False
            if current_id == goal_id:
                return False  # Cycle detected

            visited.add(current_id)
            goal = self._get_goal_or_raise(current_id)
            if goal is None:
                break
            current_id = goal.parent_id

        return True  # No cycle found

    def get_breadcrumb(self, goal_id: str) -> str:
        """Get a short breadcrumb string for a goal.

        Example: "Launch v1.0 > User accounts > Auth"
        """
        ancestry = self._get_ancestry(goal_id)
        if not ancestry:
            return f"Unknown goal: {goal_id}"

        # Use abbreviated titles for the breadcrumb
        def abbreviate(title: str) -> str:
            # Shorten long titles to first 2-3 significant words
            words = title.split()
            if len(words) <= 3:
                return title
            # Pick first 2 words + ellipsis
            significant = [w for w in words if len(w) > 2 and w.lower() not in (
                "the", "and", "for", "with", "from"
            )]
            if len(significant) >= 2:
                return " ".join(significant[:2]) + "…"
            return " ".join(words[:2]) + "…"

        parts = [abbreviate(g.title) for g in ancestry]
        return " > ".join(parts)
