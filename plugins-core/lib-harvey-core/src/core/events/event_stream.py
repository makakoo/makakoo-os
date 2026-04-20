#!/usr/bin/env python3
"""
Harvey OS Event Stream — Async event system for agent coordination.

Inspired by pi-mono's EventStream async iterator pattern.
Enables real-time agent-to-agent communication:
  - Agents emit progress events as they work
  - Other agents subscribe and react
  - The orchestrator coordinates without polling

Three layers:
  Event       — A typed, timestamped occurrence
  EventStream — Producer/consumer async queue for one stream
  EventBus    — Global pub/sub across all Harvey agents

Usage:
    from core.events import EventStream, EventBus

    # Single stream (producer/consumer)
    stream = EventStream("agent-task-123")
    stream.push(Event("progress", data={"step": 1, "total": 5}))
    stream.push(Event("progress", data={"step": 2, "total": 5}))
    stream.end(result={"status": "completed"})

    async for event in stream:
        print(event.type, event.data)

    final = await stream.result()

    # Global bus (pub/sub)
    bus = EventBus.instance()
    bus.subscribe("agent.*", callback)
    bus.publish("agent.sniper.trade", data={"symbol": "BTC", "pnl": 42.0})
"""

import asyncio
import fnmatch
import json
import logging
import os
import threading
import time
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, AsyncIterator, Callable, Dict, List, Optional

log = logging.getLogger("harvey.events")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


# ═══════════════════════════════════════════════════════════════
#  Event
# ═══════════════════════════════════════════════════════════════

@dataclass
class Event:
    """A single typed event."""
    type: str                          # e.g. "progress", "error", "completed"
    data: Dict[str, Any] = field(default_factory=dict)
    source: str = ""                   # Agent/module that emitted
    timestamp: float = field(default_factory=time.time)

    def to_dict(self) -> dict:
        return {
            "type": self.type,
            "data": self.data,
            "source": self.source,
            "timestamp": self.timestamp,
            "iso": datetime.fromtimestamp(self.timestamp).isoformat(),
        }

    def __repr__(self):
        return f"Event({self.type}, source={self.source}, data={self.data})"


# ═══════════════════════════════════════════════════════════════
#  EventStream — Async producer/consumer queue
# ═══════════════════════════════════════════════════════════════

class EventStream:
    """
    Single-producer, multi-consumer event stream.

    The producer pushes events and eventually calls end().
    Consumers iterate with `async for event in stream`.
    After end(), consumers can get the final result.

    Thread-safe: producer can push from any thread.
    """

    def __init__(self, stream_id: str = "", source: str = ""):
        self.stream_id = stream_id
        self.source = source
        self._events: List[Event] = []
        self._result: Any = None
        self._ended = False
        self._error: Optional[Exception] = None
        self._lock = threading.Lock()
        self._sync_waiters: List[threading.Event] = []
        # For sync iteration
        self._cursor = 0

    def push(self, event: Event) -> None:
        """Push an event into the stream."""
        if self._ended:
            raise RuntimeError(f"Cannot push to ended stream: {self.stream_id}")
        if not event.source:
            event.source = self.source
        with self._lock:
            self._events.append(event)
            for waiter in self._sync_waiters:
                waiter.set()

    def end(self, result: Any = None) -> None:
        """Signal stream completion with optional result."""
        self._result = result
        self._ended = True
        with self._lock:
            for waiter in self._sync_waiters:
                waiter.set()

    def fail(self, error: Exception) -> None:
        """Signal stream failure."""
        self._error = error
        self._ended = True
        with self._lock:
            for waiter in self._sync_waiters:
                waiter.set()

    @property
    def is_ended(self) -> bool:
        return self._ended

    @property
    def events(self) -> List[Event]:
        """Get all events emitted so far."""
        return list(self._events)

    def get_result(self) -> Any:
        """Get the final result (blocks if not ended)."""
        if self._error:
            raise self._error
        return self._result

    # ── Sync iteration ────────────────────────────────────────

    def poll(self, timeout: float = 1.0) -> List[Event]:
        """
        Poll for new events since last poll.
        Returns empty list if no new events. Non-blocking.
        """
        with self._lock:
            new = self._events[self._cursor:]
            self._cursor = len(self._events)
        return new

    def wait_for_event(self, timeout: float = 5.0) -> Optional[Event]:
        """Block until a new event arrives or timeout."""
        waiter = threading.Event()
        current_len = len(self._events)
        with self._lock:
            self._sync_waiters.append(waiter)
        try:
            waiter.wait(timeout=timeout)
            with self._lock:
                if len(self._events) > current_len:
                    return self._events[current_len]
                return None
        finally:
            with self._lock:
                self._sync_waiters.remove(waiter)

    # ── Convenience ───────────────────────────────────────────

    def progress(self, step: int, total: int, message: str = "") -> None:
        """Emit a progress event."""
        self.push(Event("progress", data={
            "step": step, "total": total, "message": message,
            "pct": round(step / total * 100) if total else 0,
        }))

    def log(self, message: str, level: str = "info") -> None:
        """Emit a log event."""
        self.push(Event("log", data={"message": message, "level": level}))

    def error(self, message: str, details: Any = None) -> None:
        """Emit an error event."""
        self.push(Event("error", data={"message": message, "details": details}))


