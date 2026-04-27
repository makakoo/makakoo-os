"""
audit_log.py — Phase 4 deliverable

SQLite-backed audit trail for every security-relevant action in the
swarm. Records tool invocations, access denials, agent registrations,
and arbitrary "security events" with full context (workflow_id,
step_id, agent, tool, timestamp, outcome).

Designed to be called from:
  - AgentAccessControl.check (denials)
  - Subagent.tool() (successful tool calls)
  - AgentCoordinator.register (lifecycle events)
  - FailureRecovery (breaker state transitions)

Schema:
    audit_events(
      id INTEGER PRIMARY KEY,
      ts REAL,
      kind TEXT,        -- tool_call | access_denied | lifecycle | security
      agent TEXT,
      tool TEXT,
      workflow_id TEXT,
      step_id TEXT,
      outcome TEXT,     -- ok | denied | error
      detail TEXT       -- JSON blob
    )

Exposed:
  AuditEvent
  AuditLog
"""

from __future__ import annotations

import json
import logging
import os
import sqlite3
import threading
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional

log = logging.getLogger("harvey.audit")


@dataclass
class AuditEvent:
    kind: str                       # tool_call | access_denied | lifecycle | security
    agent: str = ""
    tool: str = ""
    workflow_id: str = ""
    step_id: str = ""
    outcome: str = "ok"             # ok | denied | error
    detail: Dict[str, Any] = field(default_factory=dict)
    ts: float = field(default_factory=time.time)


