"""
TaskStore — durable SQLite-backed task tree.

Wraps a single SQLite DB in WAL mode. Thread-safe via per-thread connections.
Atomic claim protocol for the cron resumer: an UPDATE ... WHERE state=?
guarantees only one worker picks up a stale task.

See development/sprints/SPRINT-HARVEY-COGNITIVE-CORE.md § 3.1 for schema.
"""

from __future__ import annotations

import json
import logging
import os
import sqlite3
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional

from .models import (
    EntryType,
    Task,
    TaskArtifact,
    TaskEntry,
    TaskKind,
    TaskState,
    new_id,
)

log = logging.getLogger("harvey.tasks.store")


# ─── Thread-local actor context ──────────────────────────────────
#
# Used by Phase 5 (TaskEntry.actor wiring): when a subagent runs inside
# tool_spawn_subagent, harvey_agent sets _ACTOR_CONTEXT.name = agent_name
# via `set_current_actor()`. Any TaskEntry appended on this thread that
# does NOT specify `actor` explicitly inherits the current actor.
# Defined here so it's reachable without creating an import cycle.

_ACTOR_CONTEXT = threading.local()


def set_current_actor(name: Optional[str]) -> None:
    """Set the current actor for entries appended on this thread."""
    _ACTOR_CONTEXT.name = name


def clear_current_actor() -> None:
    """Clear the current actor (safe to call from a `finally` block)."""
    _ACTOR_CONTEXT.name = None


def current_actor() -> Optional[str]:
    """Read the current actor without modifying it."""
    return getattr(_ACTOR_CONTEXT, "name", None)

DEFAULT_DB_PATH = os.path.join(
    os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
    "data",
    "cognitive",
    "tasks.db",
)
# NOTE: Deliberately NOT data/chat/tasks.db — that path holds the old dead
# TaskQueue schema (16 columns, one of which is `messages`), which collides
# with our schema. Phase 2 will drop data/chat/tasks.db entirely.

STALE_HEARTBEAT_SECONDS = 300  # 5 minutes
MAX_DEPTH = 3                   # planner → step → subagent


SCHEMA = """
CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    root_id     TEXT NOT NULL,
    parent_id   TEXT,
    depth       INTEGER NOT NULL DEFAULT 0,

    channel     TEXT NOT NULL,
    user_id     TEXT NOT NULL,

    kind        TEXT NOT NULL,
    goal        TEXT NOT NULL,
    state       TEXT NOT NULL,
    plan_json   TEXT,

    created_at  REAL NOT NULL,
    started_at  REAL,
    completed_at REAL,
    heartbeat   REAL,

    result      TEXT NOT NULL DEFAULT '',
    error       TEXT NOT NULL DEFAULT '',
    metadata    TEXT NOT NULL DEFAULT '{}',

    assignee    TEXT,

    FOREIGN KEY (parent_id) REFERENCES tasks(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_tasks_state     ON tasks(state);
CREATE INDEX IF NOT EXISTS idx_tasks_root      ON tasks(root_id);
CREATE INDEX IF NOT EXISTS idx_tasks_heartbeat ON tasks(heartbeat);
CREATE INDEX IF NOT EXISTS idx_tasks_user      ON tasks(channel, user_id, created_at);

CREATE TABLE IF NOT EXISTS task_entries (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL,
    parent_entry_id TEXT,
    entry_type      TEXT NOT NULL,
    role            TEXT,
    tool_name       TEXT,
    content         TEXT NOT NULL,
    is_error        INTEGER NOT NULL DEFAULT 0,
    tokens_in       INTEGER,
    tokens_out      INTEGER,
    created_at      REAL NOT NULL,
    actor           TEXT,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_entries_task  ON task_entries(task_id, created_at);

CREATE TABLE IF NOT EXISTS task_artifacts (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL,
    kind         TEXT NOT NULL,
    path         TEXT,
    url          TEXT,
    mime         TEXT,
    size_bytes   INTEGER,
    description  TEXT NOT NULL DEFAULT '',
    sent_to_user INTEGER NOT NULL DEFAULT 0,
    created_at   REAL NOT NULL,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_artifacts_task ON task_artifacts(task_id);
"""


