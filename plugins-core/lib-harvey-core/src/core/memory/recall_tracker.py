"""
Recall Tracker — Closes Harvey's memory feedback loop.

Every Superbrain search records which documents were returned, creating
a recall_log that feeds the promotion pipeline. Non-blocking by design:
tracking failures never block or error the search.

Architecture:
    search() → _track_recalls() → RecallTracker.record() → recall_log table
    SANCHO promotion task → RecallTracker.rebuild_stats() → recall_stats table
    MemoryPromoter.rank_candidates() → reads recall_stats → scores → promotes

Inspired by OpenClaw Active Memory (short-term-promotion.ts).
Adapted for Harvey's SQLite-native Superbrain.
"""

import hashlib
import json
import logging
import os
import sqlite3
import threading
import time
from datetime import datetime
from typing import List, Optional

log = logging.getLogger("harvey.memory.recall_tracker")

from core.paths import harvey_home as _harvey_home

HARVEY_HOME = _harvey_home()
DB_PATH = os.path.join(HARVEY_HOME, "data", "superbrain.db")


class RecallTracker:
    """
    Non-blocking recall tracking. Buffers writes and flushes in batches.

    Every Superbrain search calls record() with the results returned.
    The recall_log grows unbounded but is cheap (one row per result per search).
    rebuild_stats() materializes aggregates into recall_stats for the promoter.
    """

    FLUSH_THRESHOLD = 10   # Flush every N queued records
    FLUSH_INTERVAL = 30.0  # Max seconds between flushes

    def __init__(self, db_path: str = None):
        self.db_path = db_path or DB_PATH
        self._queue: list = []
        self._lock = threading.Lock()
        self._last_flush = time.time()
        self._ensure_schema()

    def _ensure_schema(self) -> None:
        """Create recall tables if they don't exist."""
        try:
            conn = sqlite3.connect(self.db_path, check_same_thread=False)
            conn.executescript("""
                CREATE TABLE IF NOT EXISTS recall_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    doc_id INTEGER NOT NULL,
                    doc_path TEXT NOT NULL DEFAULT '',
                    content_hash TEXT NOT NULL,
                    query_hash TEXT NOT NULL,
                    score REAL NOT NULL DEFAULT 0.0,
                    source TEXT NOT NULL DEFAULT 'search',
                    recalled_at TEXT NOT NULL DEFAULT (datetime('now')),
                    recall_day TEXT NOT NULL DEFAULT (date('now'))
                );

                CREATE INDEX IF NOT EXISTS idx_recall_doc_id
                    ON recall_log(doc_id);
                CREATE INDEX IF NOT EXISTS idx_recall_content_hash
                    ON recall_log(content_hash);
                CREATE INDEX IF NOT EXISTS idx_recall_day
                    ON recall_log(recall_day);
                CREATE INDEX IF NOT EXISTS idx_recall_query_hash
                    ON recall_log(query_hash);

                CREATE TABLE IF NOT EXISTS recall_stats (
                    content_hash TEXT PRIMARY KEY,
                    doc_id INTEGER NOT NULL,
                    doc_path TEXT NOT NULL DEFAULT '',
                    snippet TEXT DEFAULT '',
                    recall_count INTEGER DEFAULT 0,
                    unique_queries INTEGER DEFAULT 0,
                    unique_days INTEGER DEFAULT 0,
                    total_score REAL DEFAULT 0.0,
                    max_score REAL DEFAULT 0.0,
                    first_recalled_at TEXT,
                    last_recalled_at TEXT,
                    consolidation_hits INTEGER DEFAULT 0,
                    promoted_at TEXT,
                    concept_tags TEXT DEFAULT '[]'
                );
            """)
            conn.commit()
            conn.close()
        except Exception as e:
            log.warning("recall_tracker: schema init failed: %s", e)

    # ─────────────────────────────────────────────────────────────
    #  Recording
    # ─────────────────────────────────────────────────────────────

    def record(self, results: List[dict], query: str, source: str = "search") -> None:
        """
        Record recall events for search results. Non-blocking.

        Args:
            results: Search result dicts (must have 'content'; optionally 'id', 'path', 'score')
            query: The search query that produced these results
            source: Origin — search | vector | anchor_search | context_load | consolidation
        """
        if not results or not query:
            return

        query_hash = self._hash(self._normalize(query))
        now = datetime.now()
        recalled_at = now.strftime("%Y-%m-%d %H:%M:%S")
        recall_day = now.strftime("%Y-%m-%d")

        with self._lock:
            for r in results:
                content = r.get("content", "") or ""
                snippet = content[:280]
                content_hash = self._hash(self._normalize(snippet))

                self._queue.append((
                    r.get("id", 0),
                    r.get("path", ""),
                    content_hash,
                    query_hash,
                    r.get("score", 0.0),
                    source,
                    recalled_at,
                    recall_day,
                ))

            should_flush = (
                len(self._queue) >= self.FLUSH_THRESHOLD
                or (time.time() - self._last_flush) > self.FLUSH_INTERVAL
            )

        if should_flush:
            self._flush()

    def record_consolidation_hit(self, content_hash: str) -> None:
        """
        Record that SANCHO dream/consolidation encountered this content.
        Increments consolidation_hits in recall_stats directly.
        """
        try:
            with sqlite3.connect(self.db_path, check_same_thread=False) as conn:
                conn.execute("""
                    UPDATE recall_stats
                    SET consolidation_hits = consolidation_hits + 1
                    WHERE content_hash = ?
                """, (content_hash,))
        except Exception as e:
            log.debug("consolidation_hit record failed: %s", e)

    def flush(self) -> None:
        """Public flush — call at end of session or before promotion."""
        self._flush()

    # ─────────────────────────────────────────────────────────────
    #  Stats Aggregation
    # ─────────────────────────────────────────────────────────────

    def rebuild_stats(self) -> int:
        """
        Rebuild recall_stats from recall_log. Called by promoter before scoring.

        Strategy:
        - GROUP BY content_hash to aggregate counts, scores, days
        - Preserve existing consolidation_hits and promoted_at (not in recall_log)
        - UPSERT: new entries get fresh stats, existing entries get updated

        Returns:
            Number of entries in recall_stats after rebuild.
        """
        try:
            with sqlite3.connect(self.db_path, check_same_thread=False) as conn:
                conn.row_factory = sqlite3.Row

                # Aggregate from recall_log
                rows = conn.execute("""
                    SELECT
                        content_hash,
                        MAX(doc_id) AS doc_id,
                        MAX(doc_path) AS doc_path,
                        COUNT(*) AS recall_count,
                        COUNT(DISTINCT query_hash) AS unique_queries,
                        COUNT(DISTINCT recall_day) AS unique_days,
                        SUM(score) AS total_score,
                        MAX(score) AS max_score,
                        MIN(recalled_at) AS first_recalled_at,
                        MAX(recalled_at) AS last_recalled_at
                    FROM recall_log
                    GROUP BY content_hash
                """).fetchall()

                for row in rows:
                    conn.execute("""
                        INSERT INTO recall_stats (
                            content_hash, doc_id, doc_path,
                            recall_count, unique_queries, unique_days,
                            total_score, max_score,
                            first_recalled_at, last_recalled_at
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        ON CONFLICT(content_hash) DO UPDATE SET
                            doc_id = excluded.doc_id,
                            doc_path = excluded.doc_path,
                            recall_count = excluded.recall_count,
                            unique_queries = excluded.unique_queries,
                            unique_days = excluded.unique_days,
                            total_score = excluded.total_score,
                            max_score = excluded.max_score,
                            first_recalled_at = COALESCE(
                                recall_stats.first_recalled_at, excluded.first_recalled_at
                            ),
                            last_recalled_at = excluded.last_recalled_at
                    """, (
                        row["content_hash"], row["doc_id"], row["doc_path"],
                        row["recall_count"], row["unique_queries"], row["unique_days"],
                        row["total_score"], row["max_score"],
                        row["first_recalled_at"], row["last_recalled_at"],
                    ))

                count = conn.execute("SELECT COUNT(*) AS c FROM recall_stats").fetchone()["c"]

            log.info("recall_stats rebuilt: %d entries from recall_log", count)
            return count

        except Exception as e:
            log.error("rebuild_stats failed: %s", e)
            return 0

    def get_stats(self) -> dict:
        """Return summary statistics for observability."""
        try:
            with sqlite3.connect(self.db_path, check_same_thread=False) as conn:
                conn.row_factory = sqlite3.Row

                log_count = conn.execute(
                    "SELECT COUNT(*) AS c FROM recall_log"
                ).fetchone()["c"]

                stats_count = conn.execute(
                    "SELECT COUNT(*) AS c FROM recall_stats"
                ).fetchone()["c"]

                promoted_count = conn.execute(
                    "SELECT COUNT(*) AS c FROM recall_stats WHERE promoted_at IS NOT NULL"
                ).fetchone()["c"]

                top_recalled = conn.execute("""
                    SELECT content_hash, doc_path, recall_count, unique_days, max_score
                    FROM recall_stats
                    ORDER BY recall_count DESC
                    LIMIT 5
                """).fetchall()

            return {
                "recall_log_entries": log_count,
                "recall_stats_entries": stats_count,
                "promoted_count": promoted_count,
                "top_recalled": [dict(r) for r in top_recalled],
            }
        except Exception as e:
            log.error("get_stats failed: %s", e)
            return {"error": str(e)}

    def prune_old_logs(self, max_age_days: int = 90) -> int:
        """Remove recall_log entries older than max_age_days. Returns deleted count."""
        try:
            with sqlite3.connect(self.db_path, check_same_thread=False) as conn:
                cursor = conn.execute("""
                    DELETE FROM recall_log
                    WHERE recall_day < date('now', ?)
                """, (f"-{max_age_days} days",))
                deleted = cursor.rowcount
                conn.execute("PRAGMA incremental_vacuum")
            log.info("pruned %d recall_log entries older than %d days", deleted, max_age_days)
            return deleted
        except Exception as e:
            log.error("prune_old_logs failed: %s", e)
            return 0

    # ─────────────────────────────────────────────────────────────
    #  Utilities
    # ─────────────────────────────────────────────────────────────

    @staticmethod
    def _normalize(text: str) -> str:
        """Lowercase, strip, collapse whitespace."""
        return " ".join(text.lower().split())

    @staticmethod
    def _hash(text: str) -> str:
        """SHA1 fingerprint, 12 hex chars (48 bits). Privacy-safe, collision-safe at <10K entries."""
        return hashlib.sha1(text.encode("utf-8", errors="replace")).hexdigest()[:12]

    def _flush(self) -> None:
        """Batch-write queued records to SQLite."""
        with self._lock:
            if not self._queue:
                return
            batch = list(self._queue)
            self._queue.clear()
            self._last_flush = time.time()

        try:
            with sqlite3.connect(self.db_path, check_same_thread=False) as conn:
                conn.executemany("""
                    INSERT INTO recall_log (
                        doc_id, doc_path, content_hash, query_hash,
                        score, source, recalled_at, recall_day
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                """, batch)
            log.debug("flushed %d recall records", len(batch))
        except Exception as e:
            log.warning("recall_tracker flush failed (%d records lost): %s", len(batch), e)
