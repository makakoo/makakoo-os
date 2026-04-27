"""
⚠️ DUAL-WRITE — Rust equivalent at makakoo-os/makakoo-core/src/event_bus.rs
is now authoritative for new consumers. Both Python and Rust read/write the
same SQLite DB (bus_events table). This Python version remains active because
14+ Python consumers depend on it. New code should prefer the Rust event bus
via the makakoo CLI or makakoo-client crate.

---
PersistentEventBus — SQLite-backed pub/sub with replay + cross-process tail.

Phase 1.5 deliverable. API-compatible with the in-memory EventBus in
core/events/event_stream.py, but every publish is also written to SQLite
with a monotonic sequence number.

Why this exists:

  - In-process subscribers keep the fast-path callback behavior
  - Cross-process subscribers can poll SQLite via poll_since(seq)
  - Crash recovery: replay_from(seq) after a restart
  - Auditability: every event has a persistent record

NOT a replacement for the in-memory EventBus. Existing callsites keep
working. New code opts in by instantiating PersistentEventBus directly.
Phase 2 will migrate callsites gradually.
"""

from __future__ import annotations

import fnmatch
import json
import logging
import os
import sqlite3
import threading
import time
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

log = logging.getLogger("harvey.persistent_event_bus")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
DEFAULT_DB_PATH = os.path.join(HARVEY_HOME, "data", "events.db")


@dataclass
class Event:
    """A persistent event row. Compatible with core.events.event_stream.Event."""

    type: str
    data: Dict[str, Any] = field(default_factory=dict)
    source: str = ""
    timestamp: float = field(default_factory=time.time)
    seq: int = 0  # populated after persist

    def to_dict(self) -> dict:
        return {
            "seq": self.seq,
            "type": self.type,
            "data": self.data,
            "source": self.source,
            "timestamp": self.timestamp,
        }

    def __repr__(self):
        return (
            f"Event(seq={self.seq}, type={self.type}, "
            f"source={self.source}, data={self.data})"
        )