# ═══════════════════════════════════════════════════════════════
#  EventBus — Global pub/sub for agent coordination
# ═══════════════════════════════════════════════════════════════

class EventBus:
    """
    Global event bus for Harvey OS. Agents publish, others subscribe.

    Topics use dot notation: "agent.sniper.trade", "brain.compile", etc.
    Subscribers can use glob patterns: "agent.*", "brain.*", "*"

    Thread-safe singleton.
    """

    _instance: Optional["EventBus"] = None
    _lock = threading.Lock()

    def __init__(self):
        self._subscribers: Dict[str, List[Callable]] = defaultdict(list)
        self._history: List[Event] = []
        self._max_history = 500
        self._streams: Dict[str, EventStream] = {}

    @classmethod
    def instance(cls) -> "EventBus":
        """Get or create the global event bus."""
        if cls._instance is None:
            with cls._lock:
                if cls._instance is None:
                    cls._instance = cls()
        return cls._instance

    # ── Pub/Sub ───────────────────────────────────────────────

    def subscribe(self, pattern: str, callback: Callable) -> Callable:
        """
        Subscribe to events matching a glob pattern.

        Pattern examples:
          "agent.sniper.*"  — all sniper events
          "brain.*"         — all Brain events
          "*"               — everything

        Returns the callback (for unsubscribe).
        """
        self._subscribers[pattern].append(callback)
        return callback

    def unsubscribe(self, pattern: str, callback: Callable) -> None:
        """Remove a subscription."""
        if pattern in self._subscribers:
            self._subscribers[pattern] = [
                cb for cb in self._subscribers[pattern] if cb != callback
            ]

    def publish(self, topic: str, source: str = "", **data) -> Event:
        """
        Publish an event to all matching subscribers.

        Args:
            topic: Dot-notation topic ("agent.sniper.trade")
            source: Who emitted this
            **data: Event payload
        """
        event = Event(type=topic, data=data, source=source)

        # Store in history
        self._history.append(event)
        if len(self._history) > self._max_history:
            self._history = self._history[-self._max_history:]

        # Notify matching subscribers
        for pattern, callbacks in self._subscribers.items():
            if fnmatch.fnmatch(topic, pattern):
                for cb in callbacks:
                    try:
                        cb(event)
                    except Exception as e:
                        log.error("Event subscriber failed for '%s': %s", topic, e)

        return event

    # ── Stream Management ─────────────────────────────────────

    def create_stream(self, stream_id: str, source: str = "") -> EventStream:
        """Create and register a named event stream."""
        stream = EventStream(stream_id=stream_id, source=source)
        self._streams[stream_id] = stream
        self.publish("stream.created", source=source, stream_id=stream_id)
        return stream

    def get_stream(self, stream_id: str) -> Optional[EventStream]:
        """Get an existing stream by ID."""
        return self._streams.get(stream_id)

    def list_streams(self) -> Dict[str, dict]:
        """List all active streams with status."""
        return {
            sid: {
                "source": s.source,
                "events": len(s.events),
                "ended": s.is_ended,
            }
            for sid, s in self._streams.items()
        }

    # ── History ───────────────────────────────────────────────

    def recent(self, n: int = 20, topic_filter: str = "*") -> List[Event]:
        """Get recent events, optionally filtered by topic pattern."""
        filtered = [
            e for e in self._history
            if fnmatch.fnmatch(e.type, topic_filter)
        ]
        return filtered[-n:]

    def stats(self) -> dict:
        """Get bus statistics."""
        return {
            "subscribers": {p: len(cbs) for p, cbs in self._subscribers.items()},
            "history_size": len(self._history),
            "active_streams": len([s for s in self._streams.values() if not s.is_ended]),
            "total_streams": len(self._streams),
        }
