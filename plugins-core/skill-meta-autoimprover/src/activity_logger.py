#!/usr/bin/env python3
"""
Activity Logger — Sprint 5

Immutable JSONL audit trail. All significant agent actions logged
with actor, action, entity, timestamp. Based on Paperclip's activity-log.ts.

Log file: data/logs/activity/YYYY/MM/YYYY-MM-DD.jsonl
One JSON object per line.
"""

import json
import os
import threading
import uuid
from datetime import datetime, timedelta
from enum import Enum
from pathlib import Path
from typing import Optional, List, Dict, Any, Callable
from queue import Queue, Empty
from dataclasses import dataclass, field, asdict


class ActivityAction(Enum):
    """All trackable agent actions."""
    # Skills
    SKILL_CREATED = "skill.created"
    SKILL_UPDATED = "skill.updated"
    SKILL_DELETED = "skill.deleted"
    SKILL_PATCHED = "skill.patched"

    # Memory
    MEMORY_ADDED = "memory.added"
    MEMORY_UPDATED = "memory.updated"
    MEMORY_REMOVED = "memory.removed"

    # Session
    SESSION_START = "session.start"
    SESSION_END = "session.end"
    SESSION_COMPACTED = "session.compacted"
    SESSION_RESUMED = "session.resumed"

    # Budget
    BUDGET_WARNING = "budget.warning"
    BUDGET_EXCEEDED = "budget.exceeded"
    BUDGET_PAUSED = "budget.paused"
    BUDGET_RESUMED = "budget.resumed"

    # Agent
    AGENT_FORKED = "agent.forked"
    AGENT_TOOL_CALL = "agent.tool_call"
    AGENT_ERROR = "agent.error"

    # Tasks
    TASK_COMPLETED = "task.completed"
    TASK_FAILED = "task.failed"
    TASK_STARTED = "task.started"

    # Improvement
    IMPROVEMENT_MEMORY = "improvement.memory"
    IMPROVEMENT_SKILL = "improvement.skill"


@dataclass
class ActivityEvent:
    """One audit log entry."""
    id: str                    # UUID
    timestamp: str            # ISO 8601
    actor: str                 # "harvey", "review_agent", "user"
    action: str               # ActivityAction.value
    entity_type: str          # "skill", "memory", "session", "budget", "task"
    entity_id: str            # skill name, session_id, etc.
    details: Dict[str, Any]   # Additional structured data
    session_id: str           # Which session this belongs to

    def to_json(self) -> str:
        return json.dumps(asdict(self), ensure_ascii=False)


