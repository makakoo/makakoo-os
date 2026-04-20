#!/usr/bin/env python3
"""
SQLite Session Storage with FTS5 Full-Text Search.

Implements session storage for Harvey OS with FTS5 full-text search on messages.
Adapted from hermes-agent/hermes_state.py pattern.

Key features:
- SQLite with WAL mode for concurrent readers + one writer
- FTS5 virtual table for fast text search across all session messages
- Session grouping with parent_session_id chains for delegation/compression
- Child session resolution to find root parent

Database path: $HARVEY_HOME/data/sessions/sessions.db
"""

import json
import logging
import os
import re
import sqlite3
import threading
import time
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, TypeVar

logger = logging.getLogger(__name__)

T = TypeVar("T")

# Default database path relative to HARVEY_HOME
HARVEY_HOME = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
DEFAULT_DB_PATH = Path(HARVEY_HOME) / "data" / "sessions" / "sessions.db"

SCHEMA_VERSION = 1

SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    title TEXT,
    source TEXT NOT NULL DEFAULT 'harvey',
    model TEXT,
    started_at REAL NOT NULL,
    last_active REAL,
    ended_at REAL,
    end_reason TEXT,
    parent_session_id TEXT,
    message_count INTEGER DEFAULT 0,
    FOREIGN KEY (parent_session_id) REFERENCES sessions(id)
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,
    content TEXT,
    tool_call_id TEXT,
    tool_calls TEXT,
    tool_name TEXT,
    created_at REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_source ON sessions(source);
CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);
CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, created_at);
"""

FTS_SQL = """
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content,
    content=messages,
    content_rowid=id
);

CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;
"""


class SessionDB:
    """
    SQLite-backed session storage with FTS5 search.

    Thread-safe for concurrent readers + single writer via WAL mode.
    Each method opens its own cursor.
    """

    _WRITE_MAX_RETRIES = 15
    _WRITE_RETRY_MIN_S = 0.020
    _WRITE_RETRY_MAX_S = 0.150

    def __init__(self, db_path: Path = None):
        self.db_path = Path(db_path) if db_path else DEFAULT_DB_PATH
        self.db_path.parent.mkdir(parents=True, exist_ok=True)

        self._lock = threading.Lock()
        self._conn = self._create_connection()
        self._init_schema()

    def _create_connection(self) -> sqlite3.Connection:
        """Create a new SQLite connection with WAL mode and proper settings."""
        conn = sqlite3.connect(
            str(self.db_path),
            check_same_thread=False,
            timeout=1.0,
            isolation_level=None,
        )
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA foreign_keys=ON")
        return conn

    def _get_conn(self) -> sqlite3.Connection:
        """Get the connection (not thread-safe for writes)."""
        return self._conn

    def _init_schema(self):
        """Create tables and FTS if they don't exist."""
        cursor = self._conn.cursor()
        cursor.executescript(SCHEMA_SQL)

        # Check schema version
        cursor.execute("SELECT version FROM schema_version LIMIT 1")
        row = cursor.fetchone()
        if row is None:
            cursor.execute(
                "INSERT INTO schema_version (version) VALUES (?)", (SCHEMA_VERSION,)
            )
        else:
            current_version = row["version"] if isinstance(row, sqlite3.Row) else row[0]
            if current_version < SCHEMA_VERSION:
                cursor.execute(
                    "UPDATE schema_version SET version = ?", (SCHEMA_VERSION,)
                )

        # FTS5 setup
        try:
            cursor.execute("SELECT * FROM messages_fts LIMIT 0")
        except sqlite3.OperationalError:
            cursor.executescript(FTS_SQL)

        self._conn.commit()

    def close(self):
        """Close the database connection."""
        with self._lock:
            if self._conn:
                self._conn.close()
                self._conn = None

    # =========================================================================
    # Session lifecycle
    # =========================================================================

    def create_session(self, title: str, source: str = "harvey") -> str:
        """Create a new session. Returns the session_id (UUID)."""
        import uuid

        session_id = str(uuid.uuid4())

        def _do(conn):
            conn.execute(
                """INSERT INTO sessions (id, title, source, started_at, last_active)
                   VALUES (?, ?, ?, ?, ?)""",
                (session_id, title, source, time.time(), time.time()),
            )

        self._execute_write(_do)
        return session_id

    def get_session(self, session_id: str) -> Optional[Dict[str, Any]]:
        """Get a session by ID."""
        with self._lock:
            cursor = self._conn.execute(
                "SELECT * FROM sessions WHERE id = ?", (session_id,)
            )
            row = cursor.fetchone()
        return dict(row) if row else None

    def list_sessions(
        self, limit: int = 10, exclude_sources: List[str] = None
    ) -> List[Dict[str, Any]]:
        """List recent sessions with metadata."""
        if exclude_sources:
            placeholders = ",".join("?" for _ in exclude_sources)
            where_sql = f"WHERE source NOT IN ({placeholders})"
            params = exclude_sources + [limit]
        else:
            where_sql = ""
            params = [limit]

        with self._lock:
            cursor = self._conn.execute(
                f"""SELECT s.*,
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) as message_count
                    FROM sessions s
                    {where_sql}
                    ORDER BY s.started_at DESC
                    LIMIT ?""",
                params,
            )
            rows = cursor.fetchall()
        return [dict(row) for row in rows]

    # =========================================================================
    # Message storage
    # =========================================================================

    def insert_message(
        self,
        session_id: str,
        role: str,
        content: str = None,
        tool_name: str = None,
        tool_call_id: str = None,
    ) -> int:
        """Append a message to a session. Returns the message row ID."""
        tool_calls_json = None

        def _do(conn):
            cursor = conn.execute(
                """INSERT INTO messages (session_id, role, content, tool_call_id, tool_calls, tool_name, created_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?)""",
                (
                    session_id,
                    role,
                    content,
                    tool_call_id,
                    tool_calls_json,
                    tool_name,
                    time.time(),
                ),
            )
            msg_id = cursor.lastrowid

            # Update session counters
            conn.execute(
                "UPDATE sessions SET message_count = message_count + 1, last_active = ? WHERE id = ?",
                (time.time(), session_id),
            )
            return msg_id

        return self._execute_write(_do)

    def get_messages_as_conversation(self, session_id: str) -> List[Dict[str, Any]]:
        """Load messages in conversation format (role + content dicts)."""
        with self._lock:
            cursor = self._conn.execute(
                "SELECT role, content, tool_call_id, tool_name FROM messages WHERE session_id = ? ORDER BY created_at, id",
                (session_id,),
            )
            rows = cursor.fetchall()
        messages = []
        for row in rows:
            msg: Dict[str, Any] = {"role": row["role"], "content": row["content"]}
            if row["tool_call_id"]:
                msg["tool_call_id"] = row["tool_call_id"]
            if row["tool_name"]:
                msg["tool_name"] = row["tool_name"]
            messages.append(msg)
        return messages

    # =========================================================================
    # Search
    # =========================================================================

    @staticmethod
    def _sanitize_fts5_query(query: str) -> str:
        """Sanitize user input for safe use in FTS5 MATCH queries."""
        # Extract quoted phrases and protect them
        _quoted_parts: list = []

        def _preserve_quoted(m: re.Match) -> str:
            _quoted_parts.append(m.group(0))
            return f"\x00Q{len(_quoted_parts) - 1}\x00"

        sanitized = re.sub(r'"[^"]*"', _preserve_quoted, query)

        # Strip FTS5-special characters
        sanitized = re.sub(r"[+{}()\"^]", " ", sanitized)

        # Collapse repeated * and remove leading *
        sanitized = re.sub(r"\*+", "*", sanitized)
        sanitized = re.sub(r"(^|\s)\*", r"\1", sanitized)

        # Remove dangling boolean operators
        sanitized = re.sub(r"(?i)^(AND|OR|NOT)\b\s*", "", sanitized.strip())
        sanitized = re.sub(r"(?i)\s+(AND|OR|NOT)\s*$", "", sanitized.strip())

        # Wrap unquoted hyphenated terms in quotes
        sanitized = re.sub(r"\b(\w+(?:-\w+)+)\b", r'"\1"', sanitized)

        # Restore preserved quoted phrases
        for i, quoted in enumerate(_quoted_parts):
            sanitized = sanitized.replace(f"\x00Q{i}\x00", quoted)

        return sanitized.strip()

    def search_messages(
        self,
        query: str,
        role_filter: List[str] = None,
        exclude_sources: List[str] = None,
        limit: int = 50,
        offset: int = 0,
    ) -> List[Dict[str, Any]]:
        """
        Full-text search across session messages using FTS5.

        Supports FTS5 query syntax:
          - Simple keywords: "docker deployment"
          - Phrases: '"exact phrase"'
          - Boolean: "docker OR kubernetes"
          - Prefix: "deploy*"
        """
        if not query or not query.strip():
            return []

        query = self._sanitize_fts5_query(query)
        if not query:
            return []

        # Build WHERE clauses
        where_clauses = ["messages_fts MATCH ?"]
        params: list = [query]

        if exclude_sources:
            exclude_placeholders = ",".join("?" for _ in exclude_sources)
            where_clauses.append(f"s.source NOT IN ({exclude_placeholders})")
            params.extend(exclude_sources)

        if role_filter:
            role_placeholders = ",".join("?" for _ in role_filter)
            where_clauses.append(f"m.role IN ({role_placeholders})")
            params.extend(role_filter)

        where_sql = " AND ".join(where_clauses)
        params.extend([limit, offset])

        sql = f"""
            SELECT
                m.id,
                m.session_id,
                m.role,
                snippet(messages_fts, 0, '>>>', '<<<', '...', 40) AS snippet,
                m.content,
                m.created_at as timestamp,
                m.tool_name,
                s.source,
                s.started_at AS session_started
            FROM messages_fts
            JOIN messages m ON m.id = messages_fts.rowid
            JOIN sessions s ON s.id = m.session_id
            WHERE {where_sql}
            ORDER BY rank
            LIMIT ? OFFSET ?
        """

        with self._lock:
            try:
                cursor = self._conn.execute(sql, params)
            except sqlite3.OperationalError:
                # FTS5 query syntax error - return empty
                return []
            matches = [dict(row) for row in cursor.fetchall()]

        # Remove full content from result (snippet is enough)
        for match in matches:
            match.pop("content", None)

        return matches

    # =========================================================================
    # Write helper
    # =========================================================================

    def _execute_write(self, fn: Callable[[sqlite3.Connection], T]) -> T:
        """Execute a write transaction with BEGIN IMMEDIATE and jitter retry."""
        import random

        last_err: Optional[Exception] = None
        for attempt in range(self._WRITE_MAX_RETRIES):
            try:
                with self._lock:
                    self._conn.execute("BEGIN IMMEDIATE")
                    try:
                        result = fn(self._conn)
                        self._conn.commit()
                    except BaseException:
                        try:
                            self._conn.rollback()
                        except Exception:
                            pass
                        raise
                return result
            except sqlite3.OperationalError as exc:
                err_msg = str(exc).lower()
                if "locked" in err_msg or "busy" in err_msg:
                    last_err = exc
                    if attempt < self._WRITE_MAX_RETRIES - 1:
                        jitter = random.uniform(
                            self._WRITE_RETRY_MIN_S,
                            self._WRITE_RETRY_MAX_S,
                        )
                        time.sleep(jitter)
                        continue
                raise
        raise last_err or sqlite3.OperationalError(
            "database is locked after max retries"
        )


# Exports
__all__ = ["SessionDB", "search_messages", "get_messages_as_conversation"]
