#!/usr/bin/env python3
"""
Harvey OS Auto-Improver — wired Sprint 1–6 into one coherent class.

Usage:
    improver = AutoImprover(
        session_id="abc123",
        memory_nudge_interval=10,
        skill_nudge_interval=10,
        compaction_policy=CompactionPolicy(),
        budget_policy=BudgetPolicy(),
    )

    # In agent loop — after each user turn:
    improver.on_turn()

    # After each API call:
    improver.on_api_call(input_tokens=500, output_tokens=1000)

    # After each tool call:
    improver.on_tool_call("Read", success=True)

    # Check if review should fire:
    if improver.should_review():
        improver.spawn_review(messages, callback=my_callback)

    # Check if budget exceeded:
    if improver.should_stop():
        raise BudgetExceededError()

    # Check if compaction needed:
    if improver.should_compact():
        improver.run_compaction()

    # Activity log:
    improver.activity_logger.log_skill_created("my-skill", "dev")

    # Goal context for sub-agent:
    context = improver.get_goal_context(goal_id)

    # User-facing summary:
    print(improver.get_status_summary())
"""

from __future__ import annotations

import threading
from datetime import datetime, timezone
from typing import Any, Callable, List, Optional

# ---------------------------------------------------------------------------
# Sprint 1 — Core nudge + review + iteration budget
# ---------------------------------------------------------------------------
from .nudge_triggers import NudgeState
from .review_spawner import spawn_background_review
from .brain_writer import BrainWriter
from .iteration_budget import (
    IterationBudget as _IterationBudget,
    FREE_TOOLS,
    is_free_tool as _is_free_tool,
)

# ---------------------------------------------------------------------------
# Sprint 2 — skill_manage (stub — module does not exist yet)
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Sprint 3 — Compaction
# ---------------------------------------------------------------------------
from .compaction_policy import (
    CompactionPolicy,
    CompactionState,
    resolve_compaction_policy,
)
from .handoff_generator import HandoffGenerator
from .session_archiver import SessionArchiver

# ---------------------------------------------------------------------------
# Sprint 4 — Budget
# ---------------------------------------------------------------------------
from .budget_tracker import BudgetTracker
from .budget_enforcer import (
    BudgetEnforcer,
    BudgetState,
    BudgetLimit,
    BudgetStatus,
)
from .budget_config import (
    BudgetPolicy,
    DEFAULT_POLICY,
    resolve_budget_policy,
)

# ---------------------------------------------------------------------------
# Sprint 5 — Activity Logging
# ---------------------------------------------------------------------------
from .activity_logger import ActivityLogger, ActivityAction

# ---------------------------------------------------------------------------
# Sprint 6 — Goals
# ---------------------------------------------------------------------------
from .goal_tracker import Goal, GoalState, GoalPriority, GoalTracker
from .goal_hierarchy import GoalHierarchy
from .task_linker import Task, TaskState, TaskLinker

# ---------------------------------------------------------------------------
# Exception
# ---------------------------------------------------------------------------


class BudgetExceededError(Exception):
    """Raised when the budget has been exceeded and the agent should stop."""
    pass


# ---------------------------------------------------------------------------
# AutoImprover
# ---------------------------------------------------------------------------


