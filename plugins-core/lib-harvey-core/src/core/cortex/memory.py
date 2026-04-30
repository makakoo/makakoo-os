"""Native SQLite-backed Cortex Memory for Makakoo HarveyChat."""

from __future__ import annotations

import json
import logging
import os
import re
import sqlite3
import time
import uuid
from pathlib import Path
from typing import Dict, List, Optional

from .config import CortexConfig
from .extractor import extract_memory_candidates
from .models import MemoryCandidate, MemorySource
from .scrubber import scrub_memory_text

log = logging.getLogger("harvey.cortex")

_TOKEN_RE = re.compile(r"[A-Za-z0-9_]{3,}")


def sanitize_fts_query(text: str) -> str:
    tokens = []
    seen = set()
    for token in _TOKEN_RE.findall((text or "").lower()):
        if token in seen:
            continue
        seen.add(token)
        tokens.append(token)
        if len(tokens) >= 12:
            break
    return " OR ".join(f"{token}*" for token in tokens)


def _now() -> float:
    return time.time()


class CortexMemory:
    def __init__(self, db_path: str, config: Optional[CortexConfig] = None):
        self.db_path = os.path.expanduser(db_path)
        self.config = config or CortexConfig(enabled=True)
        Path(self.db_path).parent.mkdir(parents=True, exist_ok=True)
        self._init_schema()

    def _connect(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self.db_path, timeout=5.0)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA busy_timeout=5000")
        return conn

    def _init_schema(self) -> None:
        with self._connect() as conn:
            conn.executescript(
                """
                CREATE TABLE IF NOT EXISTS cortex_user_aliases (
                    channel TEXT NOT NULL,
                    channel_user_id TEXT NOT NULL,
                    person_id TEXT NOT NULL,
                    label TEXT,
                    created_at REAL NOT NULL,
                    updated_at REAL NOT NULL,
                    PRIMARY KEY (channel, channel_user_id)
                );

                CREATE TABLE IF NOT EXISTS cortex_sessions (
                    id TEXT PRIMARY KEY,
                    person_id TEXT NOT NULL,
                    app_id TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    channel_user_id TEXT NOT NULL,
                    title TEXT,
                    summary TEXT,
                    message_count INTEGER NOT NULL DEFAULT 0,
                    active INTEGER NOT NULL DEFAULT 1,
                    created_at REAL NOT NULL,
                    updated_at REAL NOT NULL,
                    ended_at REAL
                );

                CREATE INDEX IF NOT EXISTS idx_cortex_sessions_active
                    ON cortex_sessions(person_id, app_id, channel, active, updated_at);

                CREATE TABLE IF NOT EXISTS cortex_memories (
                    id TEXT PRIMARY KEY,
                    person_id TEXT NOT NULL,
                    app_id TEXT NOT NULL,
                    memory_type TEXT NOT NULL,
                    content TEXT NOT NULL,
                    normalized_content TEXT NOT NULL,
                    confidence REAL NOT NULL DEFAULT 0.0,
                    importance_score REAL NOT NULL DEFAULT 0.5,
                    source_channel TEXT,
                    source_channel_user_id TEXT,
                    source_session_id TEXT,
                    source_message_id INTEGER,
                    access_count INTEGER NOT NULL DEFAULT 0,
                    created_at REAL NOT NULL,
                    updated_at REAL NOT NULL,
                    last_accessed REAL,
                    expires_at REAL,
                    metadata TEXT NOT NULL DEFAULT '{}'
                );

                CREATE INDEX IF NOT EXISTS idx_cortex_memories_person
                    ON cortex_memories(person_id, app_id, created_at DESC);
                CREATE UNIQUE INDEX IF NOT EXISTS idx_cortex_memories_dedupe
                    ON cortex_memories(person_id, app_id, normalized_content);

                CREATE VIRTUAL TABLE IF NOT EXISTS cortex_memories_fts USING fts5(
                    content,
                    memory_type UNINDEXED,
                    content='cortex_memories',
                    content_rowid='rowid'
                );

                CREATE TRIGGER IF NOT EXISTS cortex_memories_ai
                AFTER INSERT ON cortex_memories BEGIN
                    INSERT INTO cortex_memories_fts(rowid, content, memory_type)
                    VALUES (new.rowid, new.content, new.memory_type);
                END;

                CREATE TRIGGER IF NOT EXISTS cortex_memories_ad
                AFTER DELETE ON cortex_memories BEGIN
                    INSERT INTO cortex_memories_fts(cortex_memories_fts, rowid, content, memory_type)
                    VALUES ('delete', old.rowid, old.content, old.memory_type);
                END;

                CREATE TRIGGER IF NOT EXISTS cortex_memories_au
                AFTER UPDATE ON cortex_memories BEGIN
                    INSERT INTO cortex_memories_fts(cortex_memories_fts, rowid, content, memory_type)
                    VALUES ('delete', old.rowid, old.content, old.memory_type);
                    INSERT INTO cortex_memories_fts(rowid, content, memory_type)
                    VALUES (new.rowid, new.content, new.memory_type);
                END;
                """
            )
            conn.commit()

    def resolve_person_id(self, channel: str, channel_user_id: str) -> str:
        with self._connect() as conn:
            row = conn.execute(
                "SELECT person_id FROM cortex_user_aliases WHERE channel = ? AND channel_user_id = ?",
                (channel, str(channel_user_id)),
            ).fetchone()
            if row:
                return row["person_id"]
        return f"channel:{channel}:{channel_user_id}"

    def set_alias(self, channel: str, channel_user_id: str, person_id: str, label: str | None = None) -> None:
        now = _now()
        with self._connect() as conn:
            conn.execute(
                """INSERT INTO cortex_user_aliases (channel, channel_user_id, person_id, label, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?)
                   ON CONFLICT(channel, channel_user_id) DO UPDATE SET
                       person_id = excluded.person_id,
                       label = excluded.label,
                       updated_at = excluded.updated_at""",
                (channel, str(channel_user_id), person_id, label, now, now),
            )
            conn.commit()

    def get_or_create_session(self, channel: str, channel_user_id: str, username: str | None = None) -> str:
        person_id = self.resolve_person_id(channel, str(channel_user_id))
        with self._connect() as conn:
            row = conn.execute(
                """SELECT id FROM cortex_sessions
                   WHERE person_id = ? AND app_id = ? AND channel = ? AND active = 1
                   ORDER BY updated_at DESC LIMIT 1""",
                (person_id, self.config.app_id, channel),
            ).fetchone()
            if row:
                return row["id"]
            sid = str(uuid.uuid4())
            now = _now()
            conn.execute(
                """INSERT INTO cortex_sessions
                   (id, person_id, app_id, channel, channel_user_id, title, message_count, active, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, 0, 1, ?, ?)""",
                (sid, person_id, self.config.app_id, channel, str(channel_user_id), username, now, now),
            )
            conn.commit()
            return sid

    def end_session(self, channel: str, channel_user_id: str) -> None:
        person_id = self.resolve_person_id(channel, str(channel_user_id))
        now = _now()
        with self._connect() as conn:
            conn.execute(
                """UPDATE cortex_sessions SET active = 0, ended_at = ?, updated_at = ?
                   WHERE person_id = ? AND app_id = ? AND channel = ? AND active = 1""",
                (now, now, person_id, self.config.app_id, channel),
            )
            conn.commit()

    def increment_session_count(self, session_id: str, by: int = 1) -> None:
        with self._connect() as conn:
            conn.execute(
                "UPDATE cortex_sessions SET message_count = message_count + ?, updated_at = ? WHERE id = ?",
                (by, _now(), session_id),
            )
            conn.commit()

    @staticmethod
    def _normalize_content(content: str) -> str:
        return re.sub(r"\W+", " ", (content or "").lower()).strip()

    def _expires_at(self, candidate: MemoryCandidate) -> float | None:
        if candidate.memory_type in {"identity", "preference", "decision"} and candidate.importance_score >= 0.75:
            return None
        days = max(30, min(self.config.max_memory_age_days, int(candidate.importance_score * self.config.max_memory_age_days)))
        return _now() + days * 86400

    def create_memory(self, candidate: MemoryCandidate, source: MemorySource) -> str | None:
        if candidate.confidence < self.config.min_confidence or candidate.importance_score < self.config.min_importance:
            return None
        content = (candidate.content or "").strip()
        if not content:
            return None
        if len(content) > self.config.max_memory_chars:
            content = content[: self.config.max_memory_chars - 3].rstrip() + "..."

        scrubbed = scrub_memory_text(content, pii_enabled=self.config.pii_scrubbing)
        if not scrubbed.ok:
            log.warning("[cortex] scrubber rejected memory candidate: %s", scrubbed.reason)
            return None
        content = scrubbed.text.strip()
        normalized = self._normalize_content(content)
        if not normalized:
            return None

        person_id = self.resolve_person_id(source.channel, source.channel_user_id)
        mid = str(uuid.uuid4())
        now = _now()
        metadata = dict(candidate.metadata or {})
        metadata["scrubbed"] = scrubbed.changed
        metadata["scrubber"] = scrubbed.reason
        try:
            with self._connect() as conn:
                conn.execute(
                    """INSERT INTO cortex_memories
                       (id, person_id, app_id, memory_type, content, normalized_content,
                        confidence, importance_score, source_channel, source_channel_user_id,
                        source_session_id, source_message_id, created_at, updated_at, expires_at, metadata)
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                    (
                        mid,
                        person_id,
                        self.config.app_id,
                        candidate.memory_type,
                        content,
                        normalized,
                        candidate.confidence,
                        candidate.importance_score,
                        source.channel,
                        str(source.channel_user_id),
                        source.session_id,
                        source.source_message_id,
                        now,
                        now,
                        self._expires_at(candidate),
                        json.dumps(metadata),
                    ),
                )
                conn.commit()
            return mid
        except sqlite3.IntegrityError:
            return None

    def _prune_expired(self) -> None:
        with self._connect() as conn:
            conn.execute("DELETE FROM cortex_memories WHERE expires_at IS NOT NULL AND expires_at < ?", (_now(),))
            conn.commit()

    def search(self, query: str, channel: str, channel_user_id: str, limit: int | None = None) -> List[Dict]:
        self._prune_expired()
        fts_query = sanitize_fts_query(query)
        if not fts_query:
            return []
        person_id = self.resolve_person_id(channel, str(channel_user_id))
        limit = limit or self.config.memory_limit
        now = _now()
        with self._connect() as conn:
            rows = conn.execute(
                """SELECT m.id, m.memory_type, m.content, m.confidence, m.importance_score,
                          m.created_at, m.last_accessed, m.access_count,
                          bm25(cortex_memories_fts) AS rank
                   FROM cortex_memories_fts
                   JOIN cortex_memories m ON m.rowid = cortex_memories_fts.rowid
                   WHERE m.person_id = ? AND m.app_id = ? AND cortex_memories_fts MATCH ?
                   ORDER BY rank ASC, m.importance_score DESC, m.created_at DESC
                   LIMIT ?""",
                (person_id, self.config.app_id, fts_query, limit),
            ).fetchall()
            for row in rows:
                conn.execute(
                    "UPDATE cortex_memories SET access_count = access_count + 1, last_accessed = ? WHERE id = ?",
                    (now, row["id"]),
                )
            conn.commit()
            return [dict(row) for row in rows]

    def delete_memory(self, memory_id: str, person_id: str | None = None) -> bool:
        with self._connect() as conn:
            if person_id:
                cur = conn.execute("DELETE FROM cortex_memories WHERE id = ? AND person_id = ?", (memory_id, person_id))
            else:
                cur = conn.execute("DELETE FROM cortex_memories WHERE id = ?", (memory_id,))
            conn.commit()
            return cur.rowcount > 0

    def delete_person_memories(self, person_id: str) -> int:
        with self._connect() as conn:
            cur = conn.execute("DELETE FROM cortex_memories WHERE person_id = ?", (person_id,))
            conn.commit()
            return cur.rowcount

    def record_turn(
        self,
        *,
        channel: str,
        channel_user_id: str,
        username: str | None,
        session_id: str | None,
        user_text: str,
        assistant_text: str,
        source_message_id: int | None = None,
    ) -> List[str]:
        source = MemorySource(
            channel=channel,
            channel_user_id=str(channel_user_id),
            session_id=session_id,
            source_message_id=source_message_id,
        )
        ids: List[str] = []
        for candidate in extract_memory_candidates(user_text, assistant_text):
            mid = self.create_memory(candidate, source)
            if mid:
                ids.append(mid)
        return ids


def get_cortex_memory(db_path: str, config: CortexConfig | None = None) -> CortexMemory | None:
    cfg = config or CortexConfig().apply_env()
    # If config came from file, env still overrides.
    cfg.apply_env()
    if not cfg.enabled:
        return None
    return CortexMemory(db_path, cfg)
