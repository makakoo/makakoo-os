#!/usr/bin/env python3
"""
Nudge Triggers — Auto-Improver Core

Tracks conversation turns and tool iterations, fires memory/skill review triggers
at configured intervals. Based on Hermes-Agent nudge logic.

Config (from hermes config.yaml):
  memory.nudge_interval: 10  (turns between memory reviews)
  memory.flush_min_turns: 6  (minimum turns before first nudge)
  skills.creation_nudge_interval: 10  (iterations between skill reviews)
"""

from dataclasses import dataclass, field
from typing import Optional


@dataclass
class NudgeState:
    """Tracks nudge state for one Harvey agent session."""
    # Counters
    turns_since_memory: int = 0
    iters_since_skill: int = 0

    # Config (defaults from Hermes)
    memory_nudge_interval: int = 10
    memory_flush_min_turns: int = 6
    skill_nudge_interval: int = 10

    # Internal
    _memory_enabled: bool = True
    _skill_enabled: bool = True

    def should_review_memory(self) -> bool:
        """Returns True if memory review should fire this turn."""
        if not self._memory_enabled:
            return False
        if self.memory_nudge_interval <= 0:
            return False
        if (self.turns_since_memory >= self.memory_nudge_interval
                and self.turns_since_memory >= self.memory_flush_min_turns):
            self.turns_since_memory = 0
            return True
        return False

    def should_review_skills(self) -> bool:
        """Returns True if skill review should fire this iteration."""
        if not self._skill_enabled:
            return False
        if self.skill_nudge_interval <= 0:
            return False
        if self.iters_since_skill >= self.skill_nudge_interval:
            self.iters_since_skill = 0
            return True
        return False

    def on_turn(self) -> None:
        """Called every conversation turn. Increments turn counter."""
        self.turns_since_memory += 1

    def on_iteration(self) -> None:
        """Called after each tool-call iteration. Increments iteration counter."""
        self.iters_since_skill += 1

    def on_memory_used(self) -> None:
        """Called when the agent uses the memory/brain tool. Resets counter."""
        self.turns_since_memory = 0

    def on_skill_used(self) -> None:
        """Called when the agent uses skill_manage. Resets counter."""
        self.iters_since_skill = 0

    def reset(self) -> None:
        """Reset all counters to zero."""
        self.turns_since_memory = 0
        self.iters_since_skill = 0
