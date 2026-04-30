# 02 — Cortex Module Design

## File: `core/cortex/memory.py`

Native Python module using SQLite directly. No HTTP. No Docker.

```python
"""Cortex Memory — native SQLite-backed memory for Makakoo agents."""

import logging
import os
import sqlite3
import uuid
from datetime import datetime, timedelta
from typing import Dict, List, Optional

log = logging.getLogger("harvey.cortex")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
DEFAULT_DB = os.path.join(HARVEY_HOME, "data", "chat", "store.db")


class CortexMemory:
    """SQLite-backed memory store with FTS5 search, temporal decay, and PII scrubbing."""

    def __init__(self, db_path: str = DEFAULT_DB, config: Optional["CortexConfig"] = None):
        self.db_path = db_path
        self.config = config or CortexConfig()
        self._session_cache: Dict[str, str] = {}  # "channel:user_id" → session_id
        self._init_schema()

    # ── Schema ───────────────────────────────────────────────

    def _init_schema(self) -> None:
        with sqlite3.connect(self.db_path) as conn:
            conn.executescript("""
                CREATE TABLE IF NOT EXISTS cortex_sessions (
                    id TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL,
                    app_id TEXT NOT NULL,
                    title TEXT,
                    summary TEXT,
                    message_count INTEGER DEFAULT 0,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    UNIQUE(user_id, app_id)
                );

                CREATE TABLE IF NOT EXISTS cortex_memories (
                    id TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL,
                    app_id TEXT NOT NULL,
                    content TEXT NOT NULL,
                    importance_score REAL DEFAULT 0.5,
                    access_count INTEGER DEFAULT 0,
                    last_accessed TIMESTAMP,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    expires_at TIMESTAMP
                );

                CREATE VIRTUAL TABLE IF NOT EXISTS cortex_memories_fts USING fts5(
                    content,
                    content='cortex_memories',
                    content_rowid='rowid'
                );

                CREATE TRIGGER IF NOT EXISTS cortex_memories_ai AFTER INSERT ON cortex_memories BEGIN
                    INSERT INTO cortex_memories_fts(rowid, content) VALUES (new.rowid, new.content);
                END;

                CREATE TRIGGER IF NOT EXISTS cortex_memories_ad AFTER DELETE ON cortex_memories BEGIN
                    INSERT INTO cortex_memories_fts(cortex_memories_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
                END;

                CREATE TRIGGER IF NOT EXISTS cortex_memories_au AFTER UPDATE ON cortex_memories BEGIN
                    INSERT INTO cortex_memories_fts(cortex_memories_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
                    INSERT INTO cortex_memories_fts(rowid, content) VALUES (new.rowid, new.content);
                END;
            """)

    # ── Sessions ─────────────────────────────────────────────

    def get_or_create_session(self, channel: str, user_id: str, title: Optional[str] = None) -> str:
        """Get cached session or create new one."""
        key = f"{channel}:{user_id}"
        if key in self._session_cache:
            return self._session_cache[key]

        # Check DB
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.execute(
                "SELECT id FROM cortex_sessions WHERE user_id = ? AND app_id = ?",
                (self._cortex_user_id(channel, user_id), f"makakoo-{channel}")
            )
            row = cursor.fetchone()
            if row:
                self._session_cache[key] = row[0]
                return row[0]

            # Create new
            session_id = str(uuid.uuid4())
            conn.execute(
                "INSERT INTO cortex_sessions (id, user_id, app_id, title) VALUES (?, ?, ?, ?)",
                (session_id, self._cortex_user_id(channel, user_id), f"makakoo-{channel}", title)
            )
            conn.commit()
            self._session_cache[key] = session_id
            return session_id

    def add_message(self, session_id: str, role: str, content: str) -> None:
        """Store message in session and increment count."""
        with sqlite3.connect(self.db_path) as conn:
            conn.execute(
                "UPDATE cortex_sessions SET message_count = message_count + 1, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
                (session_id,)
            )
            conn.commit()

    def get_session_messages(self, session_id: str, limit: int = 50) -> List[Dict]:
        """Return messages from ChatStore for this session's user/channel."""
        # Session just tracks metadata; actual messages are in ChatStore
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.execute(
                "SELECT user_id, app_id FROM cortex_sessions WHERE id = ?",
                (session_id,)
            )
            row = cursor.fetchone()
            if not row:
                return []
            # Parse user_id back to channel:user_id
            # For full messages, delegate to ChatStore
            return []

    def get_session_summary(self, session_id: str) -> Optional[str]:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.execute(
                "SELECT summary FROM cortex_sessions WHERE id = ?",
                (session_id,)
            )
            row = cursor.fetchone()
            return row[0] if row else None

    def set_session_summary(self, session_id: str, summary: str) -> None:
        with sqlite3.connect(self.db_path) as conn:
            conn.execute(
                "UPDATE cortex_sessions SET summary = ? WHERE id = ?",
                (summary, session_id)
            )
            conn.commit()

    # ── Memories ─────────────────────────────────────────────

    def create_memory(self, content: str, channel: str, user_id: str, importance: float = 0.5) -> str:
        """Store a long-term memory with temporal decay."""
        memory_id = str(uuid.uuid4())
        
        # PII scrubbing
        if self.config.pii_scrubbing:
            content = self._scrub(content)

        # Calculate expiration based on importance
        days = max(1, int(importance * 365))
        expires_at = datetime.now() + timedelta(days=days)

        with sqlite3.connect(self.db_path) as conn:
            conn.execute(
                """INSERT INTO cortex_memories
                   (id, user_id, app_id, content, importance_score, expires_at)
                   VALUES (?, ?, ?, ?, ?, ?)""",
                (memory_id, self._cortex_user_id(channel, user_id), f"makakoo-{channel}",
                 content, importance, expires_at.isoformat())
            )
            conn.commit()
        return memory_id

    def search(self, query: str, channel: str, user_id: str, limit: int = 5) -> List[Dict]:
        """Search memories via FTS5."""
        # Prune expired first
        self._prune_expired()

        with sqlite3.connect(self.db_path) as conn:
            conn.row_factory = sqlite3.Row
            cursor = conn.execute(
                """SELECT m.id, m.content, m.importance_score, m.created_at, m.last_accessed
                   FROM cortex_memories m
                   JOIN cortex_memories_fts fts ON m.rowid = fts.rowid
                   WHERE m.user_id = ? AND m.app_id = ?
                   AND cortex_memories_fts MATCH ?
                   ORDER BY rank, m.importance_score DESC
                   LIMIT ?""",
                (self._cortex_user_id(channel, user_id), f"makakoo-{channel}", query, limit)
            )
            rows = cursor.fetchall()
            
            # Update access counts
            for row in rows:
                conn.execute(
                    "UPDATE cortex_memories SET access_count = access_count + 1, last_accessed = CURRENT_TIMESTAMP WHERE id = ?",
                    (row["id"],)
                )
            conn.commit()
            
            return [dict(r) for r in rows]

    def delete_user_memories(self, channel: str, user_id: str) -> int:
        """GDPR delete all memories for a user."""
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.execute(
                "DELETE FROM cortex_memories WHERE user_id = ? AND app_id = ?",
                (self._cortex_user_id(channel, user_id), f"makakoo-{channel}")
            )
            conn.commit()
            return cursor.rowcount

    # ── Maintenance ──────────────────────────────────────────

    def _prune_expired(self) -> None:
        """Remove memories past their expiration date."""
        with sqlite3.connect(self.db_path) as conn:
            conn.execute(
                "DELETE FROM cortex_memories WHERE expires_at < datetime('now')"
            )
            conn.commit()

    # ── PII Scrubbing ────────────────────────────────────────

    def _scrub(self, text: str) -> str:
        """Remove PII using Presidio. Lazy-load models."""
        try:
            from presidio_analyzer import AnalyzerEngine
            from presidio_anonymizer import AnonymizerEngine
        except ImportError:
            log.warning("Presidio not installed, skipping PII scrubbing")
            return text

        if not hasattr(self, "_analyzer"):
            self._analyzer = AnalyzerEngine()
            self._anonymizer = AnonymizerEngine()

        results = self._analyzer.analyze(text=text, language="en")
        if not results:
            return text

        return self._anonymizer.anonymize(text=text, analyzer_results=results).text

    # ── Helpers ──────────────────────────────────────────────

    @staticmethod
    def _cortex_user_id(channel: str, user_id: str) -> str:
        return f"{channel}:{user_id}"


# ── Config ─────────────────────────────────────────────────

class CortexConfig:
    def __init__(
        self,
        enabled: bool = False,
        memory_limit: int = 5,
        auto_summarize_after: int = 4,
        pii_scrubbing: bool = True,
        temporal_decay: float = 0.05,
        min_importance: float = 0.3,
        max_memory_age_days: int = 365,
    ):
        self.enabled = enabled
        self.memory_limit = memory_limit
        self.auto_summarize_after = auto_summarize_after
        self.pii_scrubbing = pii_scrubbing
        self.temporal_decay = temporal_decay
        self.min_importance = min_importance
        self.max_memory_age_days = max_memory_age_days

    @classmethod
    def from_env(cls) -> "CortexConfig":
        import os
        return cls(
            enabled=os.environ.get("MAKAKOO_CORTEX_ENABLED", "0") == "1",
            memory_limit=int(os.environ.get("MAKAKOO_CORTEX_MEMORY_LIMIT", "5")),
            auto_summarize_after=int(os.environ.get("MAKAKOO_CORTEX_SUMMARIZE_AFTER", "4")),
            pii_scrubbing=os.environ.get("MAKAKOO_CORTEX_PII", "1") == "1",
        )


# ── Lazy singleton ─────────────────────────────────────────

_memory_instance: Optional[CortexMemory] = None


def get_cortex_memory(db_path: str = DEFAULT_DB) -> Optional[CortexMemory]:
    global _memory_instance
    if _memory_instance is None:
        cfg = CortexConfig.from_env()
        if not cfg.enabled:
            return None
        _memory_instance = CortexMemory(db_path, cfg)
        log.info("Cortex memory initialized: %s", db_path)
    return _memory_instance
```

## File: `core/cortex/summarizer.py`

```python
"""Auto-summarize sessions via LLM."""

import logging
from typing import List

log = logging.getLogger("harvey.cortex")


def summarize_session(messages: List[str], max_tokens: int = 100) -> str:
    """Summarize a list of messages into 2-3 sentences."""
    from core.llm import switchAILocal

    text = "\n".join(f"- {m}" for m in messages[-20:])  # last 20 messages
    prompt = (
        "Summarize the following conversation in 2-3 sentences. "
        "Focus on key decisions, facts, and user preferences:\n\n"
        f"{text}\n\nSummary:"
    )

    try:
        response = switchAILocal.complete(
            model="ail-fast",
            messages=[{"role": "user", "content": prompt}],
            max_tokens=max_tokens,
        )
        return response.choices[0].message.content.strip()
    except Exception as e:
        log.warning("Session summarization failed: %s", e)
        return ""
```

## File: `core/cortex/__init__.py`

```python
from .memory import CortexMemory, CortexConfig, get_cortex_memory
from .summarizer import summarize_session

__all__ = ["CortexMemory", "CortexConfig", "get_cortex_memory", "summarize_session"]
```
