"""
HarveyChat conversation store — SQLite persistence for chat history.

Stores messages per channel+user with metadata for context retrieval.
"""

import json
import sqlite3
import time
from pathlib import Path
from typing import Dict, List, Optional


class ChatStore:
    """SQLite-backed conversation store."""

    def __init__(self, db_path: str):
        Path(db_path).parent.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(db_path, check_same_thread=False)
        self.db.row_factory = sqlite3.Row
        # Enable WAL mode for concurrent read/write safety
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA busy_timeout=5000")
        self._init_schema()

    def _init_schema(self):
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,          -- 'telegram', 'whatsapp', etc.
                channel_user_id TEXT NOT NULL,   -- user ID on that channel
                role TEXT NOT NULL,              -- 'user' or 'assistant'
                content TEXT NOT NULL,
                metadata TEXT DEFAULT '{}',      -- JSON blob for extras
                created_at REAL NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_messages_channel_user
                ON messages(channel, channel_user_id, created_at);

            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                channel_user_id TEXT NOT NULL,
                started_at REAL NOT NULL,
                last_active REAL NOT NULL,
                message_count INTEGER DEFAULT 0,
                metadata TEXT DEFAULT '{}'
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_channel_user
                ON sessions(channel, channel_user_id);
        """)
        self.db.commit()

    def add_message(self, channel: str, channel_user_id: str, role: str,
                    content: str, metadata: Optional[Dict] = None) -> int:
        """Store a message and return its ID."""
        now = time.time()
        cur = self.db.execute(
            "INSERT INTO messages (channel, channel_user_id, role, content, metadata, created_at) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            (channel, channel_user_id, role, content, json.dumps(metadata or {}), now)
        )
        # Update or create session
        session = self.db.execute(
            "SELECT id FROM sessions WHERE channel = ? AND channel_user_id = ? "
            "ORDER BY last_active DESC LIMIT 1",
            (channel, channel_user_id)
        ).fetchone()

        if session and (now - self._get_session_last_active(session["id"])) < 3600:
            self.db.execute(
                "UPDATE sessions SET last_active = ?, message_count = message_count + 1 WHERE id = ?",
                (now, session["id"])
            )
        else:
            self.db.execute(
                "INSERT INTO sessions (channel, channel_user_id, started_at, last_active, message_count) "
                "VALUES (?, ?, ?, ?, 1)",
                (channel, channel_user_id, now, now)
            )

        self.db.commit()
        return cur.lastrowid

    def _get_session_last_active(self, session_id: int) -> float:
        row = self.db.execute("SELECT last_active FROM sessions WHERE id = ?", (session_id,)).fetchone()
        return row["last_active"] if row else 0

    def get_history(self, channel: str, channel_user_id: str,
                    limit: int = 20) -> List[Dict]:
        """Get recent conversation history as OpenAI-format messages."""
        rows = self.db.execute(
            "SELECT role, content, created_at FROM messages "
            "WHERE channel = ? AND channel_user_id = ? "
            "ORDER BY created_at DESC LIMIT ?",
            (channel, channel_user_id, limit)
        ).fetchall()

        return [{"role": r["role"], "content": r["content"]} for r in reversed(rows)]

    def get_stats(self) -> Dict:
        """Get store statistics."""
        total = self.db.execute("SELECT COUNT(*) as c FROM messages").fetchone()["c"]
        sessions = self.db.execute("SELECT COUNT(*) as c FROM sessions").fetchone()["c"]
        channels = self.db.execute("SELECT DISTINCT channel FROM messages").fetchall()
        return {
            "total_messages": total,
            "total_sessions": sessions,
            "channels": [r["channel"] for r in channels],
        }

    def close(self):
        self.db.close()