class PersistentEventBus:
    """
    SQLite-backed pub/sub with in-process fast path.

    Thread-safe. Cross-process-safe via SQLite WAL mode.
    """

    def __init__(self, db_path: Optional[str] = None):
        self.db_path = db_path or DEFAULT_DB_PATH
        Path(self.db_path).parent.mkdir(parents=True, exist_ok=True)

        self._db = sqlite3.connect(
            self.db_path,
            check_same_thread=False,
            isolation_level=None,
            timeout=30.0,
        )
        self._db.row_factory = sqlite3.Row
        self._conn_lock = threading.RLock()

        self._subscribers: Dict[str, List[Callable]] = defaultdict(list)
        self._subscribers_lock = threading.RLock()

        self._init_schema()

    def _init_schema(self) -> None:
        with self._conn_lock:
            self._db.execute("PRAGMA journal_mode=WAL")
            self._db.execute("PRAGMA synchronous=NORMAL")
            self._db.executescript(
                """
                CREATE TABLE IF NOT EXISTS events (
                    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
                    topic       TEXT NOT NULL,
                    source      TEXT NOT NULL DEFAULT '',
                    data        TEXT NOT NULL,
                    timestamp   REAL NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_events_topic
                    ON events(topic, seq);
                CREATE INDEX IF NOT EXISTS idx_events_timestamp
                    ON events(timestamp);
                """
            )

    # ─── Publish ─────────────────────────────────────────────────

    def publish(self, topic: str, source: str = "", **data) -> int:
        """
        Publish an event. Returns the monotonic sequence number.

        Persists to SQLite, then fires in-process callbacks. Callbacks run
        synchronously on the calling thread (same semantics as the in-memory
        EventBus).
        """
        ts = time.time()
        data_json = json.dumps(data, default=str)

        with self._conn_lock:
            cursor = self._db.execute(
                "INSERT INTO events (topic, source, data, timestamp) "
                "VALUES (?, ?, ?, ?)",
                (topic, source, data_json, ts),
            )
            seq = cursor.lastrowid or 0

        event = Event(
            type=topic, data=data, source=source, timestamp=ts, seq=seq
        )

        # Fire in-process callbacks
        with self._subscribers_lock:
            patterns = list(self._subscribers.items())

        for pattern, callbacks in patterns:
            if fnmatch.fnmatch(topic, pattern):
                for cb in callbacks:
                    try:
                        cb(event)
                    except Exception as e:
                        log.error(
                            f"[event_bus] subscriber failed for '{topic}' "
                            f"pattern '{pattern}': {e}"
                        )

        return seq

    # ─── Subscribe ───────────────────────────────────────────────

    def subscribe(self, pattern: str, callback: Callable[[Event], None]) -> Callable:
        """Subscribe to events matching a glob pattern."""
        with self._subscribers_lock:
            self._subscribers[pattern].append(callback)
        return callback

    def unsubscribe(
        self, pattern: str, callback: Callable[[Event], None]
    ) -> None:
        with self._subscribers_lock:
            if pattern in self._subscribers:
                self._subscribers[pattern] = [
                    cb for cb in self._subscribers[pattern] if cb != callback
                ]

    # ─── Query / Replay ──────────────────────────────────────────

    def recent(self, n: int = 20, topic_filter: str = "*") -> List[Event]:
        """Most recent events, newest last."""
        with self._conn_lock:
            rows = self._db.execute(
                "SELECT * FROM events ORDER BY seq DESC LIMIT ?",
                (n,),
            ).fetchall()
        events = [self._row_to_event(r) for r in rows]
        events.reverse()  # oldest first

        if topic_filter != "*":
            events = [e for e in events if fnmatch.fnmatch(e.type, topic_filter)]
        return events

    def poll_since(
        self, seq: int, topic_filter: str = "*", limit: int = 100
    ) -> List[Tuple[int, Event]]:
        """
        Fetch events with seq > `seq`. Returns list of (seq, Event) tuples.

        Cross-process subscribers call this in a loop, tracking the last
        seen seq locally.
        """
        with self._conn_lock:
            rows = self._db.execute(
                "SELECT * FROM events WHERE seq > ? ORDER BY seq ASC LIMIT ?",
                (seq, limit),
            ).fetchall()

        out: List[Tuple[int, Event]] = []
        for r in rows:
            ev = self._row_to_event(r)
            if topic_filter == "*" or fnmatch.fnmatch(ev.type, topic_filter):
                out.append((ev.seq, ev))
        return out

    def replay_from(
        self,
        seq: int,
        callback: Callable[[Event], None],
        topic_filter: str = "*",
        batch_size: int = 500,
    ) -> int:
        """
        Replay all events after seq through the callback. Returns count replayed.

        Use for crash recovery: after restart, call with the last-processed
        seq, and the bus will re-deliver everything since then.
        """
        replayed = 0
        cursor_seq = seq
        while True:
            batch = self.poll_since(cursor_seq, topic_filter, batch_size)
            if not batch:
                break
            for next_seq, event in batch:
                try:
                    callback(event)
                    replayed += 1
                except Exception as e:
                    log.error(f"[event_bus] replay callback failed: {e}")
                cursor_seq = next_seq
        return replayed

    def latest_seq(self) -> int:
        with self._conn_lock:
            row = self._db.execute(
                "SELECT COALESCE(MAX(seq), 0) AS latest FROM events"
            ).fetchone()
        return row["latest"] if row else 0

    def count(self) -> int:
        with self._conn_lock:
            row = self._db.execute(
                "SELECT COUNT(*) AS n FROM events"
            ).fetchone()
        return row["n"] if row else 0

    def close(self) -> None:
        with self._conn_lock:
            try:
                self._db.close()
            except Exception:
                pass

    # ─── Internal ────────────────────────────────────────────────

    def _row_to_event(self, row: sqlite3.Row) -> Event:
        return Event(
            type=row["topic"],
            data=json.loads(row["data"] or "{}"),
            source=row["source"] or "",
            timestamp=row["timestamp"],
            seq=row["seq"],
        )


# ─────────────────────────────────────────────────────────────────────
# Module-level singleton
# ─────────────────────────────────────────────────────────────────────

_default_bus: Optional[PersistentEventBus] = None
_default_lock = threading.Lock()


def get_default_bus() -> PersistentEventBus:
    """Returns the module-level singleton PersistentEventBus.

    Honors HARVEY_EVENTS_DB env var if set — used by tests to isolate
    event publishing from the production data/events.db. Production
    callers leave the env var unset and get the default path.

    Callers that need a truly fresh bus per test should call
    shutdown_default_bus() between tests to invalidate the cache.
    """
    global _default_bus
    with _default_lock:
        if _default_bus is None:
            override = os.environ.get("HARVEY_EVENTS_DB")
            _default_bus = PersistentEventBus(db_path=override)
    return _default_bus


def shutdown_default_bus() -> None:
    global _default_bus
    with _default_lock:
        if _default_bus is not None:
            _default_bus.close()
            _default_bus = None


__all__ = [
    "Event",
    "PersistentEventBus",
    "get_default_bus",
    "shutdown_default_bus",
    "DEFAULT_DB_PATH",
]