class AutoImprover:
    """
    Harvey OS Auto-Improver — complete self-improvement system.

    Wires: nudges + review + budget + compaction + logging + goals.
    """

    # Free tools whose iterations are refunded (don't count against iteration budget)
    FREE_TOOLS = FREE_TOOLS

    def __init__(
        self,
        session_id: str,
        # Sprint 1
        memory_nudge_interval: int = 10,
        skill_nudge_interval: int = 10,
        # Sprint 3
        compaction_policy: Optional[CompactionPolicy] = None,
        # Sprint 4
        budget_policy: Optional[BudgetPolicy] = None,
        warning_pct: float = 80.0,
    ):
        self.session_id = session_id

        # Sprint 1: Nudge state
        self._nudge_state = NudgeState(
            memory_nudge_interval=memory_nudge_interval,
            skill_nudge_interval=skill_nudge_interval,
        )

        # Sprint 1: Iteration budget
        self._iteration_budget = _IterationBudget(max_total=100)

        # Sprint 3: Compaction
        self._compaction_policy = compaction_policy if compaction_policy is not None else CompactionPolicy()
        self._compaction_state = CompactionState(session_id=session_id)
        self._handoff_generator = HandoffGenerator()
        self._session_archiver = SessionArchiver()

        # Sprint 4: Budget
        resolved_policy = budget_policy if budget_policy is not None else DEFAULT_POLICY
        self._budget_tracker = BudgetTracker()
        self._budget_enforcer = BudgetEnforcer(
            tracker=self._budget_tracker,
            limits=[],  # Populated below
            warning_pct=warning_pct,
            on_warning=self._on_budget_warning,
            on_exceeded=self._on_budget_exceeded,
        )
        self._budget_policy = resolved_policy
        self._budget_warning_pct = warning_pct
        self._sync_budget_limits()

        # Sprint 5: Activity logger (lazy)
        self._activity_logger: Optional[ActivityLogger] = None

        # Sprint 6: Goal hierarchy (lazy)
        self._goal_tracker: Optional[GoalTracker] = None
        self._goal_hierarchy: Optional[GoalHierarchy] = None
        self._task_linker: Optional[TaskLinker] = None

        # Review callback (set by spawn_review caller)
        self._review_callback: Optional[Callable[[str], None]] = None

    # -------------------------------------------------------------------------
    # Sprint 1: Nudge + Review
    # -------------------------------------------------------------------------

    def on_turn(self) -> None:
        """Called after each user turn. Increments turn counter and logs."""
        self._nudge_state.on_turn()
        self._log_activity(ActivityAction.AGENT_TOOL_CALL, "turn", self.session_id,
                           {"turn": self._nudge_state.turns_since_memory})

    def on_tool_call(self, tool_name: str, success: bool) -> None:
        """
        Called after each tool call.

        - Increments the iteration counter
        - Resets nudge counters for memory/skill tools
        - Refunds or consumes iteration budget for free/paid tools
        - Records tool call in activity log
        """
        self._nudge_state.on_iteration()

        # Reset nudge counters for relevant tools
        tn = tool_name.lower()
        if tn in ("brain_bridge", "logseq_bridge", "brain_writer", "create_page", "append_block",
                  "log_to_today_journal", "upsert_property", "memory", "brain"):
            self._nudge_state.on_memory_used()
        if tn in ("skill_manage", "skill_manage_tool"):
            self._nudge_state.on_skill_used()

        # Iteration budget: free tools are refunded
        if _is_free_tool(tool_name):
            self._iteration_budget.refund()
        else:
            if not self._iteration_budget.consume():
                # Budget exhausted — don't count this as a consume, just log
                pass

        # Activity log
        self._log_activity(
            ActivityAction.AGENT_TOOL_CALL,
            tool_name,
            self.session_id,
            {"success": success},
        )

    def should_review_memory(self) -> bool:
        """Returns True if memory review should fire this turn."""
        return self._nudge_state.should_review_memory()

    def should_review_skills(self) -> bool:
        """Returns True if skill review should fire this iteration."""
        return self._nudge_state.should_review_skills()

    def should_review(self) -> bool:
        """Returns True if memory OR skill review should fire."""
        return self.should_review_memory() or self.should_review_skills()

    def spawn_review(
        self,
        messages: List[dict],
        callback: Optional[Callable[[str], None]] = None,
    ) -> None:
        """
        Spawn a daemon-thread background review if thresholds are hit.

        Args:
            messages: Conversation message snapshot to review
            callback: Optional callback to surface the review result
        """
        mem = self.should_review_memory()
        skill = self.should_review_skills()

        if not mem and not skill:
            return

        # Reset counters
        if mem:
            self._nudge_state.turns_since_memory = 0
        if skill:
            self._nudge_state.iters_since_skill = 0

        # Store callback for the review thread to use
        self._review_callback = callback

        spawn_background_review(
            messages_snapshot=messages,
            review_memory=mem,
            review_skills=skill,
            callback=callback,
        )

    @property
    def budget(self) -> _IterationBudget:
        """Sprint 1 iteration budget property."""
        return self._iteration_budget

    # -------------------------------------------------------------------------
    # Sprint 4: Budget
    # -------------------------------------------------------------------------

    def on_api_call(self, input_tokens: int, output_tokens: int) -> None:
        """
        Record token usage after each API call.

        Also updates compaction state (Sprint 3) with these tokens.
        """
        # Sprint 4: Record in budget tracker
        self._budget_tracker.record_call(
            session_id=self.session_id,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )

        # Sprint 3: Update compaction state
        self._compaction_state.record_run(input_tokens, output_tokens)

        # Sprint 5: Log API call
        self._log_activity(
            ActivityAction.AGENT_TOOL_CALL,
            "api_call",
            self.session_id,
            {"input_tokens": input_tokens, "output_tokens": output_tokens},
        )

    def check_budget(self) -> BudgetStatus:
        """Check current budget status. Returns BudgetStatus."""
        return self._budget_enforcer.check(self.session_id)

    def can_continue(self) -> bool:
        """Returns True if session is within budget and can continue."""
        return self._budget_enforcer.can_continue(self.session_id)

    def should_stop(self) -> bool:
        """Returns True if budget has been exceeded (hard stop)."""
        return self._budget_enforcer.should_stop(self.session_id)

    def _on_budget_warning(self, session_id: str, status: BudgetStatus) -> None:
        """Callback when budget warning fires."""
        self._log_activity(
            ActivityAction.BUDGET_WARNING,
            session_id,
            session_id,
            {"pct": status.worst_pct, "limit": status.worst_limit_name},
        )

    def _on_budget_exceeded(self, session_id: str, status: BudgetStatus) -> None:
        """Callback when budget exceeded fires."""
        self._log_activity(
            ActivityAction.BUDGET_EXCEEDED,
            session_id,
            session_id,
            {"pct": status.worst_pct, "limit": status.worst_limit_name},
        )

    def _sync_budget_limits(self) -> None:
        """Sync budget limits from policy into the enforcer."""
        # Clear existing limits
        self._budget_enforcer._limits.clear()  # pylint: disable=protected-access

        p = self._budget_policy
        if p.max_tokens_per_session is not None:
            self._budget_enforcer.add_limit(
                BudgetLimit(
                    name="tokens_session",
                    max_tokens=p.max_tokens_per_session,
                )
            )
        if p.max_cost_per_session is not None:
            self._budget_enforcer.add_limit(
                BudgetLimit(
                    name="cost_session",
                    max_cost_usd=p.max_cost_per_session,
                )
            )
        if p.max_turns_per_session is not None:
            self._budget_enforcer.add_limit(
                BudgetLimit(
                    name="turns_session",
                    max_turns=p.max_turns_per_session,
                )
            )

    # -------------------------------------------------------------------------
    # Sprint 3: Compaction
    # -------------------------------------------------------------------------

    def should_compact(self) -> bool:
        """Check if any compaction threshold is exceeded."""
        return self._compaction_state.should_compact(self._compaction_policy)

    def run_compaction(self) -> str:
        """
        Run session compaction: generate handoff, archive session, reset state.

        Returns the handoff summary string.
        """
        # Generate handoff summary
        # Note: messages are passed separately by the caller; we generate
        # the handoff text from current state + messages snapshot
        handoff = self._handoff_generator.generate_handoff(
            session_id=self._compaction_state.session_id,
            started_at=self._compaction_state.started_at,
            runs=self._compaction_state.runs,
            input_tokens=self._compaction_state.input_tokens,
            output_tokens=self._compaction_state.output_tokens,
            messages=[],  # Caller passes messages to the new session directly
        )

        # Archive session
        self._session_archiver.archive_session(
            session_id=self._compaction_state.session_id,
            started_at=self._compaction_state.started_at,
            ended_at=datetime.now(timezone.utc),
            runs=self._compaction_state.runs,
            input_tokens=self._compaction_state.input_tokens,
            output_tokens=self._compaction_state.output_tokens,
            handoff_summary=handoff,
            messages=None,
        )

        # Log compaction
        self._log_activity(
            ActivityAction.SESSION_COMPACTED,
            self._compaction_state.session_id,
            self.session_id,
            {
                "runs": self._compaction_state.runs,
                "tokens": self._compaction_state.total_tokens,
            },
        )

        # Reset compaction state for new session
        old_id = self._compaction_state.session_id
        self._compaction_state.reset()
        self._compaction_state.session_id = f"{old_id}_rotated"
        self._compaction_state.started_at = datetime.now(timezone.utc)

        # Reset budget for new session
        self._budget_enforcer.reset(self.session_id)

        return handoff

    def compaction_trigger_reason(self) -> Optional[str]:
        """Human-readable reason why compaction should fire."""
        s = self._compaction_state
        p = self._compaction_policy

        if s.runs >= p.max_session_runs:
            return f"run limit: {s.runs} >= {p.max_session_runs}"
        if s.input_tokens >= p.max_raw_input_tokens:
            return f"input token limit: {s.input_tokens:,} >= {p.max_raw_input_tokens:,}"
        if s.total_tokens >= p.max_total_tokens:
            return f"total token limit: {s.total_tokens:,} >= {p.max_total_tokens:,}"
        if s.session_age_hours >= p.max_session_age_hours:
            return f"session age: {s.session_age_hours:.1f}h >= {p.max_session_age_hours}h"
        return None

    # -------------------------------------------------------------------------
    # Sprint 5: Activity
    # -------------------------------------------------------------------------

    @property
    def activity_logger(self) -> ActivityLogger:
        """Returns the ActivityLogger, lazy-initting on first access."""
        if self._activity_logger is None:
            self._activity_logger = ActivityLogger(session_id=self.session_id)
        return self._activity_logger

    def _log_activity(
        self,
        action: ActivityAction,
        entity_id: str,
        entity_type: str = "agent",
        details: Optional[dict] = None,
    ) -> None:
        """Internal helper to log an activity event if logger is initialized."""
        if self._activity_logger is not None:
            self._activity_logger.log(action, entity_type, entity_id, details)

    # -------------------------------------------------------------------------
    # Sprint 6: Goals
    # -------------------------------------------------------------------------

    def _init_goals(self) -> None:
        """Lazy-init goal hierarchy components."""
        if self._goal_tracker is None:
            self._goal_tracker = GoalTracker()
            self._goal_hierarchy = GoalHierarchy(tracker=self._goal_tracker)
            self._task_linker = TaskLinker(
                tasks_dir=None,
                hierarchy=self._goal_hierarchy,
            )

    def get_goal_context(self, goal_id: str) -> str:
        """Get goal ancestry context string for injection into prompts."""
        self._init_goals()
        return self._goal_hierarchy.get_context_for_goal(goal_id)

    def create_goal(self, title: str, **kwargs) -> Goal:
        """Create a new goal. kwargs passed to GoalTracker.create_goal."""
        self._init_goals()
        goal = self._goal_tracker.create_goal(title=title, **kwargs)
        self._log_activity(
            ActivityAction.SKILL_CREATED,  # closest action
            goal.id,
            "goal",
            {"title": title},
        )
        return goal

    def create_task(self, goal_id: str, title: str, **kwargs) -> Task:
        """Create a new task linked to a goal."""
        self._init_goals()
        task = self._task_linker.create_task(goal_id=goal_id, title=title, **kwargs)
        self._log_activity(
            ActivityAction.TASK_STARTED,
            task.id,
            "task",
            {"goal_id": goal_id, "title": title},
        )
        return task

    def inject_task_context(self, task_id: str, prompt: str) -> str:
        """Inject goal context into a task prompt."""
        self._init_goals()
        return self._task_linker.inject_context_into_prompt(task_id, prompt)

    # -------------------------------------------------------------------------
    # Status
    # -------------------------------------------------------------------------

    def get_status_summary(self) -> str:
        """Return a human-readable one-line status summary."""
        parts = []

        # Nudge state
        parts.append(
            f"turns={self._nudge_state.turns_since_memory}/"
            f"{self._nudge_state.memory_nudge_interval} "
            f"iters={self._nudge_state.iters_since_skill}/"
            f"{self._nudge_state.skill_nudge_interval}"
        )

        # Budget state
        try:
            status = self.check_budget()
            state_str = status.state.value if hasattr(status.state, 'value') else str(status.state)
            parts.append(f"budget={state_str} {status.worst_pct:.0f}%")
        except Exception:
            parts.append("budget=unknown")

        # Compaction
        if self.should_compact():
            reason = self.compaction_trigger_reason()
            parts.append(f"COMPACT({reason})")

        # Iteration budget
        parts.append(f"iter_budget={self._iteration_budget.remaining} remaining")

        return " | ".join(parts)