class ActivityLogger:
    """Thread-safe async activity logger with batch writes."""

    LOG_DIR = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / "data" / "logs" / "activity"

    def __init__(
        self,
        session_id: str,
        actor: str = "harvey",
        flush_interval: float = 2.0,  # seconds
        batch_size: int = 50,
        async_buffer: bool = True,
    ):
        """
        Args:
            session_id: Current session ID for all events
            actor: Who is performing actions (default "harvey")
            flush_interval: Flush buffer to disk every N seconds
            batch_size: Flush when buffer reaches this size
            async_buffer: If True, writes are non-blocking
        """
        self.session_id = session_id
        self.actor = actor
        self._flush_interval = flush_interval
        self._batch_size = batch_size
        self._async = async_buffer

        self._buffer: List[ActivityEvent] = []
        self._lock = threading.Lock()
        self._queue: Optional[Queue] = None
        self._writer_thread: Optional[threading.Thread] = None
        self._stop_event = threading.Event()
        self._last_flush_time = datetime.now()
        self._timer_lock = threading.Lock()

        if self._async:
            self._queue = Queue(maxsize=1000)
            self._writer_thread = threading.Thread(
                target=self._writer_loop, daemon=True, name="activity-logger"
            )
            self._writer_thread.start()

    def log(
        self,
        action: ActivityAction,
        entity_type: str,
        entity_id: str,
        details: Optional[Dict[str, Any]] = None,
        actor: Optional[str] = None,
    ) -> str:
        """Log one activity event. Returns the event ID."""
        event = ActivityEvent(
            id=str(uuid.uuid4()),
            timestamp=datetime.now().isoformat(),
            actor=actor or self.actor,
            action=action.value,
            entity_type=entity_type,
            entity_id=entity_id,
            details=details or {},
            session_id=self.session_id,
        )

        if self._async:
            # Non-blocking: put in queue, wake writer
            try:
                self._queue.put_nowait(event)
            except Exception:
                # Queue full — fall back to sync buffer
                with self._lock:
                    self._buffer.append(event)
            self._check_timer_flush()
        else:
            with self._lock:
                self._buffer.append(event)
            self._check_timer_flush()

        self._check_batch_flush()
        return event.id

    def _check_batch_flush(self) -> None:
        with self._lock:
            if len(self._buffer) >= self._batch_size:
                self._flush_buffer()

    def _check_timer_flush(self) -> None:
        with self._timer_lock:
            if (datetime.now() - self._last_flush_time).total_seconds() >= self._flush_interval:
                self.flush()

    def log_skill_created(self, skill_name: str, category: str, details: Optional[dict] = None):
        return self.log(ActivityAction.SKILL_CREATED, "skill", skill_name,
                        {**(details or {}), "category": category})

    def log_skill_updated(self, skill_name: str, details: Optional[dict] = None):
        return self.log(ActivityAction.SKILL_UPDATED, "skill", skill_name, details)

    def log_skill_deleted(self, skill_name: str, details: Optional[dict] = None):
        return self.log(ActivityAction.SKILL_DELETED, "skill", skill_name, details)

    def log_skill_patched(self, skill_name: str, details: Optional[dict] = None):
        return self.log(ActivityAction.SKILL_PATCHED, "skill", skill_name, details)

    def log_memory_added(self, target: str, entry_preview: str, details: Optional[dict] = None):
        return self.log(ActivityAction.MEMORY_ADDED, "memory", target,
                        {**(details or {}), "preview": entry_preview[:100]})

    def log_memory_updated(self, target: str, entry_preview: str, details: Optional[dict] = None):
        return self.log(ActivityAction.MEMORY_UPDATED, "memory", target,
                        {**(details or {}), "preview": entry_preview[:100]})

    def log_memory_removed(self, target: str, details: Optional[dict] = None):
        return self.log(ActivityAction.MEMORY_REMOVED, "memory", target, details)

    def log_session_start(self, session_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.SESSION_START, "session", session_id, details)

    def log_session_end(self, session_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.SESSION_END, "session", session_id, details)

    def log_session_compacted(self, session_id: str, runs: int, tokens: int, details: Optional[dict] = None):
        return self.log(ActivityAction.SESSION_COMPACTED, "session", session_id,
                        {**(details or {}), "runs": runs, "tokens": tokens})

    def log_session_resumed(self, session_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.SESSION_RESUMED, "session", session_id, details)

    def log_budget_warning(self, session_id: str, pct: float, details: Optional[dict] = None):
        return self.log(ActivityAction.BUDGET_WARNING, "budget", session_id,
                        {**(details or {}), "pct": pct})

    def log_budget_exceeded(self, session_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.BUDGET_EXCEEDED, "budget", session_id, details)

    def log_budget_paused(self, session_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.BUDGET_PAUSED, "budget", session_id, details)

    def log_budget_resumed(self, session_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.BUDGET_RESUMED, "budget", session_id, details)

    def log_agent_forked(self, parent_session_id: str, child_session_id: str,
                         details: Optional[dict] = None):
        return self.log(ActivityAction.AGENT_FORKED, "agent", child_session_id,
                        {**(details or {}), "parent_session": parent_session_id})

    def log_agent_tool_call(self, tool_name: str, details: Optional[dict] = None):
        return self.log(ActivityAction.AGENT_TOOL_CALL, "agent", tool_name, details)

    def log_agent_error(self, error_msg: str, details: Optional[dict] = None):
        return self.log(ActivityAction.AGENT_ERROR, "agent", error_msg, details)

    def log_task_started(self, task_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.TASK_STARTED, "task", task_id, details)

    def log_task_completed(self, task_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.TASK_COMPLETED, "task", task_id, details)

    def log_task_failed(self, task_id: str, details: Optional[dict] = None):
        return self.log(ActivityAction.TASK_FAILED, "task", task_id, details)

    def log_improvement_memory(self, memory_target: str, details: Optional[dict] = None):
        return self.log(ActivityAction.IMPROVEMENT_MEMORY, "improvement", memory_target, details)

    def log_improvement_skill(self, skill_name: str, details: Optional[dict] = None):
        return self.log(ActivityAction.IMPROVEMENT_SKILL, "improvement", skill_name, details)

    def flush(self) -> None:
        """Force flush buffer to disk."""
        if self._async:
            self._flush_queue()
        self._flush_buffer()

    def _flush_queue(self) -> None:
        """Drain async queue into buffer."""
        if not self._queue:
            return
        while True:
            try:
                event = self._queue.get_nowait()
                with self._lock:
                    self._buffer.append(event)
            except Empty:
                break

    def _flush_buffer(self) -> None:
        """Write buffered events to JSONL file. Called under lock."""
        if not self._buffer:
            return

        events_to_write = self._buffer[:]
        self._buffer.clear()
        self._last_flush_time = datetime.now()

        if not events_to_write:
            return

        log_path = self._ensure_log_dir()
        try:
            with open(log_path, "a", encoding="utf-8") as f:
                for event in events_to_write:
                    f.write(event.to_json() + "\n")
        except Exception:
            # On failure, put events back in buffer
            with self._lock:
                self._buffer = events_to_write + self._buffer
            raise

    def _ensure_log_dir(self) -> Path:
        """Get today's log directory, create if needed."""
        now = datetime.now()
        log_dir = self.LOG_DIR / str(now.year) / f"{now.month:02d}"
        log_dir.mkdir(parents=True, exist_ok=True)
        return log_dir / f"{now.year:04d}-{now.month:02d}-{now.day:02d}.jsonl"

    def _writer_loop(self) -> None:
        """Async writer loop running in background thread."""
        while not self._stop_event.is_set():
            try:
                # Collect events from queue with timeout
                events: List[ActivityEvent] = []
                while len(events) < self._batch_size:
                    try:
                        event = self._queue.get(timeout=0.5)
                        events.append(event)
                    except Empty:
                        break

                # Also check flush interval
                if events:
                    time_since_flush = (datetime.now() - self._last_flush_time).total_seconds()
                    if time_since_flush < self._flush_interval and len(events) < self._batch_size:
                        # Put events back and wait more
                        for e in reversed(events):
                            try:
                                self._queue.put_nowait(e)
                            except Exception:
                                pass
                        events.clear()
                        try:
                            event = self._queue.get(timeout=max(0.1, self._flush_interval - time_since_flush))
                            events.append(event)
                        except Empty:
                            pass

                if events:
                    with self._lock:
                        self._buffer.extend(events)
                    self._flush_buffer()

                # Check if we should flush on interval even with no new events
                if not events:
                    time_since_flush = (datetime.now() - self._last_flush_time).total_seconds()
                    if time_since_flush >= self._flush_interval:
                        self._flush_buffer()

            except Exception:
                # Keep writer alive
                pass

    def close(self) -> None:
        """Stop writer thread and flush remaining events."""
        self._stop_event.set()
        if self._writer_thread and self._writer_thread.is_alive():
            self._writer_thread.join(timeout=5.0)
        self._flush_queue()
        self._flush_buffer()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
