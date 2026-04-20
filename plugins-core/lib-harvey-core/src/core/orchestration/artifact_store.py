"""
ArtifactStore — SQLite-backed artifact registry with cross-process safety.

Phase 1.5 deliverable. This is the "shared state" that makes the worktree
principle real: task 4 can read task 1's output by name, across processes,
with restart survival.

Design goals (from PHASE_1_5_DESIGN.md):

  - SQLite with WAL mode for safe concurrent multi-writer access
  - HARVEY_HOME-aware db path
  - get_by_name (latest version), wait_for (polling block), resolve_deps
  - Versioning: same-name publishes create new versions, old ones kept until GC
  - TTL + pinned flag for garbage collection
  - Thread-safe; cross-process-safe via SQLite locking

NOT reused from core/orchestration/memory_substrate/layer6_artifact.py
because that one is JSONL, not HARVEY_HOME-aware, has no get_by_name,
no wait_for, no dependency resolution.
"""

from __future__ import annotations

import json
import logging
import os
import sqlite3
import threading
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

log = logging.getLogger("harvey.artifact_store")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

DEFAULT_DB_PATH = os.path.join(HARVEY_HOME, "data", "artifacts.db")
DEFAULT_TTL_SECONDS = 86400  # 24h


# ─────────────────────────────────────────────────────────────────────
# Artifact dataclass
# ─────────────────────────────────────────────────────────────────────


@dataclass
class Artifact:
    """A single artifact row from the store."""

    id: str
    name: str
    producer: str
    payload: Any  # JSON-deserialized (could be dict, list, str, etc.)
    depends_on: List[str] = field(default_factory=list)
    created_at: float = 0.0
    ttl_seconds: int = DEFAULT_TTL_SECONDS
    version: int = 1
    pinned: bool = False

    def is_expired(self, now: Optional[float] = None) -> bool:
        if self.pinned:
            return False
        now = now or time.time()
        return (now - self.created_at) >= self.ttl_seconds

    def to_dict(self) -> Dict[str, Any]:
        return {
            "id": self.id,
            "name": self.name,
            "producer": self.producer,
            "payload": self.payload,
            "depends_on": self.depends_on,
            "created_at": self.created_at,
            "ttl_seconds": self.ttl_seconds,
            "version": self.version,
            "pinned": self.pinned,
        }


# ─────────────────────────────────────────────────────────────────────
# ArtifactStore
# ─────────────────────────────────────────────────────────────────────


