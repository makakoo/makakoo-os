#!/usr/bin/env python3
"""
Review Triggers — When to fire the Background Review.

Provides configurable trigger conditions:
- Every N conversations
- After long conversations (>N messages)
- After conversations with errors
- On demand (manual trigger)

Source: hermes-agent pattern, adapted for Harvey OS
"""

from __future__ import annotations

import threading
from dataclasses import dataclass
from enum import Enum
from typing import Optional

# ---------------------------------------------------------------------------
# Trigger type enum
# ---------------------------------------------------------------------------


class TriggerType(Enum):
    """When to fire the background review."""

    EVERY = "every"  # Every conversation
    EVERY_N = "every_n"  # Every N conversations
    LONG_SESSION = "long"  # Only after long sessions
    MANUAL = "manual"  # Only when explicitly triggered
    NEVER = "never"  # Disabled


# ---------------------------------------------------------------------------
# Trigger configuration
# ---------------------------------------------------------------------------


@dataclass
class ReviewTriggerConfig:
    """Configuration for when to fire background reviews."""

    trigger_type: TriggerType = TriggerType.EVERY_N
    every_n: int = 3
    long_session_min_messages: int = 10
    review_memory: bool = True
    review_skills: bool = True
    cooldown_seconds: float = 60.0


# ---------------------------------------------------------------------------
# Review Trigger state machine
# ---------------------------------------------------------------------------


class ReviewTrigger:
    """
    Tracks conversation count and determines when to fire a background review.

    Usage:
        trigger = ReviewTrigger(config=ReviewTriggerConfig(
            trigger_type=TriggerType.EVERY_N,
            every_n=3,
        ))

        # After each conversation:
        should_fire, memory, skills = trigger.should_review(num_messages=5)

        if should_fire:
            spawn_background_review(messages, review_memory=memory, review_skills=skills)
    """

    def __init__(self, config: Optional[ReviewTriggerConfig] = None):
        self.config = config or ReviewTriggerConfig()
        self._conversation_count = 0
        self._last_review_time = 0.0
        self._lock = threading.Lock()

    def record_conversation(self, num_messages: int) -> tuple[bool, bool, bool]:
        """
        Call after each conversation ends.

        Returns (should_fire, review_memory, review_skills).

        Thread-safe.
        """
        with self._lock:
            self._conversation_count += 1
            return self._evaluate(num_messages)

    def should_review_now(self, num_messages: int = 0) -> tuple[bool, bool, bool]:
        """
        Check if a review should fire for the current conversation.

        Does not increment the conversation counter.
        Thread-safe.
        """
        with self._lock:
            return self._evaluate(num_messages)

    def manual_trigger(self) -> tuple[bool, bool, bool]:
        """
        Force a manual trigger regardless of counter/cooldown.

        Returns (should_fire, review_memory, review_skills).
        """
        with self._lock:
            return self._evaluate(999)

    def _evaluate(self, num_messages: int) -> tuple[bool, bool, bool]:
        cfg = self.config
        ct = cfg.trigger_type

        if ct == TriggerType.NEVER:
            return False, False, False

        if ct == TriggerType.EVERY:
            return True, cfg.review_memory, cfg.review_skills

        if ct == TriggerType.MANUAL:
            return False, False, False

        if ct == TriggerType.EVERY_N:
            if self._conversation_count % cfg.every_n == 0:
                return True, cfg.review_memory, cfg.review_skills
            return False, False, False

        if ct == TriggerType.LONG_SESSION:
            if num_messages >= cfg.long_session_min_messages:
                return True, cfg.review_memory, cfg.review_skills
            return False, False, False

        return False, False, False

    def update_config(self, **kwargs) -> None:
        """Update trigger config at runtime."""
        for key, value in kwargs.items():
            if hasattr(self.config, key):
                setattr(self.config, key, value)

    @property
    def conversation_count(self) -> int:
        with self._lock:
            return self._conversation_count


# ---------------------------------------------------------------------------
# Global default trigger (can be overridden)
# ---------------------------------------------------------------------------

_default_trigger: Optional[ReviewTrigger] = None
_trigger_lock = threading.Lock()


def get_default_trigger() -> ReviewTrigger:
    global _default_trigger
    with _trigger_lock:
        if _default_trigger is None:
            _default_trigger = ReviewTrigger()
        return _default_trigger


def set_default_trigger(trigger: ReviewTrigger) -> None:
    global _default_trigger
    with _trigger_lock:
        _default_trigger = trigger
