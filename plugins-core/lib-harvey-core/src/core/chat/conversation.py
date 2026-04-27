"""
Conversation State Management — tracks per-user context and active tasks.

Determines whether a new message is:
1. Continuing an active task (provide feedback/clarification)
2. Responding to a pending question from Harvey
3. Starting a new conversation/task

This enables multi-turn agentic flows across multiple Telegram messages.
"""

import logging
from typing import Optional

from core.chat.task_queue import Task, TaskQueue

log = logging.getLogger("harveychat.conversation")


class ConversationState:
    """Per-user conversation state."""

    def __init__(self, channel: str, user_id: str, task_queue: TaskQueue):
        self.channel = channel
        self.user_id = user_id
        self.task_queue = task_queue

    def get_active_task(self) -> Optional[Task]:
        """Get user's current active task (if any)."""
        return self.task_queue.get_active_task(self.channel, self.user_id)

    def has_pending_task(self) -> bool:
        """User has a task awaiting input."""
        task = self.get_active_task()
        return task is not None and task.awaiting_input_prompt

    def start_new_task(self, goal: str) -> Task:
        """Start a new task for this user."""
        return self.task_queue.create_task(self.channel, self.user_id, goal)

    def get_task_prompt(self) -> Optional[str]:
        """Get the current awaiting_input prompt (if task is waiting for user)."""
        task = self.get_active_task()
        if task and task.awaiting_input_prompt:
            return task.awaiting_input_prompt
        return None

    def classify_message(self, text: str) -> str:
        """
        Determine message type:
        - "continue_task": User is responding to a pending question
        - "start_new": Fresh message, starting new work
        """
        task = self.get_active_task()

        # If no active task, always new
        if not task:
            return "start_new"

        # If task is awaiting input, this message is responding to it
        if task.awaiting_input_prompt:
            return "continue_task"

        # If task is still running (not awaiting input), could be:
        # - User interrupting (new message)
        # - User waiting for result (should be rare, async task should resolve quickly)
        # For now, treat as "interrupt" = start new task
        return "start_new"


class ConversationManager:
    """Manages all active conversations."""

    def __init__(self, task_queue: TaskQueue):
        self.task_queue = task_queue
        self._conversations = {}  # (channel, user_id) -> ConversationState

    def get_or_create(self, channel: str, user_id: str) -> ConversationState:
        """Get or create conversation state for a user."""
        key = (channel, user_id)
        if key not in self._conversations:
            self._conversations[key] = ConversationState(channel, user_id, self.task_queue)
        return self._conversations[key]