class TaskStore:
    def __init__(self, db_path: str = DEFAULT_DB_PATH):
        self.db_path = os.path.abspath(os.path.expanduser(db_path))
        Path(self.db_path).parent.mkdir(parents=True, exist_ok=True)
        self._local = threading.local()
        self._bootstrap()

    # ─── Connection management ───────────────────────────────────

    def _conn(self) -> sqlite3.Connection:
        conn = getattr(self._local, "conn", None)
        if conn is None:
            conn = sqlite3.connect(self.db_path, isolation_level=None, check_same_thread=False)
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("PRAGMA synchronous=NORMAL")
            conn.execute("PRAGMA foreign_keys=ON")
            conn.execute("PRAGMA busy_timeout=5000")
            self._local.conn = conn
        return conn

    def _bootstrap(self):
        conn = self._conn()
        conn.executescript(SCHEMA)
        self._migrate_ticketing_columns(conn)

    @staticmethod
    def _column_names(conn: sqlite3.Connection, table: str) -> set:
        rows = conn.execute(f"PRAGMA table_info({table})").fetchall()
        return {r["name"] for r in rows}

    def _migrate_ticketing_columns(self, conn: sqlite3.Connection) -> None:
        """Add SPRINT-HARVEY-TICKETING columns to existing DBs, then index.

        Idempotent: PRAGMA table_info is cheap and safe to run on every
        boot. ALTER TABLE only fires if the column is missing, so we can
        upgrade a cognitive-core DB in-place without losing state.

        Indexes are created here (not in SCHEMA) because SCHEMA's
        CREATE INDEX runs BEFORE the ALTER TABLE on an old DB, which
        would fail with "no such column: assignee". Create indexes
        unconditionally AFTER the columns exist — `IF NOT EXISTS`
        makes this safe on repeated bootstraps.
        """
        task_cols = self._column_names(conn, "tasks")
        if "assignee" not in task_cols:
            conn.execute("ALTER TABLE tasks ADD COLUMN assignee TEXT")
            log.info("[store] migrated: added tasks.assignee column")

        entry_cols = self._column_names(conn, "task_entries")
        if "actor" not in entry_cols:
            conn.execute("ALTER TABLE task_entries ADD COLUMN actor TEXT")
            log.info("[store] migrated: added task_entries.actor column")

        conn.execute("CREATE INDEX IF NOT EXISTS idx_tasks_assignee ON tasks(assignee)")
        conn.execute("CREATE INDEX IF NOT EXISTS idx_entries_actor ON task_entries(actor)")

    @contextmanager
    def _tx(self) -> Iterator[sqlite3.Connection]:
        conn = self._conn()
        conn.execute("BEGIN IMMEDIATE")
        try:
            yield conn
            conn.execute("COMMIT")
        except Exception:
            conn.execute("ROLLBACK")
            raise

    def close(self):
        conn = getattr(self._local, "conn", None)
        if conn is not None:
            conn.close()
            self._local.conn = None

    # ─── Task CRUD ──────────────────────────────────────────────

    def create_root_task(self, *, channel: str, user_id: str, goal: str) -> Task:
        task = Task.new_root(channel=channel, user_id=user_id, goal=goal)
        self._insert_task(task)
        log.info(f"Created root task {task.id[:8]} for {channel}:{user_id} — {goal[:60]}")
        return task

    def create_child_task(
        self,
        parent: Task,
        *,
        kind: TaskKind,
        goal: str,
    ) -> Task:
        if parent.depth >= MAX_DEPTH:
            raise ValueError(
                f"Max task depth {MAX_DEPTH} exceeded "
                f"(parent {parent.id[:8]} depth={parent.depth})"
            )
        task = Task.new_child(parent, kind=kind, goal=goal)
        self._insert_task(task)
        log.info(f"Created child task {task.id[:8]} (kind={kind.value}) under {parent.id[:8]}")
        return task

    def _insert_task(self, task: Task) -> None:
        with self._tx() as conn:
            conn.execute(
                """
                INSERT INTO tasks (
                    id, root_id, parent_id, depth,
                    channel, user_id,
                    kind, goal, state, plan_json,
                    created_at, started_at, completed_at, heartbeat,
                    result, error, metadata, assignee
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    task.id, task.root_id, task.parent_id, task.depth,
                    task.channel, task.user_id,
                    task.kind.value, task.goal, task.state.value, task.plan_json,
                    task.created_at, task.started_at, task.completed_at, task.heartbeat,
                    task.result, task.error, json.dumps(task.metadata),
                    task.assignee,
                ),
            )

    def set_assignee(self, task_id: str, agent_name: Optional[str]) -> bool:
        """Set (or clear) the agent that owns this task.

        Returns True if the row was updated. No-op returning False if the
        task doesn't exist — keeps the call site simple (no try/except
        around "might not be there yet" cases).
        """
        with self._tx() as conn:
            cursor = conn.execute(
                "UPDATE tasks SET assignee = ? WHERE id = ?",
                (agent_name, task_id),
            )
            return cursor.rowcount == 1

    def get_task(self, task_id: str) -> Optional[Task]:
        row = self._conn().execute("SELECT * FROM tasks WHERE id = ?", (task_id,)).fetchone()
        return _row_to_task(row) if row else None

    def set_state(
        self,
        task_id: str,
        state: TaskState,
        *,
        result: Optional[str] = None,
        error: Optional[str] = None,
    ) -> None:
        now = time.time()
        sets = ["state = ?"]
        params: List[Any] = [state.value]

        if state == TaskState.RUNNING:
            sets.append("started_at = COALESCE(started_at, ?)")
            params.append(now)
            sets.append("heartbeat = ?")
            params.append(now)
        if state in TaskState.terminal_states():
            sets.append("completed_at = ?")
            params.append(now)
        if result is not None:
            sets.append("result = ?")
            params.append(result)
        if error is not None:
            sets.append("error = ?")
            params.append(error)

        params.append(task_id)
        with self._tx() as conn:
            conn.execute(f"UPDATE tasks SET {', '.join(sets)} WHERE id = ?", params)

    def touch(self, task_id: str) -> None:
        """Heartbeat — signals the task is still being worked on."""
        with self._tx() as conn:
            conn.execute("UPDATE tasks SET heartbeat = ? WHERE id = ?", (time.time(), task_id))

    def set_plan(self, task_id: str, plan: Dict[str, Any]) -> None:
        with self._tx() as conn:
            conn.execute(
                "UPDATE tasks SET plan_json = ?, state = ? WHERE id = ?",
                (json.dumps(plan), TaskState.PLANNING.value, task_id),
            )

    # ─── Query helpers ──────────────────────────────────────────

    def active_root_for_user(self, channel: str, user_id: str) -> Optional[Task]:
        """The most recent non-terminal root task for this user, if any."""
        active = [s.value for s in TaskState.active_states()]
        placeholders = ", ".join("?" * len(active))
        row = self._conn().execute(
            f"""
            SELECT * FROM tasks
            WHERE channel = ? AND user_id = ? AND kind = 'root' AND state IN ({placeholders})
            ORDER BY created_at DESC LIMIT 1
            """,
            (channel, user_id, *active),
        ).fetchone()
        return _row_to_task(row) if row else None

    def pending_for_user(self, channel: str, user_id: str) -> List[Task]:
        """All non-terminal root tasks for this user, newest first."""
        active = [s.value for s in TaskState.active_states()]
        placeholders = ", ".join("?" * len(active))
        rows = self._conn().execute(
            f"""
            SELECT * FROM tasks
            WHERE channel = ? AND user_id = ? AND kind = 'root' AND state IN ({placeholders})
            ORDER BY created_at DESC
            """,
            (channel, user_id, *active),
        ).fetchall()
        return [_row_to_task(r) for r in rows]

    def children_of(self, parent_id: str) -> List[Task]:
        rows = self._conn().execute(
            "SELECT * FROM tasks WHERE parent_id = ? ORDER BY created_at",
            (parent_id,),
        ).fetchall()
        return [_row_to_task(r) for r in rows]

    def tree(self, root_id: str) -> List[Task]:
        """All tasks under a root, depth-ordered then created-ordered."""
        rows = self._conn().execute(
            "SELECT * FROM tasks WHERE root_id = ? ORDER BY depth, created_at",
            (root_id,),
        ).fetchall()
        return [_row_to_task(r) for r in rows]

    def stale_running(self, older_than_seconds: float = STALE_HEARTBEAT_SECONDS) -> List[Task]:
        """Tasks that are still marked running but have not heartbeat recently."""
        cutoff = time.time() - older_than_seconds
        rows = self._conn().execute(
            """
            SELECT * FROM tasks
            WHERE state = ? AND heartbeat IS NOT NULL AND heartbeat < ?
            """,
            (TaskState.RUNNING.value, cutoff),
        ).fetchall()
        return [_row_to_task(r) for r in rows]

    def claim_stale(self, task_id: str, older_than_seconds: float = STALE_HEARTBEAT_SECONDS) -> bool:
        """Atomically mark a stale running task as resuming. Returns True if claimed."""
        cutoff = time.time() - older_than_seconds
        now = time.time()
        with self._tx() as conn:
            cursor = conn.execute(
                """
                UPDATE tasks
                SET state = ?, heartbeat = ?
                WHERE id = ? AND state = ? AND heartbeat IS NOT NULL AND heartbeat < ?
                """,
                (
                    TaskState.RESUMING.value, now,
                    task_id, TaskState.RUNNING.value, cutoff,
                ),
            )
            return cursor.rowcount == 1

    def reset_stuck_resuming(
        self, older_than_seconds: float = STALE_HEARTBEAT_SECONDS * 2
    ) -> int:
        """Reset tasks stuck in RESUMING back to RUNNING so they can be re-swept.

        Handles the kill-9 edge case: if a spawned resumer subprocess dies
        between `claim_stale → RESUMING` and `run_task → RUNNING`, the task
        would otherwise stay in RESUMING forever since RESUMING is an active
        state and `stale_running()` only matches RUNNING.

        Uses 2x the stale threshold so the resumer has time to actually
        claim, start the Python VM, and transition to RUNNING on healthy
        hardware. Returns the number of rows reset.
        """
        cutoff = time.time() - older_than_seconds
        with self._tx() as conn:
            cursor = conn.execute(
                """
                UPDATE tasks
                SET state = ?
                WHERE state = ? AND heartbeat IS NOT NULL AND heartbeat < ?
                """,
                (TaskState.RUNNING.value, TaskState.RESUMING.value, cutoff),
            )
            return cursor.rowcount

    # ─── Entries ────────────────────────────────────────────────

    def append_entry(self, entry: TaskEntry) -> TaskEntry:
        # Inherit thread-local actor if the caller didn't set one explicitly.
        # Phase 5: harvey_agent wraps the ThreadPoolExecutor.submit in a helper
        # that sets _ACTOR_CONTEXT.name = agent_name for the duration of the
        # subagent call. Any entries written on that thread pick it up here.
        if entry.actor is None:
            entry.actor = current_actor()

        with self._tx() as conn:
            conn.execute(
                """
                INSERT INTO task_entries (
                    id, task_id, parent_entry_id, entry_type,
                    role, tool_name, content, is_error,
                    tokens_in, tokens_out, created_at, actor
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    entry.id, entry.task_id, entry.parent_entry_id, entry.entry_type.value,
                    entry.role, entry.tool_name, entry.content, 1 if entry.is_error else 0,
                    entry.tokens_in, entry.tokens_out, entry.created_at, entry.actor,
                ),
            )
        return entry

    def get_entries(self, task_id: str) -> List[TaskEntry]:
        rows = self._conn().execute(
            "SELECT * FROM task_entries WHERE task_id = ? ORDER BY created_at, id",
            (task_id,),
        ).fetchall()
        return [_row_to_entry(r) for r in rows]

    def get_entries_for_tree(self, root_id: str) -> List[TaskEntry]:
        rows = self._conn().execute(
            """
            SELECT e.* FROM task_entries e
            JOIN tasks t ON e.task_id = t.id
            WHERE t.root_id = ?
            ORDER BY e.created_at, e.id
            """,
            (root_id,),
        ).fetchall()
        return [_row_to_entry(r) for r in rows]

    def last_user_message(self, task_id: str) -> Optional[TaskEntry]:
        row = self._conn().execute(
            """
            SELECT * FROM task_entries
            WHERE task_id = ? AND entry_type = 'message' AND role = 'user'
            ORDER BY created_at DESC LIMIT 1
            """,
            (task_id,),
        ).fetchone()
        return _row_to_entry(row) if row else None

    # ─── Artifacts ──────────────────────────────────────────────

    def record_artifact(self, artifact: TaskArtifact) -> TaskArtifact:
        with self._tx() as conn:
            conn.execute(
                """
                INSERT INTO task_artifacts (
                    id, task_id, kind, path, url, mime, size_bytes,
                    description, sent_to_user, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    artifact.id, artifact.task_id, artifact.kind,
                    artifact.path, artifact.url, artifact.mime, artifact.size_bytes,
                    artifact.description, 1 if artifact.sent_to_user else 0, artifact.created_at,
                ),
            )
        return artifact

    def get_artifacts(self, task_id: str) -> List[TaskArtifact]:
        rows = self._conn().execute(
            "SELECT * FROM task_artifacts WHERE task_id = ? ORDER BY created_at",
            (task_id,),
        ).fetchall()
        return [_row_to_artifact(r) for r in rows]

    def get_artifacts_for_tree(self, root_id: str) -> List[TaskArtifact]:
        rows = self._conn().execute(
            """
            SELECT a.* FROM task_artifacts a
            JOIN tasks t ON a.task_id = t.id
            WHERE t.root_id = ? ORDER BY a.created_at
            """,
            (root_id,),
        ).fetchall()
        return [_row_to_artifact(r) for r in rows]

    def mark_artifact_sent(self, artifact_id: str) -> None:
        with self._tx() as conn:
            conn.execute(
                "UPDATE task_artifacts SET sent_to_user = 1 WHERE id = ?",
                (artifact_id,),
            )


# ─── Row adapters ────────────────────────────────────────────────

def _row_to_task(row: sqlite3.Row) -> Task:
    return Task(
        id=row["id"],
        root_id=row["root_id"],
        parent_id=row["parent_id"],
        depth=row["depth"],
        channel=row["channel"],
        user_id=row["user_id"],
        kind=TaskKind(row["kind"]),
        goal=row["goal"],
        state=TaskState(row["state"]),
        plan_json=row["plan_json"],
        created_at=row["created_at"],
        started_at=row["started_at"],
        completed_at=row["completed_at"],
        heartbeat=row["heartbeat"],
        result=row["result"],
        error=row["error"],
        metadata=json.loads(row["metadata"] or "{}"),
        assignee=row["assignee"] if "assignee" in row.keys() else None,
    )


def _row_to_entry(row: sqlite3.Row) -> TaskEntry:
    return TaskEntry(
        id=row["id"],
        task_id=row["task_id"],
        parent_entry_id=row["parent_entry_id"],
        entry_type=EntryType(row["entry_type"]),
        role=row["role"],
        tool_name=row["tool_name"],
        content=row["content"],
        is_error=bool(row["is_error"]),
        tokens_in=row["tokens_in"],
        tokens_out=row["tokens_out"],
        created_at=row["created_at"],
        actor=row["actor"] if "actor" in row.keys() else None,
    )


def _row_to_artifact(row: sqlite3.Row) -> TaskArtifact:
    return TaskArtifact(
        id=row["id"],
        task_id=row["task_id"],
        kind=row["kind"],
        path=row["path"],
        url=row["url"],
        mime=row["mime"],
        size_bytes=row["size_bytes"],
        description=row["description"],
        sent_to_user=bool(row["sent_to_user"]),
        created_at=row["created_at"],
    )