class ArtifactStore:
    """
    SQLite-backed artifact registry. Thread-safe, cross-process-safe.

    Use a single store instance per process, share across threads. Cross-
    process coordination happens via SQLite's own locking (WAL mode).
    """

    def __init__(self, db_path: Optional[str] = None):
        self.db_path = db_path or DEFAULT_DB_PATH
        Path(self.db_path).parent.mkdir(parents=True, exist_ok=True)

        # check_same_thread=False so multiple threads in one process can share.
        # isolation_level=None → autocommit mode; we'll use explicit transactions
        # where needed via `with self._conn_lock:` blocks.
        self._db = sqlite3.connect(
            self.db_path,
            check_same_thread=False,
            isolation_level=None,
            timeout=30.0,
        )
        self._db.row_factory = sqlite3.Row
        self._conn_lock = threading.RLock()

        self._init_schema()

    def _init_schema(self) -> None:
        with self._conn_lock:
            self._db.execute("PRAGMA journal_mode=WAL")
            self._db.execute("PRAGMA synchronous=NORMAL")
            self._db.executescript(
                """
                CREATE TABLE IF NOT EXISTS artifacts (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL,
                    producer        TEXT NOT NULL,
                    payload         TEXT NOT NULL,
                    depends_on      TEXT NOT NULL DEFAULT '[]',
                    created_at      REAL NOT NULL,
                    ttl_seconds     INTEGER NOT NULL DEFAULT 86400,
                    version         INTEGER NOT NULL DEFAULT 1,
                    pinned          INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_artifacts_name
                    ON artifacts(name, version DESC);
                CREATE INDEX IF NOT EXISTS idx_artifacts_producer
                    ON artifacts(producer);
                CREATE INDEX IF NOT EXISTS idx_artifacts_created
                    ON artifacts(created_at);
                """
            )

    # ─── Publish ─────────────────────────────────────────────────

    def publish(
        self,
        name: str,
        payload: Any,
        producer: str,
        depends_on: Optional[List[str]] = None,
        ttl_seconds: int = DEFAULT_TTL_SECONDS,
        pinned: bool = False,
    ) -> str:
        """
        Publish a new artifact version under `name`.

        Returns the new artifact_id. Same-name republishing creates a new
        version — old versions remain until GC'd.
        """
        if not name:
            raise ValueError("Artifact name cannot be empty")

        payload_json = json.dumps(payload, default=str)
        deps_json = json.dumps(depends_on or [])
        created_at = time.time()

        with self._conn_lock:
            # Compute next version for this name
            row = self._db.execute(
                "SELECT COALESCE(MAX(version), 0) + 1 AS next_ver "
                "FROM artifacts WHERE name = ?",
                (name,),
            ).fetchone()
            next_version = row["next_ver"]

            artifact_id = (
                f"artifact://{producer}/{name}/v{next_version}/{uuid.uuid4().hex[:8]}"
            )

            self._db.execute(
                """
                INSERT INTO artifacts
                    (id, name, producer, payload, depends_on,
                     created_at, ttl_seconds, version, pinned)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    artifact_id,
                    name,
                    producer,
                    payload_json,
                    deps_json,
                    created_at,
                    ttl_seconds,
                    next_version,
                    1 if pinned else 0,
                ),
            )

        log.debug(
            f"[artifact] published {name} v{next_version} "
            f"by {producer} (id={artifact_id})"
        )
        return artifact_id

    # ─── Read ────────────────────────────────────────────────────

    def get(self, name: str) -> Optional[Artifact]:
        """Return the latest version of the named artifact, or None."""
        with self._conn_lock:
            row = self._db.execute(
                "SELECT * FROM artifacts WHERE name = ? "
                "ORDER BY version DESC LIMIT 1",
                (name,),
            ).fetchone()
        return self._row_to_artifact(row) if row else None

    def get_by_id(self, artifact_id: str) -> Optional[Artifact]:
        with self._conn_lock:
            row = self._db.execute(
                "SELECT * FROM artifacts WHERE id = ?", (artifact_id,)
            ).fetchone()
        return self._row_to_artifact(row) if row else None

    def get_all_versions(self, name: str) -> List[Artifact]:
        """All versions of a named artifact, newest first."""
        with self._conn_lock:
            rows = self._db.execute(
                "SELECT * FROM artifacts WHERE name = ? ORDER BY version DESC",
                (name,),
            ).fetchall()
        return [self._row_to_artifact(r) for r in rows]

    def exists(self, name: str) -> bool:
        """True iff at least one version of the named artifact exists."""
        with self._conn_lock:
            row = self._db.execute(
                "SELECT 1 FROM artifacts WHERE name = ? LIMIT 1", (name,)
            ).fetchone()
        return row is not None

    def wait_for(
        self,
        name: str,
        timeout: float = 30.0,
        poll_interval: float = 0.1,
    ) -> Optional[Artifact]:
        """
        Block until the named artifact exists or timeout expires.

        Simple polling loop — portable across threads and processes. For
        very high-frequency waiters, Phase 2 can swap in a notification
        mechanism (fcntl, inotify, etc.), but polling is fine for now.
        """
        deadline = time.time() + timeout
        while True:
            art = self.get(name)
            if art is not None:
                return art
            if time.time() >= deadline:
                return None
            time.sleep(poll_interval)

    def list_by_producer(self, producer: str, limit: int = 100) -> List[Artifact]:
        with self._conn_lock:
            rows = self._db.execute(
                "SELECT * FROM artifacts WHERE producer = ? "
                "ORDER BY created_at DESC LIMIT ?",
                (producer, limit),
            ).fetchall()
        return [self._row_to_artifact(r) for r in rows]

    def list_recent(self, limit: int = 100) -> List[Artifact]:
        with self._conn_lock:
            rows = self._db.execute(
                "SELECT * FROM artifacts ORDER BY created_at DESC LIMIT ?",
                (limit,),
            ).fetchall()
        return [self._row_to_artifact(r) for r in rows]

    # ─── Dependency resolution ───────────────────────────────────

    def resolve_deps(
        self, name: str, max_depth: int = 10
    ) -> List[Artifact]:
        """
        Walk the dependency graph transitively starting from `name` and
        return all artifacts that `name` depends on (directly or indirectly).

        Returns the collected artifacts in topological order (deepest first).
        The root artifact itself is NOT included — only its dependencies.
        Missing dependencies are skipped silently (they may not be published yet).
        """
        root = self.get(name)
        if root is None:
            return []

        visited: Dict[str, Artifact] = {}
        order: List[Artifact] = []

        def walk(current_name: str, depth: int):
            if depth > max_depth:
                log.warning(
                    f"[artifact] resolve_deps hit max_depth={max_depth} "
                    f"at {current_name}"
                )
                return
            current = self.get(current_name)
            if current is None:
                return
            if current.id in visited:
                return
            visited[current.id] = current
            for dep_name in current.depends_on:
                walk(dep_name, depth + 1)
            if current_name != name:  # exclude root
                order.append(current)

        for dep_name in root.depends_on:
            walk(dep_name, 1)

        return order

    # ─── Garbage collection ──────────────────────────────────────

    def gc(self) -> int:
        """Delete expired (non-pinned) artifacts. Returns count removed."""
        now = time.time()
        with self._conn_lock:
            cursor = self._db.execute(
                """
                DELETE FROM artifacts
                WHERE pinned = 0
                AND (? - created_at) >= ttl_seconds
                """,
                (now,),
            )
            removed = cursor.rowcount or 0
        if removed:
            log.info(f"[artifact] gc removed {removed} expired artifacts")
        return removed

    def count(self) -> int:
        with self._conn_lock:
            row = self._db.execute(
                "SELECT COUNT(*) AS n FROM artifacts"
            ).fetchone()
        return row["n"] if row else 0

    def close(self) -> None:
        with self._conn_lock:
            try:
                self._db.close()
            except Exception:
                pass

    # ─── Internal helpers ────────────────────────────────────────

    def _row_to_artifact(self, row: sqlite3.Row) -> Artifact:
        return Artifact(
            id=row["id"],
            name=row["name"],
            producer=row["producer"],
            payload=json.loads(row["payload"]),
            depends_on=json.loads(row["depends_on"] or "[]"),
            created_at=row["created_at"],
            ttl_seconds=row["ttl_seconds"],
            version=row["version"],
            pinned=bool(row["pinned"]),
        )


# ─────────────────────────────────────────────────────────────────────
# Module-level singleton
# ─────────────────────────────────────────────────────────────────────

_default_store: Optional[ArtifactStore] = None
_default_lock = threading.Lock()


def get_default_store() -> ArtifactStore:
    """Lazy-create the process-wide default store."""
    global _default_store
    with _default_lock:
        if _default_store is None:
            _default_store = ArtifactStore()
    return _default_store


def shutdown_default_store() -> None:
    global _default_store
    with _default_lock:
        if _default_store is not None:
            _default_store.close()
            _default_store = None


__all__ = [
    "Artifact",
    "ArtifactStore",
    "get_default_store",
    "shutdown_default_store",
    "DEFAULT_DB_PATH",
    "DEFAULT_TTL_SECONDS",
]
