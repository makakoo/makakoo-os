"""
Harvey OS — Telemetry Events

~60 event types for observability.
Matches Claude Code's analytics patterns.

Event categories:
- Session: SESSION_START, SESSION_END
- Agent: AGENT_SPAWN, AGENT_DIED, AGENT_RESTART, AGENT_MESSAGE_SENT, AGENT_MESSAGE_RECEIVED
- Tool: TOOL_CALLED, TOOL_RESULT, TOOL_ERROR, TOOL_BLOCKED
- Memory: MEMORY_AUTO_EXTRACT, MEMORY_SELECTED, MEMORY_PROMOTED
- Settings: SETTINGS_CHANGED, SETTINGS_MIGRATION
- MCP: MCP_CONNECT, MCP_DISCONNECT, MCP_AUTH_REQUIRED, MCP_ERROR
- Skill: SKILL_INVOKED, SKILL_LOADED, SKILL_ERROR
- Cost: COST_TRACKED
- Startup: STARTUP_PHASE, STARTUP_COMPLETE

Path: plugins-core/lib-harvey-core/src/core/telemetry/events.py
"""

from __future__ import annotations

import os
import json
import time
import threading
import uuid
from pathlib import Path
from dataclasses import dataclass, field, asdict
from typing import Any
from enum import Enum


class EventType(Enum):
    """
    All telemetry event types.

    ~60 event types matching Claude Code's analytics.
    """

    # Session events
    SESSION_START = "harvey.session.started"
    SESSION_END = "harvey.session.ended"
    SESSION_ERROR = "harvey.session.error"

    # Agent lifecycle events
    AGENT_SPAWN = "harvey.agent.spawn"
    AGENT_DIED = "harvey.agent.died"
    AGENT_RESTART = "harvey.agent.restart"
    AGENT_PAUSED = "harvey.agent.paused"
    AGENT_RESUMED = "harvey.agent.resumed"
    AGENT_MESSAGE_SENT = "harvey.agent.message.sent"
    AGENT_MESSAGE_RECEIVED = "harvey.agent.message.received"
    AGENT_KILLED = "harvey.agent.killed"

    # Tool events
    TOOL_CALLED = "harvey.tool.called"
    TOOL_RESULT = "harvey.tool.result"
    TOOL_ERROR = "harvey.tool.error"
    TOOL_BLOCKED = "harvey.tool.blocked"
    TOOL_DURATION = "harvey.tool.duration"

    # Memory events
    MEMORY_AUTO_EXTRACT = "harvey.memory.auto_extract"
    MEMORY_SELECTED = "harvey.memory.selected"
    MEMORY_PROMOTED = "harvey.memory.promoted"
    MEMORY_SYNCED = "harvey.memory.synced"
    MEMORY_QUERY = "harvey.memory.query"

    # Settings events
    SETTINGS_CHANGED = "harvey.settings.changed"
    SETTINGS_MIGRATION = "harvey.settings.migration"
    SETTINGS_LOADED = "harvey.settings.loaded"

    # MCP events
    MCP_CONNECT = "harvey.mcp.connect"
    MCP_DISCONNECT = "harvey.mcp.disconnect"
    MCP_AUTH_REQUIRED = "harvey.mcp.auth_required"
    MCP_ERROR = "harvey.mcp.error"
    MCP_TOOL_CALL = "harvey.mcp.tool_call"

    # Skill events
    SKILL_INVOKED = "harvey.skill.invoked"
    SKILL_LOADED = "harvey.skill.loaded"
    SKILL_ERROR = "harvey.skill.error"
    SKILL_DISCOVERED = "harvey.skill.discovered"

    # Cost tracking
    COST_TRACKED = "harvey.cost.tracked"
    TOKEN_USAGE = "harvey.token.usage"

    # Startup events
    STARTUP_PHASE = "harvey.startup.phase"
    STARTUP_COMPLETE = "harvey.startup.complete"
    BOOTSTRAP_START = "harvey.bootstrap.start"
    BOOTSTRAP_COMPLETE = "harvey.bootstrap.complete"
    MIGRATION_RUN = "harvey.migration.run"

    # Health & monitoring
    HEALTH_CHECK = "harvey.health.check"
    HEALTH_RESTART = "harvey.health.restart"
    WATCHDOG_ALERT = "harvey.watchdog.alert"

    # Brain events
    BRAIN_QUERY = "harvey.brain.query"
    BRAIN_INDEX = "harvey.brain.index"
    BRAIN_UPDATE = "harvey.brain.update"

    # Feature flag events
    FLAG_ENABLED = "harvey.flag.enabled"
    FLAG_DISABLED = "harvey.flag.disabled"
    FLAG_CHECK = "harvey.flag.check"

    # Auth events
    AUTH_SUCCESS = "harvey.auth.success"
    AUTH_FAILURE = "harvey.auth.failure"
    AUTH_TOKEN_REFRESH = "harvey.auth.token_refresh"

    # Git events
    GIT_OPERATION = "harvey.git.operation"
    GIT_HOOK_RUN = "harvey.git.hook_run"

    # Index events
    INDEX_REBUILD = "harvey.index.rebuild"
    INDEX_QUERY = "harvey.index.query"
    LRU_CACHE_HIT = "harvey.lru.cache_hit"
    LRU_CACHE_MISS = "harvey.lru.cache_miss"

    # User interaction
    USER_PROMPT = "harvey.user.prompt"
    USER_RESPONSE = "harvey.user.response"
    TRUST_DIALOG_SHOWN = "harvey.trust_dialog.shown"
    TRUST_DIALOG_ACCEPTED = "harvey.trust_dialog.accepted"

    # Error events
    ERROR_GENERAL = "harvey.error.general"
    ERROR_RECOVERY = "harvey.error.recovery"

    # Custom events (user-defined)
    CUSTOM_EVENT = "harvey.custom.event"


