"""
ChannelErrorCooldown — per-chat error suppression backed by SQLite.

When a chat hits a sustained failure (user blocked the bot, chat deleted,
persistent rate-limit), further sends are suppressed until the cooldown
expires. This prevents the cognitive core's resumer from retrying forever
against a dead chat and prevents log spam.

Design decisions (from sprint integration, 2026-04-11):
  - SQLite over Redis: Harvey is single-process, deployment simplicity wins
  - TTL is category-dependent: BOT_BLOCKED=4h, CHAT_NOT_FOUND=24h, RATE_LIMITED=30s
  - Per (channel, chat_id) isolation — a block on one chat must not affect others
  - Clearing on success is automatic and instant
"""

from __future__ import annotations

import logging
import os
import sqlite3
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator, Optional

from .errors import ErrorCategory

log = logging.getLogger("harvey.chat.cooldowns")

DEFAULT_DB_PATH = os.path.join(
    os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
    "data",
    "chat",
    "channel_cooldowns.db",
)

# Per-category suppression windows
TTL_BY_CATEGORY: dict[ErrorCategory, float] = {
    ErrorCategory.BOT_BLOCKED: 4 * 3600,        # 4 hours
    ErrorCategory.CHAT_NOT_FOUND: 24 * 3600,    # 24 hours
    ErrorCategory.RATE_LIMITED: 30.0,           # 30 seconds
    ErrorCategory.FATAL: 3600,                  # 1 hour
}

DEFAULT_TTL = 60.0  # any other retryable — 1 minute


SCHEMA = """
CREATE TABLE IF NOT EXISTS cooldowns (
    channel      TEXT NOT NULL,
    chat_id      TEXT NOT NULL,
    category     TEXT NOT NULL,
    reason       TEXT,
    expires_at   REAL NOT NULL,
    created_at   REAL NOT NULL,
    PRIMARY KEY (channel, chat_id)
);
CREATE INDEX IF NOT EXISTS idx_cooldowns_expires ON cooldowns(expires_at);
"""


class ChannelErrorCooldown:
    def __init__(self, db_path: str = DEFAULT_DB_PATH):
        self.db_path = os.path.abspath(os.path.expanduser(db_path))
        Path(self.db_path).parent.mkdir(parents=True, exist_ok=True)
        self._local = threading.local()
        self._bootstrap()

    def _conn(self) -> sqlite3.Connection:
        conn = getattr(self._local, "conn", None)
        if conn is None:
            conn = sqlite3.connect(self.db_path, isolation_level=None, check_same_thread=False)
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("PRAGMA synchronous=NORMAL")
            conn.execute("PRAGMA busy_timeout=5000")
            self._local.conn = conn
        return conn

    def _bootstrap(self):
        self._conn().executescript(SCHEMA)

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

    # ─── Public API ─────────────────────────────────────────────

    def record_error(
        self,
        channel: str,
        chat_id: str,
        category: ErrorCategory,
        reason: str = "",
        now: Optional[float] = None,
    ) -> float:
        """Mark a chat as cooled-down. Returns expires_at timestamp."""
        ttl = TTL_BY_CATEGORY.get(category, DEFAULT_TTL)
        t = now if now is not None else time.time()
        expires = t + ttl
        with self._tx() as conn:
            conn.execute(
                """
                INSERT INTO cooldowns (channel, chat_id, category, reason, expires_at, created_at)
                VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT(channel, chat_id) DO UPDATE SET
                    category = excluded.category,
                    reason = excluded.reason,
                    expires_at = excluded.expires_at,
                    created_at = excluded.created_at
                """,
                (channel, chat_id, category.value, reason, expires, t),
            )
        log.info(
            f"Cooldown recorded: {channel}:{chat_id} category={category.value} "
            f"ttl={ttl:.0f}s reason={reason[:60]}"
        )
        return expires

    def record_success(self, channel: str, chat_id: str) -> bool:
        """Clear cooldown for a chat on successful send. Returns True if a cooldown was cleared."""
        with self._tx() as conn:
            cursor = conn.execute(
                "DELETE FROM cooldowns WHERE channel = ? AND chat_id = ?",
                (channel, chat_id),
            )
            return cursor.rowcount > 0

    def should_suppress(
        self,
        channel: str,
        chat_id: str,
        now: Optional[float] = None,
    ) -> bool:
        """Is this chat currently cooled down? Also lazy-evicts expired rows."""
        t = now if now is not None else time.time()
        row = self._conn().execute(
            "SELECT expires_at FROM cooldowns WHERE channel = ? AND chat_id = ?",
            (channel, chat_id),
        ).fetchone()
        if row is None:
            return False
        if row["expires_at"] <= t:
            # Lazy eviction
            with self._tx() as conn:
                conn.execute(
                    "DELETE FROM cooldowns WHERE channel = ? AND chat_id = ? AND expires_at <= ?",
                    (channel, chat_id, t),
                )
            return False
        return True

    def remaining(self, channel: str, chat_id: str, now: Optional[float] = None) -> float:
        """Seconds remaining on cooldown, or 0 if not cooled down."""
        t = now if now is not None else time.time()
        row = self._conn().execute(
            "SELECT expires_at FROM cooldowns WHERE channel = ? AND chat_id = ?",
            (channel, chat_id),
        ).fetchone()
        if row is None:
            return 0.0
        remaining = row["expires_at"] - t
        return max(0.0, remaining)

    def sweep_expired(self, now: Optional[float] = None) -> int:
        """Evict all expired cooldowns. Returns count evicted."""
        t = now if now is not None else time.time()
        with self._tx() as conn:
            cursor = conn.execute("DELETE FROM cooldowns WHERE expires_at <= ?", (t,))
            return cursor.rowcount
