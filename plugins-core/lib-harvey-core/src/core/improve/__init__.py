"""
Harvey OS Auto-Improve Core.

Provides autonomous self-improvement mechanisms:
- BackgroundReview: fork agent after conversation to review + save learnings
- DelegateTool: spawn subagents with isolated context and restricted toolsets
"""

from core.improve.background_review import (
    BackgroundReview,
    ReviewResult,
    spawn_background_review,
)
from core.improve.delegate_tool import (
    DelegateResult,
    DelegateTask,
    delegate_task,
    list_active_sessions,
    spawn_delegate,
)
from core.improve.review_triggers import (
    ReviewTrigger,
    ReviewTriggerConfig,
    TriggerType,
    get_default_trigger,
)

__all__ = [
    "BackgroundReview",
    "ReviewResult",
    "spawn_background_review",
    "ReviewTrigger",
    "ReviewTriggerConfig",
    "TriggerType",
    "get_default_trigger",
    "DelegateResult",
    "DelegateTask",
    "delegate_task",
    "spawn_delegate",
    "list_active_sessions",
]