@dataclass
class Event:
    """
    A telemetry event.

    All events include timing, session context, and optional metadata.
    """

    event: str
    timestamp: float = field(default_factory=time.time)
    session_id: str | None = None
    agent_id: str | None = None
    user_id: str | None = None
    duration_ms: float | None = None
    error: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> Event:
        """Create Event from dictionary."""
        return cls(**data)


class TelemetryEmitter:
    """
    Thread-safe telemetry event emitter.

    Features:
    - Thread-safe queueing
    - Batch flushing to disk
    - Session tracking
    - JSONL output format

    Output: ~/.harvey/data/logs/telemetry/{session_id}.jsonl
    """

    def __init__(
        self,
        telemetry_dir: Path | None = None,
        session_id: str | None = None,
        flush_interval_sec: float = 5.0,
        batch_size: int = 100,
    ):
        """
        Initialize TelemetryEmitter.

        Args:
            telemetry_dir: Directory for telemetry files
            session_id: Session identifier (auto-generated if not provided)
            flush_interval_sec: How often to flush queue to disk
            batch_size: Max events per flush
        """
        Harvey_home = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
        default_dir = Path(Harvey_home) / ".harvey" / "data" / "logs" / "telemetry"

        self.telemetry_dir = telemetry_dir or default_dir
        self.session_id = session_id or str(uuid.uuid4())[:8]
        self.flush_interval_sec = flush_interval_sec
        self.batch_size = batch_size

        self._queue: list[Event] = []
        self._lock = threading.RLock()
        self._closed = False
        self._session_start = time.time()

        # Ensure directory exists
        self.telemetry_dir.mkdir(parents=True, exist_ok=True)

        # Start background flusher
        self._flusher_thread = threading.Thread(target=self._flusher_loop, daemon=True)
        self._flusher_thread.start()

    @property
    def session_file(self) -> Path:
        """Get the session telemetry file path."""
        return self.telemetry_dir / f"{self.session_id}.jsonl"

    def emit(
        self,
        event_type: EventType | str,
        metadata: dict | None = None,
        agent_id: str | None = None,
        user_id: str | None = None,
        duration_ms: float | None = None,
        error: str | None = None,
        **kwargs,
    ) -> None:
        """
        Emit a telemetry event.

        Args:
            event_type: EventType enum or string event name
            metadata: Additional event metadata
            agent_id: Agent identifier
            user_id: User identifier
            duration_ms: Operation duration in milliseconds
            error: Error message if applicable
            **kwargs: Additional metadata fields
        """
        if self._closed:
            return

        # Convert EventType to string
        if isinstance(event_type, EventType):
            event_name = event_type.value
        else:
            event_name = event_type

        event = Event(
            event=event_name,
            timestamp=time.time(),
            session_id=self.session_id,
            agent_id=agent_id,
            user_id=user_id,
            duration_ms=duration_ms,
            error=error,
            metadata={**(metadata or {}), **kwargs},
        )

        with self._lock:
            self._queue.append(event)

            # Auto-flush if batch size reached
            if len(self._queue) >= self.batch_size:
                self._flush()

    def _flush(self) -> None:
        """Flush queued events to disk."""
        if not self._queue:
            return

        events_to_write = self._queue.copy()
        self._queue.clear()

        try:
            with open(self.session_file, "a") as f:
                for event in events_to_write:
                    f.write(json.dumps(event.to_dict()) + "\n")
        except OSError:
            # Put events back in queue if write fails
            with self._lock:
                self._queue = events_to_write + self._queue

    def _flusher_loop(self) -> None:
        """Background thread that periodically flushes events."""
        while not self._closed:
            time.sleep(self.flush_interval_sec)
            with self._lock:
                self._flush()

    def flush(self) -> None:
        """Manually flush all queued events."""
        with self._lock:
            self._flush()

    def close(self) -> None:
        """Close the emitter and flush remaining events."""
        self._closed = True
        self.flush()

    def __enter__(self) -> TelemetryEmitter:
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()


# Module-level singleton
_emitter: TelemetryEmitter | None = None


def get_emitter() -> TelemetryEmitter:
    """Get the global TelemetryEmitter instance."""
    global _emitter
    if _emitter is None:
        _emitter = TelemetryEmitter()
    return _emitter


def emit_event(
    event_type: EventType | str, metadata: dict | None = None, **kwargs
) -> None:
    """Convenience function to emit an event."""
    get_emitter().emit(event_type, metadata, **kwargs)