class AuditLog:
    """
    Append-only audit log backed by SQLite in WAL mode. Safe for
    concurrent writers across threads.

    Open once per process (or pass `db_path=":memory:"` in tests) and
    call `.record(...)` from anywhere.
    """

    def __init__(self, db_path: Optional[str] = None):
        if db_path is None:
            harvey_home = os.environ.get(
                "HARVEY_HOME", str(Path.home() / "HARVEY")
            )
            db_path = os.path.join(harvey_home, "data", "audit.db")
            Path(db_path).parent.mkdir(parents=True, exist_ok=True)

        self.db_path = db_path
        self._lock = threading.RLock()
        self.db = sqlite3.connect(
            db_path, check_same_thread=False, isolation_level=None
        )
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=NORMAL")
        self._init_schema()

    def _init_schema(self) -> None:
        self.db.execute("""
            CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts REAL NOT NULL,
                kind TEXT NOT NULL,
                agent TEXT DEFAULT '',
                tool TEXT DEFAULT '',
                workflow_id TEXT DEFAULT '',
                step_id TEXT DEFAULT '',
                outcome TEXT DEFAULT 'ok',
                detail TEXT DEFAULT '{}'
            )
        """)
        self.db.execute(
            "CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_events(ts DESC)"
        )
        self.db.execute(
            "CREATE INDEX IF NOT EXISTS idx_audit_kind ON audit_events(kind, ts DESC)"
        )
        self.db.execute(
            "CREATE INDEX IF NOT EXISTS idx_audit_agent ON audit_events(agent, ts DESC)"
        )
        self.db.execute(
            "CREATE INDEX IF NOT EXISTS idx_audit_workflow "
            "ON audit_events(workflow_id, ts DESC)"
        )

    # ── Writes ──

    def record(self, event: AuditEvent) -> int:
        """Persist an event. Returns the row id."""
        with self._lock:
            cur = self.db.execute(
                """INSERT INTO audit_events
                   (ts, kind, agent, tool, workflow_id, step_id, outcome, detail)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    event.ts,
                    event.kind,
                    event.agent,
                    event.tool,
                    event.workflow_id,
                    event.step_id,
                    event.outcome,
                    json.dumps(event.detail, default=str),
                ),
            )
            return cur.lastrowid

    def record_tool_call(
        self,
        agent: str,
        tool: str,
        workflow_id: str = "",
        step_id: str = "",
        outcome: str = "ok",
        **detail: Any,
    ) -> int:
        return self.record(AuditEvent(
            kind="tool_call",
            agent=agent, tool=tool,
            workflow_id=workflow_id, step_id=step_id,
            outcome=outcome, detail=detail,
        ))

    def record_denial(
        self, agent: str, tool: str, reason: str, **detail: Any
    ) -> int:
        d = {"reason": reason, **detail}
        return self.record(AuditEvent(
            kind="access_denied",
            agent=agent, tool=tool,
            outcome="denied", detail=d,
        ))

    def record_lifecycle(
        self, agent: str, action: str, **detail: Any
    ) -> int:
        d = {"action": action, **detail}
        return self.record(AuditEvent(
            kind="lifecycle", agent=agent, detail=d,
        ))

    # ── Reads ──

    def recent(
        self, n: int = 100, kind: Optional[str] = None
    ) -> List[AuditEvent]:
        with self._lock:
            if kind:
                rows = self.db.execute(
                    "SELECT * FROM audit_events WHERE kind = ? "
                    "ORDER BY ts DESC LIMIT ?",
                    (kind, n),
                ).fetchall()
            else:
                rows = self.db.execute(
                    "SELECT * FROM audit_events ORDER BY ts DESC LIMIT ?",
                    (n,),
                ).fetchall()
            return [self._row_to_event(r) for r in rows]

    def by_agent(self, agent: str, n: int = 100) -> List[AuditEvent]:
        with self._lock:
            rows = self.db.execute(
                "SELECT * FROM audit_events WHERE agent = ? "
                "ORDER BY ts DESC LIMIT ?",
                (agent, n),
            ).fetchall()
            return [self._row_to_event(r) for r in rows]

    def by_workflow(self, workflow_id: str, n: int = 500) -> List[AuditEvent]:
        with self._lock:
            rows = self.db.execute(
                "SELECT * FROM audit_events WHERE workflow_id = ? "
                "ORDER BY ts ASC LIMIT ?",
                (workflow_id, n),
            ).fetchall()
            return [self._row_to_event(r) for r in rows]

    def count(self, kind: Optional[str] = None) -> int:
        with self._lock:
            if kind:
                row = self.db.execute(
                    "SELECT COUNT(*) FROM audit_events WHERE kind = ?",
                    (kind,),
                ).fetchone()
            else:
                row = self.db.execute(
                    "SELECT COUNT(*) FROM audit_events"
                ).fetchone()
            return int(row[0])

    def status(self) -> Dict[str, Any]:
        with self._lock:
            kinds = self.db.execute(
                "SELECT kind, COUNT(*) FROM audit_events GROUP BY kind"
            ).fetchall()
            return {
                "db_path": self.db_path,
                "total_events": self.count(),
                "by_kind": {row[0]: row[1] for row in kinds},
            }

    def close(self) -> None:
        with self._lock:
            try:
                self.db.close()
            except Exception:
                pass

    # ── Helpers ──

    @staticmethod
    def _row_to_event(row: sqlite3.Row) -> AuditEvent:
        return AuditEvent(
            kind=row["kind"],
            agent=row["agent"] or "",
            tool=row["tool"] or "",
            workflow_id=row["workflow_id"] or "",
            step_id=row["step_id"] or "",
            outcome=row["outcome"] or "ok",
            detail=json.loads(row["detail"] or "{}"),
            ts=row["ts"],
        )


# ── Module-level singleton ──

_default_audit_log: Optional[AuditLog] = None


def get_default_audit_log() -> Optional[AuditLog]:
    """
    Get the singleton AuditLog. Returns None if not explicitly configured —
    callers should treat None as "auditing disabled" and no-op.

    This is opt-in on purpose: we don't want every test run to spawn a
    real SQLite file in HARVEY_HOME. Call `set_default_audit_log(AuditLog(...))`
    at boot time to enable.
    """
    return _default_audit_log


def set_default_audit_log(log: Optional[AuditLog]) -> None:
    """Install or clear the singleton AuditLog."""
    global _default_audit_log
    _default_audit_log = log


__all__ = [
    "AuditEvent",
    "AuditLog",
    "get_default_audit_log",
    "set_default_audit_log",
]
