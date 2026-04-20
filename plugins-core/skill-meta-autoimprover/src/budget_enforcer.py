#!/usr/bin/env python3
"""
Budget Enforcer — Sprint 4

Enforces token/monetary budgets. Warns at 80%, hard-stops at 100%.
Based on Paperclip's budget enforcement model.

States:
  - OK: under 80% of budget
  - WARNING: 80-99% of budget
  - EXCEEDED: at or over budget (hard stop)
  - PAUSED: manually paused by user
"""

from enum import Enum
from dataclasses import dataclass, field
from typing import Optional, Callable, List, Dict
import threading

from .budget_tracker import BudgetTracker, TokenCounter


class BudgetState(Enum):
    OK = "ok"
    WARNING = "warning"   # 80-99% used
    EXCEEDED = "exceeded" # at or over budget
    PAUSED = "paused"


@dataclass
class BudgetLimit:
    """A single budget limit."""
    name: str
    max_tokens: Optional[int] = None      # Token limit
    max_cost_usd: Optional[float] = None  # Dollar limit
    max_turns: Optional[int] = None        # Turn limit
    enabled: bool = True


@dataclass
class BudgetStatus:
    """Current budget status for one limit."""
    limit: BudgetLimit
    used_tokens: int = 0
    used_cost_usd: float = 0.0
    used_turns: int = 0
    state: BudgetState = BudgetState.OK
    warning_emitted: bool = False
    worst_pct: float = 0.0           # Computed worst % across all active limits
    worst_limit_name: str = ""       # Name of the limit driving worst_pct

    @property
    def token_pct(self) -> float:
        """Percentage of token budget used."""
        if not self.limit.max_tokens:
            return 0.0
        return min(100.0, self.used_tokens / self.limit.max_tokens * 100)

    @property
    def cost_pct(self) -> float:
        """Percentage of cost budget used."""
        if not self.limit.max_cost_usd:
            return 0.0
        return min(100.0, self.used_cost_usd / self.limit.max_cost_usd * 100)

    @property
    def turns_pct(self) -> float:
        """Percentage of turn budget used."""
        if not self.limit.max_turns:
            return 0.0
        return min(100.0, self.used_turns / self.limit.max_turns * 100)


