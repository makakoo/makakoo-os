"""
Agent Discovery Registry Store (SQLite)
"""

import sqlite3
import json
import threading
from datetime import datetime, timezone, timedelta
from contextlib import contextmanager
from typing import Optional, List

from .models import AgentRecord


class RegistryStore:
    """Thread-safe SQLite-backed agent registry."""

    DEFAULT_TTL = 300  # seconds

    def __init__(self, db_path: str = None):
        if db_path is None:
            import os
            _harvey_home = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
            db_path = os.path.join(_harvey_home, "data", "agent_discovery", "registry.db")
        self.db_path = db_path
        self._lock = threading.RLock()
        self._init_db()

    def _init_db(self) -> None:
        with self._lock:
            conn = sqlite3.connect(self.db_path)
            conn.execute("""
                CREATE TABLE IF NOT EXISTS agents (
                    agent_id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    capabilities TEXT NOT NULL DEFAULT '[]',
                    skills TEXT NOT NULL DEFAULT '[]',
                    endpoint TEXT NOT NULL DEFAULT '',
                    metadata TEXT NOT NULL DEFAULT '{}',
                    registered_at TEXT NOT NULL,
                    lease_expires_at TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'active'
                )
            """)
            conn.execute("CREATE INDEX IF NOT EXISTS idx_lease ON agents(lease_expires_at)")
            conn.commit()
            conn.close()

    @contextmanager
    def _conn(self):
        conn = sqlite3.connect(self.db_path)
        conn.row_factory = sqlite3.Row
        try:
            yield conn
        finally:
            conn.close()

    def register(self, record: AgentRecord, ttl_seconds: int = DEFAULT_TTL) -> bool:
        """
        Register or re-register an agent.
        Returns True if successful.
        """
        if not record.lease_expires_at:
            record.refresh(ttl_seconds)

        with self._lock:
            with self._conn() as conn:
                cursor = conn.cursor()
                cursor.execute("""
                    INSERT INTO agents (agent_id, name, capabilities, skills, endpoint, metadata, registered_at, lease_expires_at, status)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(agent_id) DO UPDATE SET
                        name = excluded.name,
                        capabilities = excluded.capabilities,
                        skills = excluded.skills,
                        endpoint = excluded.endpoint,
                        metadata = excluded.metadata,
                        lease_expires_at = excluded.lease_expires_at,
                        status = excluded.status
                """, (
                    record.agent_id,
                    record.name,
                    json.dumps(record.capabilities),
                    json.dumps(record.skills),
                    record.endpoint,
                    json.dumps(record.metadata),
                    record.registered_at,
                    record.lease_expires_at,
                    record.status,
                ))
                conn.commit()
                return True

    def deregister(self, agent_id: str) -> bool:
        """Remove an agent from the registry. Returns True if found and removed."""
        with self._lock:
            with self._conn() as conn:
                cursor = conn.cursor()
                cursor.execute("DELETE FROM agents WHERE agent_id = ?", (agent_id,))
                conn.commit()
                return cursor.rowcount > 0

    def get(self, agent_id: str) -> Optional[AgentRecord]:
        """Get a single agent record by ID."""
        with self._lock:
            with self._conn() as conn:
                cursor = conn.cursor()
                cursor.execute("SELECT * FROM agents WHERE agent_id = ?", (agent_id,))
                row = cursor.fetchone()
                if row:
                    return self._row_to_record(row)
                return None

    def list(self, capability: Optional[str] = None) -> List[AgentRecord]:  # noqa: A002
        """
        List all agents, optionally filtered by capability.
        Only returns non-stale agents.
        """
        with self._lock:
            with self._conn() as conn:
                cursor = conn.cursor()
                if capability:
                    # Filter agents whose capabilities JSON array contains the string
                    cursor.execute(
                        "SELECT * FROM agents WHERE capabilities LIKE ?",
                        (f'%"{capability}"%',)
                    )
                else:
                    cursor.execute("SELECT * FROM agents")
                rows = cursor.fetchall()
                records = [self._row_to_record(row) for row in rows]
                # Filter out stale agents
                return [r for r in records if not r.is_stale()]

    def refresh_lease(self, agent_id: str, ttl_seconds: int = DEFAULT_TTL) -> bool:
        """Refresh an agent's lease. Returns True if agent exists."""
        expires = (datetime.now(timezone.utc) + timedelta(seconds=ttl_seconds)).isoformat()
        with self._lock:
            with self._conn() as conn:
                cursor = conn.cursor()
                cursor.execute(
                    "UPDATE agents SET lease_expires_at = ? WHERE agent_id = ?",
                    (expires, agent_id)
                )
                conn.commit()
                return cursor.rowcount > 0

    def list_stale(self) -> List[AgentRecord]:
        """Return all agents whose leases have expired."""
        now = datetime.now(timezone.utc).isoformat()
        with self._lock:
            with self._conn() as conn:
                cursor = conn.cursor()
                cursor.execute(
                    "SELECT * FROM agents WHERE lease_expires_at < ?",
                    (now,)
                )
                rows = cursor.fetchall()
                return [self._row_to_record(row) for row in rows]

    def delete_stale(self) -> int:
        """Delete all stale agents. Returns count of deleted agents."""
        now = datetime.now(timezone.utc).isoformat()
        with self._lock:
            with self._conn() as conn:
                cursor = conn.cursor()
                cursor.execute("DELETE FROM agents WHERE lease_expires_at < ?", (now,))
                conn.commit()
                return cursor.rowcount

    def health_stats(self) -> dict:
        """Return health statistics."""
        now = datetime.now(timezone.utc).isoformat()
        with self._lock:
            with self._conn() as conn:
                cursor = conn.cursor()
                cursor.execute("SELECT COUNT(*) FROM agents WHERE lease_expires_at >= ?", (now,))
                healthy = cursor.fetchone()[0]
                cursor.execute("SELECT COUNT(*) FROM agents WHERE lease_expires_at < ?", (now,))
                stale = cursor.fetchone()[0]
                cursor.execute("SELECT COUNT(*) FROM agents")
                total = cursor.fetchone()[0]
                return {"healthy": healthy, "stale": stale, "total": total}

    def _row_to_record(self, row: sqlite3.Row) -> AgentRecord:
        return AgentRecord(
            agent_id=row["agent_id"],
            name=row["name"],
            capabilities=json.loads(row["capabilities"]),
            skills=json.loads(row["skills"]),
            endpoint=row["endpoint"],
            metadata=json.loads(row["metadata"]),
            registered_at=row["registered_at"],
            lease_expires_at=row["lease_expires_at"],
            status=row["status"],
        )