class BudgetEnforcer:
    """Enforces budget limits with warning/hard-stop behavior."""

    def __init__(
        self,
        tracker: BudgetTracker,
        limits: Optional[List[BudgetLimit]] = None,
        warning_pct: float = 80.0,
        on_warning: Optional[Callable[[str, BudgetStatus], None]] = None,
        on_exceeded: Optional[Callable[[str, BudgetStatus], None]] = None,
    ):
        """
        Args:
            tracker: BudgetTracker instance
            limits: List of BudgetLimit definitions
            warning_pct: Warn when this % of any limit is reached
            on_warning: Callback(session_id, status) when warning fires
            on_exceeded: Callback(session_id, status) when hard stop fires
        """
        self._tracker = tracker
        self._limits = limits or []
        self._warning_pct = warning_pct
        self._on_warning = on_warning
        self._on_exceeded = on_exceeded
        self._paused_sessions: set = set()
        self._lock = threading.Lock()
        self._status_cache: Dict[str, BudgetStatus] = {}
        # Track turns per session (not exposed by BudgetTracker)
        self._turn_counts: Dict[str, int] = {}

    def _get_turns(self, session_id: str) -> int:
        """Get turn count for a session, defaulting to 0."""
        return self._turn_counts.get(session_id, 0)

    def _increment_turns(self, session_id: str) -> None:
        """Increment turn count for a session."""
        with self._lock:
            self._turn_counts[session_id] = self._turn_counts.get(session_id, 0) + 1

    def check(self, session_id: str) -> BudgetStatus:
        """Check current budget status for a session.

        Returns BudgetStatus. If state is EXCEEDED, the agent should STOP.
        """
        with self._lock:
            # Get token counter from tracker
            counter = self._tracker.get_session(session_id)
            cost = self._tracker.get_session_cost(session_id)
            turns = self._get_turns(session_id)

            # Build per-limit status and find worst
            worst_state = BudgetState.OK
            worst_pct = 0.0
            worst_limit_name = ""
            previous_warning_emitted = False

            # Check existing cached status for warning_emitted flag
            cached = self._status_cache.get(session_id)
            if cached:
                previous_warning_emitted = cached.warning_emitted

            for limit in self._limits:
                if not limit.enabled:
                    continue

                used_tokens = counter.total_tokens
                used_cost_usd = cost
                used_turns = turns

                # Determine worst percentage for this limit across its active dimensions
                pct_token = (used_tokens / limit.max_tokens * 100) if limit.max_tokens else 0.0
                pct_cost = (used_cost_usd / limit.max_cost_usd * 100) if limit.max_cost_usd else 0.0
                pct_turns = (used_turns / limit.max_turns * 100) if limit.max_turns else 0.0
                limit_pct = max(pct_token, pct_cost, pct_turns)

                if limit_pct >= 100.0:
                    worst_state = BudgetState.EXCEEDED
                    if limit_pct >= worst_pct:
                        worst_pct = limit_pct
                        worst_limit_name = limit.name
                elif limit_pct >= self._warning_pct and worst_state != BudgetState.EXCEEDED:
                    worst_state = BudgetState.WARNING
                    if limit_pct >= worst_pct:
                        worst_pct = limit_pct
                        worst_limit_name = limit.name
                else:
                    if limit_pct >= worst_pct:
                        worst_pct = limit_pct
                        worst_limit_name = limit.name

            # Check if paused
            if session_id in self._paused_sessions:
                worst_state = BudgetState.PAUSED

            # Determine if this is a fresh warning (for callback)
            warning_fresh = (
                worst_state == BudgetState.WARNING
                and not previous_warning_emitted
            )
            exceeded_fresh = (
                worst_state == BudgetState.EXCEEDED
                and (cached is None or cached.state != BudgetState.EXCEEDED)
            )

            status = BudgetStatus(
                limit=BudgetLimit(name="__aggregate__"),
                used_tokens=counter.total_tokens,
                used_cost_usd=cost,
                used_turns=turns,
                state=worst_state,
                warning_emitted=worst_state == BudgetState.WARNING,
                worst_pct=worst_pct,
                worst_limit_name=worst_limit_name,
            )
            self._status_cache[session_id] = status

        # Fire callbacks outside the lock to avoid deadlocks
        if warning_fresh and self._on_warning:
            self._on_warning(session_id, status)
        if exceeded_fresh and self._on_exceeded:
            self._on_exceeded(session_id, status)

        return status

    def can_continue(self, session_id: str) -> bool:
        """Returns True if session is within budget and can continue."""
        status = self.check(session_id)
        return status.state in (BudgetState.OK, BudgetState.WARNING) and not self.is_paused(session_id)

    def should_stop(self, session_id: str) -> bool:
        """Returns True if session has exceeded budget."""
        status = self.check(session_id)
        return status.state == BudgetState.EXCEEDED or self.is_paused(session_id)

    def pause(self, session_id: str) -> None:
        """Manually pause a session (e.g., user override)."""
        with self._lock:
            self._paused_sessions.add(session_id)

    def resume(self, session_id: str) -> None:
        """Resume a paused session."""
        with self._lock:
            self._paused_sessions.discard(session_id)

    def is_paused(self, session_id: str) -> bool:
        """Check if session is manually paused."""
        with self._lock:
            return session_id in self._paused_sessions

    def add_limit(self, limit: BudgetLimit) -> None:
        """Add a new budget limit."""
        with self._lock:
            self._limits.append(limit)

    def get_status(self, session_id: str) -> Optional[BudgetStatus]:
        """Get cached status for a session."""
        with self._lock:
            return self._status_cache.get(session_id)

    def reset(self, session_id: str) -> None:
        """Reset budget for a session (start fresh)."""
        with self._lock:
            self._tracker.reset_session(session_id)
            self._turn_counts.pop(session_id, None)
            self._status_cache.pop(session_id, None)
            self._paused_sessions.discard(session_id)
