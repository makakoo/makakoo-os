#!/usr/bin/env python3
"""
⚠️ DUAL-WRITE — Rust equivalent at makakoo-os/makakoo-core/src/superbrain/
reads the same superbrain.db file. Both are active. Python is the daily-driver
CLI (`superbrain search/query/remember`). Rust is used by the makakoo kernel
(SANCHO, capability handlers). Schema is shared — do not change column names
without updating both.

Superbrain Store — SQLite FTS5 + vector storage. Zero external dependencies.

Single database at ~/MAKAKOO/data/superbrain.db handles:
- FTS5 full-text search (instant keyword search over Brain)
- Vector blobs with numpy cosine similarity (semantic search)
- Entity graph with temporal validity windows
- Content hash tracking for incremental sync

Replaces: Qdrant dependency, PostgreSQL dependency, file-scanning TF-IDF.
At 444 Brain files, brute-force cosine is sub-100ms. No HNSW needed.
"""

import hashlib
import json
import logging
import os
import re
import sqlite3
import struct
import time
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple

log = logging.getLogger("superbrain.store")

from core.paths import harvey_home as _harvey_home

HARVEY_HOME = _harvey_home()
DB_PATH = os.path.join(HARVEY_HOME, "data", "superbrain.db")
BRAIN_DIR = os.path.join(HARVEY_HOME, "data", "Brain")
AUTO_MEMORY_DIR = os.path.join(HARVEY_HOME, "data", "auto-memory")
WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")

# Anchor extraction is best-effort — wrapped so the write path never fails
# on extractor errors. Gated by BRAIN_ANCHOR_ON_SYNC env (default "1").
# See harvey-os/skills/meta/brain-anchors/SKILL.md.
try:
    from core.superbrain.anchor_extractor import extract_anchor_safe as _extract_anchor_safe
except Exception:  # pragma: no cover — import-time safety
    _extract_anchor_safe = None


# ─────────────────────────────────────────────────────────────────────────────
#  Vector serialization (compact binary, no pickle)
# ─────────────────────────────────────────────────────────────────────────────

def _pack_vector(vec: List[float]) -> bytes:
    """Pack float list to compact binary (4 bytes per float)."""
    return struct.pack(f"{len(vec)}f", *vec)


def _unpack_vector(blob: bytes) -> List[float]:
    """Unpack binary blob to float list."""
    n = len(blob) // 4
    return list(struct.unpack(f"{n}f", blob))


def _cosine_similarity(a: List[float], b: List[float]) -> float:
    """Cosine similarity between two vectors.

    Dim guard: zip() silently truncates to the shorter input. A mismatch
    between a 1024-d query and a 3072-d stored vector would compute a
    meaningless number over the first 1024 dims and the caller would rank
    garbage. Return 0.0 on mismatch so the result is excluded, not poisoned.
    """
    if len(a) != len(b):
        return 0.0
    dot = sum(x * y for x, y in zip(a, b))
    norm_a = sum(x * x for x in a) ** 0.5
    norm_b = sum(x * x for x in b) ** 0.5
    if norm_a == 0 or norm_b == 0:
        return 0.0
    return dot / (norm_a * norm_b)


# ─────────────────────────────────────────────────────────────────────────────
#  Store
# ─────────────────────────────────────────────────────────────────────────────

class SuperbrainStore:
    """
    SQLite-backed knowledge store with FTS5 + vector search.

    Usage:
        store = SuperbrainStore()
        store.sync_brain()                          # index all Brain files
        results = store.search("polymarket")        # FTS5 keyword search
        results = store.vector_search(embedding)    # cosine similarity search
        store.close()
    """

    def __init__(self, db_path: str = None, brain_dir: str = None):
        self.db_path = db_path or DB_PATH
        self.brain_dir = brain_dir or BRAIN_DIR
        os.makedirs(os.path.dirname(self.db_path), exist_ok=True)
        self._conn = sqlite3.connect(self.db_path, check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._init_schema()

    def _init_schema(self):
        """Create tables if they don't exist."""
        self._conn.executescript("""
            -- Brain documents (source of truth)
            CREATE TABLE IF NOT EXISTS brain_docs (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                name TEXT NOT NULL,
                doc_type TEXT NOT NULL,  -- 'page' or 'journal'
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                entities TEXT DEFAULT '[]',  -- JSON array of [[wikilink]] targets
                char_count INTEGER DEFAULT 0,
                updated_at TEXT DEFAULT (datetime('now'))
            );

            -- FTS5 full-text index (instant keyword search)
            CREATE VIRTUAL TABLE IF NOT EXISTS brain_fts USING fts5(
                name, content, entities,
                content=brain_docs,
                content_rowid=id,
                tokenize='porter unicode61'
            );

            -- Triggers to keep FTS in sync
            CREATE TRIGGER IF NOT EXISTS brain_docs_ai AFTER INSERT ON brain_docs BEGIN
                INSERT INTO brain_fts(rowid, name, content, entities)
                VALUES (new.id, new.name, new.content, new.entities);
            END;

            CREATE TRIGGER IF NOT EXISTS brain_docs_ad AFTER DELETE ON brain_docs BEGIN
                INSERT INTO brain_fts(brain_fts, rowid, name, content, entities)
                VALUES ('delete', old.id, old.name, old.content, old.entities);
            END;

            CREATE TRIGGER IF NOT EXISTS brain_docs_au AFTER UPDATE ON brain_docs BEGIN
                INSERT INTO brain_fts(brain_fts, rowid, name, content, entities)
                VALUES ('delete', old.id, old.name, old.content, old.entities);
                INSERT INTO brain_fts(rowid, name, content, entities)
                VALUES (new.id, new.name, new.content, new.entities);
            END;

            -- Vector embeddings (stored as binary blobs)
            CREATE TABLE IF NOT EXISTS brain_vectors (
                doc_id INTEGER PRIMARY KEY REFERENCES brain_docs(id),
                embedding BLOB NOT NULL,
                dim INTEGER NOT NULL,
                model TEXT DEFAULT 'unknown',
                created_at TEXT DEFAULT (datetime('now'))
            );

            -- Entity graph with temporal validity (inspired by MemPalace)
            CREATE TABLE IF NOT EXISTS entity_graph (
                id INTEGER PRIMARY KEY,
                subject TEXT NOT NULL,
                predicate TEXT NOT NULL,
                object TEXT NOT NULL,
                valid_from TEXT,  -- ISO date or NULL (always valid)
                valid_to TEXT,    -- ISO date or NULL (still valid)
                confidence REAL DEFAULT 1.0,
                source TEXT,      -- which doc/journal produced this triple
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS eg_subject ON entity_graph(subject);
            CREATE INDEX IF NOT EXISTS eg_object ON entity_graph(object);

            -- Events log (replaces PostgreSQL events table)
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY,
                event_type TEXT NOT NULL,
                agent TEXT NOT NULL,
                summary TEXT NOT NULL,
                details TEXT DEFAULT '{}',
                occurred_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS events_type ON events(event_type, occurred_at);

            -- Cache for precomputed layers
            CREATE TABLE IF NOT EXISTS cache (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                expires_at TEXT
            );

            -- Recall tracking (Active Memory feedback loop)
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

            -- Materialized recall stats (rebuilt by promoter)
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
        self._conn.commit()

    # ═══════════════════════════════════════════════════════════════
    #  BRAIN SYNC — Index all Brain files into FTS5
    # ═══════════════════════════════════════════════════════════════

    def sync_brain(self, force: bool = False) -> dict:
        """
        Index all Brain pages, journals, and auto-memory entries into SQLite FTS5.

        Incremental: skips files whose content_hash hasn't changed.
        Returns: {"pages": N, "journals": N, "memories": N, "skipped": N, "removed": N}

        v4.1: auto-memory entries at ~/MAKAKOO/data/auto-memory/*.md are now
        indexed alongside Brain pages/journals as doc_type="memory". This
        closes the gap where memories written via any CLI (Claude Code Write
        tool, Gemini fs.writeFile, bash redirect) were invisible to
        `superbrain search` because sync only scanned brain/ subtrees.
        """
        stats = {"pages": 0, "journals": 0, "memories": 0, "skipped": 0, "removed": 0, "errors": 0}
        brain = Path(self.brain_dir)

        # Load existing hashes for incremental sync
        existing = {}
        if not force:
            for row in self._conn.execute("SELECT path, content_hash FROM brain_docs"):
                existing[row["path"]] = row["content_hash"]

        seen_paths = set()

        # Sync pages
        pages_dir = brain / "pages"
        if pages_dir.exists():
            for f in pages_dir.glob("*.md"):
                result = self._sync_file(f, "page", existing, force)
                stats[result] += 1
                seen_paths.add(str(f))

        # Sync journals
        journals_dir = brain / "journals"
        if journals_dir.exists():
            for f in journals_dir.glob("*.md"):
                result = self._sync_file(f, "journal", existing, force)
                stats[result] += 1
                seen_paths.add(str(f))

        # Sync auto-memory (v4.1 — cross-CLI shared durable memories)
        memory_dir = Path(AUTO_MEMORY_DIR)
        if memory_dir.exists():
            for f in memory_dir.glob("*.md"):
                # MEMORY.md is an index file, not a memory entry — skip it
                if f.name == "MEMORY.md":
                    continue
                result = self._sync_file(f, "memory", existing, force)
                stats[result] += 1
                seen_paths.add(str(f))

        # Remove docs for deleted files
        for path in existing:
            if path not in seen_paths:
                self._conn.execute("DELETE FROM brain_docs WHERE path = ?", (path,))
                stats["removed"] += 1

        self._conn.commit()
        total = stats["pages"] + stats["journals"] + stats["memories"]
        log.info("Brain sync: %d indexed (%d pages, %d journals, %d memories), %d skipped, %d removed",
                 total, stats["pages"], stats["journals"], stats["memories"],
                 stats["skipped"], stats["removed"])
        return stats

    def _sync_file(self, file_path: Path, doc_type: str, existing: dict, force: bool) -> str:
        """Index a single Brain file. Returns stat key."""
        try:
            content = file_path.read_text(encoding="utf-8", errors="replace")
        except Exception:
            return "errors"

        if len(content.strip()) < 20:
            return "skipped"

        c_hash = hashlib.sha256(content.encode()).hexdigest()
        path_str = str(file_path)

        if not force and existing.get(path_str) == c_hash:
            return "skipped"

        name = file_path.stem
        entities = json.dumps(list(set(WIKILINK_RE.findall(content))))

        self._conn.execute("""
            INSERT INTO brain_docs (path, name, doc_type, content, content_hash, entities, char_count, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'))
            ON CONFLICT(path) DO UPDATE SET
                content = excluded.content,
                content_hash = excluded.content_hash,
                entities = excluded.entities,
                char_count = excluded.char_count,
                updated_at = excluded.updated_at
        """, (path_str, name, doc_type, content, c_hash, entities, len(content)))

        # Anchor extraction — best-effort, gated by env. Runs only for
        # new/changed content because unchanged rows returned "skipped"
        # above via the content_hash comparison. Failures are logged and
        # leave anchor=NULL; backfill_anchors.py can retry later.
        if os.environ.get("BRAIN_ANCHOR_ON_SYNC", "1") == "1" and _extract_anchor_safe is not None:
            try:
                result = _extract_anchor_safe(name, content, doc_type)
            except Exception as e:  # defense — extract_anchor_safe should not raise
                log.warning("anchor extraction crashed for %s: %s", path_str, e)
                result = None

            if result is not None:
                try:
                    self._conn.execute("""
                        UPDATE brain_docs SET
                            anchor = ?,
                            anchor_level = ?,
                            anchor_hash = ?,
                            anchor_keywords = ?,
                            anchor_entities = ?,
                            anchor_generated_at = datetime('now'),
                            anchor_model = ?
                        WHERE path = ?
                    """, (
                        result["anchor"],
                        result.get("anchor_level", "atomic"),
                        result["anchor_hash"],
                        json.dumps(result.get("keywords", [])),
                        json.dumps(result.get("entities", [])),
                        result.get("anchor_model", "unknown"),
                        path_str,
                    ))
                except sqlite3.OperationalError as e:
                    # Schema missing anchor columns — migration not run yet.
                    # Log once at WARNING so Sebastian sees it but don't spam.
                    log.warning("anchor UPDATE failed (migration may be pending): %s", e)

        return "pages" if doc_type == "page" else "journals"

    # ═══════════════════════════════════════════════════════════════
    #  ANCHOR SEARCH — FTS5 over compressed anchors (Phase D)
    # ═══════════════════════════════════════════════════════════════

    def search_anchors(self, query: str, top_k: int = 10, doc_type: str = None) -> List[dict]:
        """
        FTS5 keyword search over brain_anchors_fts.

        Returns the compressed anchor (~100-300 chars) instead of full
        content. Matches the same shape as search() so callers can treat
        anchor hits and full-content hits interchangeably, but with a
        much smaller text payload.

        Rows without an anchor (anchor IS NULL) are excluded.

        Returns:
            List of {id, name, doc_type, anchor, anchor_level, anchor_entities,
                     anchor_keywords, score, entities, path, content (anchor)}.
            The `content` key is set to the anchor so existing consumers of
            `r["content"]` continue to work transparently.
        """
        fts_query = self._to_fts5_query(query)
        if not fts_query:
            return []

        type_filter = "AND d.doc_type = ?" if doc_type else ""
        params = [fts_query] + ([doc_type] if doc_type else []) + [top_k]

        try:
            rows = self._conn.execute(f"""
                SELECT d.id, d.name, d.doc_type, d.anchor, d.anchor_level,
                       d.anchor_keywords, d.anchor_entities, d.entities, d.path,
                       bm25(brain_anchors_fts, 3.0, 1.5, 2.0) AS score
                FROM brain_anchors_fts f
                JOIN brain_docs d ON f.rowid = d.id
                WHERE brain_anchors_fts MATCH ?
                  AND d.anchor IS NOT NULL
                {type_filter}
                ORDER BY score
                LIMIT ?
            """, params).fetchall()
        except sqlite3.OperationalError as e:
            log.warning("brain_anchors_fts query failed for '%s': %s", fts_query, e)
            return []

        results = []
        for row in rows:
            score = -row["score"]  # BM25 returns negative; flip so higher=better
            try:
                anchor_entities = json.loads(row["anchor_entities"]) if row["anchor_entities"] else []
            except (json.JSONDecodeError, TypeError):
                anchor_entities = []
            try:
                anchor_keywords = json.loads(row["anchor_keywords"]) if row["anchor_keywords"] else []
            except (json.JSONDecodeError, TypeError):
                anchor_keywords = []
            try:
                entities = json.loads(row["entities"]) if row["entities"] else []
            except (json.JSONDecodeError, TypeError):
                entities = []
            results.append({
                "id": row["id"],
                "name": row["name"],
                "doc_type": row["doc_type"],
                "anchor": row["anchor"],
                "anchor_level": row["anchor_level"],
                "anchor_entities": anchor_entities,
                "anchor_keywords": anchor_keywords,
                "score": score,
                "entities": entities,
                "path": row["path"],
                # Mirror anchor into `content` so downstream merge/synthesis
                # code that reads r["content"] stays compatible.
                "content": row["anchor"],
            })
        self._track_recalls(results, query, source="anchor_search")
        return results

    def get_doc_by_id(self, doc_id: int) -> Optional[dict]:
        """
        Fetch a full brain_docs row by id. Used for on-demand anchor
        expansion (Phase D read path). Returns None if no row matches.
        """
        row = self._conn.execute(
            "SELECT id, name, doc_type, content, entities, path, anchor, "
            "anchor_level, anchor_keywords, anchor_entities, anchor_generated_at, "
            "anchor_model, updated_at "
            "FROM brain_docs WHERE id = ?",
            (doc_id,),
        ).fetchone()
        if row is None:
            return None
        try:
            entities = json.loads(row["entities"]) if row["entities"] else []
        except (json.JSONDecodeError, TypeError):
            entities = []
        return {
            "id": row["id"],
            "name": row["name"],
            "doc_type": row["doc_type"],
            "content": row["content"],
            "entities": entities,
            "path": row["path"],
            "anchor": row["anchor"],
            "anchor_level": row["anchor_level"],
            "anchor_model": row["anchor_model"],
            "anchor_generated_at": row["anchor_generated_at"],
            "updated_at": row["updated_at"],
        }

    def anchors_count(self) -> tuple[int, int]:
        """Return (anchored_count, total_count) for observability."""
        try:
            anchored = self._conn.execute(
                "SELECT count(*) FROM brain_docs WHERE anchor IS NOT NULL"
            ).fetchone()[0]
            total = self._conn.execute("SELECT count(*) FROM brain_docs").fetchone()[0]
            return anchored, total
        except sqlite3.OperationalError:
            return 0, 0

    # ═══════════════════════════════════════════════════════════════
    #  ANCHOR VECTOR SEARCH — Phase D2
    # ═══════════════════════════════════════════════════════════════

    def embed_and_store_anchor(self, doc_id: int, anchor_text: str,
                               anchor_hash: str = None) -> bool:
        """
        Compute an embedding for an anchor and persist it in
        brain_anchor_vectors. Returns True on success, False on skip/error.

        Safe to call repeatedly — uses UPSERT semantics via
        ON CONFLICT DO UPDATE, so re-running on already-embedded docs
        replaces the vector (good when the anchor text changes).
        """
        if not anchor_text or len(anchor_text.strip()) < 10:
            return False
        # Import lazily so store.py stays cheap to load when the
        # embedder isn't needed.
        try:
            from core.superbrain.embeddings import embed_text, CURRENT_MODEL
        except Exception as e:
            log.warning("embeddings module not importable: %s", e)
            return False

        vec = embed_text(anchor_text)
        if not vec:
            return False

        try:
            blob = _pack_vector(vec)
            self._conn.execute("""
                INSERT INTO brain_anchor_vectors (doc_id, embedding, dim, model, anchor_hash, created_at)
                VALUES (?, ?, ?, ?, ?, datetime('now'))
                ON CONFLICT(doc_id) DO UPDATE SET
                    embedding = excluded.embedding,
                    dim = excluded.dim,
                    model = excluded.model,
                    anchor_hash = excluded.anchor_hash,
                    created_at = excluded.created_at
            """, (doc_id, blob, len(vec), CURRENT_MODEL, anchor_hash))
            return True
        except sqlite3.OperationalError as e:
            log.warning("anchor vector UPSERT failed (table missing?): %s", e)
            return False

    def vector_search_anchors(self, query_vec: List[float], top_k: int = 10,
                              doc_type: str = None, min_sim: float = 0.3) -> List[dict]:
        """
        Brute-force cosine similarity over brain_anchor_vectors.

        Returns hits in the same shape as vector_search() but the
        `content` field carries the compressed anchor text, not the
        full passage. Phase D2 replaces vector_search() in the anchored
        read path.

        Dim-guarded: zero-out mismatched dims (same approach as
        vector_search) to handle a potential model swap without
        poisoning rankings.
        """
        try:
            rows = self._conn.execute("""
                SELECT v.doc_id, v.embedding, v.dim, v.model,
                       d.name, d.doc_type, d.anchor, d.entities, d.path,
                       d.anchor_keywords, d.anchor_entities
                FROM brain_anchor_vectors v
                JOIN brain_docs d ON d.id = v.doc_id
                WHERE d.anchor IS NOT NULL
            """).fetchall()
        except sqlite3.OperationalError as e:
            log.warning("vector_search_anchors failed (table missing?): %s", e)
            return []

        results = []
        for row in rows:
            if doc_type and row["doc_type"] != doc_type:
                continue
            stored_vec = _unpack_vector(row["embedding"])
            sim = _cosine_similarity(query_vec, stored_vec)
            if sim < min_sim:
                continue
            try:
                entities = json.loads(row["entities"]) if row["entities"] else []
            except (json.JSONDecodeError, TypeError):
                entities = []
            try:
                anchor_entities = json.loads(row["anchor_entities"]) if row["anchor_entities"] else []
            except (json.JSONDecodeError, TypeError):
                anchor_entities = []
            try:
                anchor_keywords = json.loads(row["anchor_keywords"]) if row["anchor_keywords"] else []
            except (json.JSONDecodeError, TypeError):
                anchor_keywords = []
            results.append({
                "id": row["doc_id"],
                "doc_id": row["doc_id"],
                "name": row["name"],
                "doc_type": row["doc_type"],
                "anchor": row["anchor"],
                "content": row["anchor"],  # mirror into content so merge code is reusable
                "score": sim,
                "entities": entities,
                "anchor_entities": anchor_entities,
                "anchor_keywords": anchor_keywords,
                "path": row["path"],
                "model": row["model"],
            })
        results.sort(key=lambda r: r["score"], reverse=True)
        return results[:top_k]

    def anchor_vectors_count(self) -> tuple[int, int]:
        """Return (embedded_anchor_count, anchored_doc_count) for observability."""
        try:
            embedded = self._conn.execute(
                "SELECT count(*) FROM brain_anchor_vectors"
            ).fetchone()[0]
            anchored = self._conn.execute(
                "SELECT count(*) FROM brain_docs WHERE anchor IS NOT NULL"
            ).fetchone()[0]
            return embedded, anchored
        except sqlite3.OperationalError:
            return 0, 0

    def find_similar_anchors(self, content: str, name: str = "",
                             top_k: int = 3, exclude_path: str = None) -> List[dict]:
        """
        Find the top-K anchors most similar to the given content, used for
        Mem0-style write-time dedup (Phase E of brain-anchors).

        Strategy: extract distinctive keyword tokens from `content + name`,
        build an FTS5 OR query over brain_anchors_fts, return the top-K
        best BM25 matches. Excludes the row at `exclude_path` so a
        re-sync doesn't match its own existing anchor.

        Returns:
            List of {id, name, anchor, anchor_level, score, path} — small
            enough to embed in an LLM prompt as dedup candidates.
        """
        # Pull distinctive tokens — alphanumeric sequences of 4+ chars, lowercased,
        # deduped, capped at 15. Strips very common words that tank FTS5 precision.
        stopwords = {
            "this", "that", "with", "from", "have", "been", "were", "will",
            "they", "them", "their", "there", "which", "would", "could", "should",
            "about", "into", "your", "what", "when", "where", "more", "some",
            "other", "also", "than", "then", "like", "just", "only", "such",
            "type", "score", "status", "date", "added", "lead", "page", "name",
            "content", "source", "file", "path", "text", "note",
        }
        haystack = (content + " " + name).lower()
        raw_tokens = re.findall(r"[a-z0-9][a-z0-9_]{3,}", haystack)
        tokens: list[str] = []
        seen: set[str] = set()
        for t in raw_tokens:
            if t in stopwords or t in seen:
                continue
            tokens.append(t)
            seen.add(t)
            if len(tokens) >= 15:
                break
        if not tokens:
            return []

        fts_query = " OR ".join(tokens)
        exclude_sql = "AND d.path != ?" if exclude_path else ""
        params: list = [fts_query]
        if exclude_path:
            params.append(exclude_path)
        params.append(top_k)

        try:
            rows = self._conn.execute(f"""
                SELECT d.id, d.name, d.anchor, d.anchor_level, d.path,
                       bm25(brain_anchors_fts, 3.0, 1.5, 2.0) AS score
                FROM brain_anchors_fts f
                JOIN brain_docs d ON f.rowid = d.id
                WHERE brain_anchors_fts MATCH ?
                  AND d.anchor IS NOT NULL
                  {exclude_sql}
                ORDER BY score
                LIMIT ?
            """, params).fetchall()
        except sqlite3.OperationalError as e:
            log.warning("find_similar_anchors FTS query failed: %s", e)
            return []

        return [
            {
                "id": row["id"],
                "name": row["name"],
                "anchor": row["anchor"],
                "anchor_level": row["anchor_level"],
                "score": -row["score"],  # flip BM25 sign so higher = better
                "path": row["path"],
            }
            for row in rows
        ]

    # ═══════════════════════════════════════════════════════════════
    #  FTS5 SEARCH — Instant keyword search
    # ═══════════════════════════════════════════════════════════════

    def search(self, query: str, top_k: int = 10, doc_type: str = None) -> List[dict]:
        """
        FTS5 keyword search over Brain. Returns ranked results with BM25 scoring.

        Args:
            query: Natural language query (auto-converted to FTS5 syntax)
            top_k: Max results
            doc_type: Filter by 'page' or 'journal' (None = all)

        Returns:
            List of {name, doc_type, content, score, entities, path}
        """
        # Convert natural language to FTS5 query
        fts_query = self._to_fts5_query(query)
        if not fts_query:
            return []

        type_filter = "AND d.doc_type = ?" if doc_type else ""
        params = [fts_query] + ([doc_type] if doc_type else []) + [top_k]

        try:
            rows = self._conn.execute(f"""
                SELECT d.name, d.doc_type, d.content, d.entities, d.path,
                       bm25(brain_fts, 5.0, 1.0, 2.0) AS score
                FROM brain_fts f
                JOIN brain_docs d ON f.rowid = d.id
                WHERE brain_fts MATCH ?
                {type_filter}
                ORDER BY score
                LIMIT ?
            """, params).fetchall()
        except sqlite3.OperationalError as e:
            log.warning("FTS5 query failed for '%s': %s", fts_query, e)
            return []

        results = []
        for row in rows:
            # BM25 returns negative scores (lower = better match)
            score = -row["score"]

            # Recency boost for journals
            if row["doc_type"] == "journal":
                try:
                    date_str = row["name"].replace("_", "-")[:10]
                    days_ago = (datetime.now() - datetime.strptime(date_str, "%Y-%m-%d")).days
                    # Detect temporal queries — boost recent journals harder
                    _temporal_words = {"today", "yesterday", "recent", "recently",
                                       "latest", "last", "week", "tonight"}
                    is_temporal = bool(_temporal_words & set(query.lower().split()))
                    if is_temporal:
                        # Strong recency boost for temporal queries
                        if days_ago == 0:
                            score *= 3.0
                        elif days_ago <= 1:
                            score *= 2.5
                        elif days_ago <= 7:
                            score *= 2.0
                        elif days_ago <= 30:
                            score *= 1.3
                    else:
                        # Standard recency boost
                        if days_ago <= 7:
                            score *= 1.0 + 0.3 * (1 - days_ago / 7)
                        elif days_ago <= 30:
                            score *= 1.0 + 0.1 * (1 - days_ago / 30)
                except (ValueError, IndexError):
                    pass

            results.append({
                "name": row["name"],
                "doc_type": row["doc_type"],
                "content": row["content"],
                "score": score,
                "entities": json.loads(row["entities"]) if row["entities"] else [],
                "path": row["path"],
            })

        # Re-sort after recency boost
        results.sort(key=lambda x: x["score"], reverse=True)
        final = results[:top_k]
        self._track_recalls(final, query, source="search")
        return final

    # Stop words: common English words that match nearly every document
    _STOP_WORDS = frozenset({
        "what", "how", "when", "where", "which", "who", "why", "does",
        "the", "and", "for", "are", "but", "not", "you", "all", "can",
        "has", "was", "its", "this", "that", "with", "from", "about",
        "into", "also", "been", "have", "will", "did", "our", "your",
        "their", "there", "some", "would", "could", "should", "make",
        "know", "just", "like", "very", "much", "more", "most", "only",
        "than", "them", "then", "they", "been", "each", "want", "need",
        "tell", "please", "really", "think", "using", "used", "use",
    })

    def _to_fts5_query(self, query: str) -> str:
        """Convert natural language to FTS5 query with stop word filtering.

        Strategy:
        - Single word: exact match OR prefix match (catches plurals, compounds)
        - Multi-word: exact phrase (highest priority) OR NEAR OR individual OR terms
        """
        cleaned = re.sub(r'[^\w\s"-]', ' ', query)
        words = [w for w in cleaned.split()
                 if len(w) > 2 and w.lower() not in self._STOP_WORDS]

        if not words:
            # Fallback: use all words > 2 chars (skip stop filter)
            words = [w for w in cleaned.split() if len(w) > 2]
        if not words:
            return ""

        if len(words) == 1:
            # Single word: exact + prefix match
            w = words[0]
            return f'"{w}" OR {w}*'

        if len(words) >= 2:
            # Multi-word: exact phrase > NEAR > OR
            phrase = " ".join(words[:5])
            near_terms = " ".join(f'"{w}"' for w in words[:5])
            near = f"NEAR({near_terms}, 10)"
            or_terms = " OR ".join(f'"{w}"' for w in words)
            # Prefix match on each word too for compound word matching
            prefix_terms = " OR ".join(f"{w}*" for w in words)
            return f'("{phrase}") OR ({near}) OR ({or_terms}) OR ({prefix_terms})'

        return f'"{words[0]}"'

    # ═══════════════════════════════════════════════════════════════
    #  VECTOR SEARCH — Cosine similarity over stored embeddings
    # ═══════════════════════════════════════════════════════════════

    def store_vector(self, doc_id: int, embedding: List[float], model: str = "unknown"):
        """Store an embedding vector for a brain doc."""
        blob = _pack_vector(embedding)
        self._conn.execute("""
            INSERT INTO brain_vectors (doc_id, embedding, dim, model, created_at)
            VALUES (?, ?, ?, ?, datetime('now'))
            ON CONFLICT(doc_id) DO UPDATE SET
                embedding = excluded.embedding,
                dim = excluded.dim,
                model = excluded.model,
                created_at = excluded.created_at
        """, (doc_id, blob, len(embedding), model))
        self._conn.commit()

    def vector_search(self, query_vec: List[float], top_k: int = 10,
                      doc_type: str = None) -> List[dict]:
        """
        Brute-force cosine similarity search over stored embeddings.
        Fast enough for <1000 docs (sub-100ms on Apple Silicon).
        """
        type_filter = "AND d.doc_type = ?" if doc_type else ""
        params = (doc_type,) if doc_type else ()

        rows = self._conn.execute(f"""
            SELECT v.doc_id, v.embedding, d.name, d.doc_type, d.content, d.entities, d.path
            FROM brain_vectors v
            JOIN brain_docs d ON v.doc_id = d.id
            WHERE 1=1 {type_filter}
        """, params).fetchall()

        if not rows:
            return []

        scored = []
        query_dim = len(query_vec)
        mismatched_dims = 0
        for row in rows:
            doc_vec = _unpack_vector(row["embedding"])
            if len(doc_vec) != query_dim:
                mismatched_dims += 1
                continue
            sim = _cosine_similarity(query_vec, doc_vec)
            if sim > 0.3:  # minimum similarity threshold
                scored.append({
                    "name": row["name"],
                    "doc_type": row["doc_type"],
                    "content": row["content"],
                    "score": sim,
                    "entities": json.loads(row["entities"]) if row["entities"] else [],
                    "path": row["path"],
                })

        if mismatched_dims:
            log.warning(
                "vector_search: skipped %d/%d rows with dim mismatch against %d-d query",
                mismatched_dims, len(rows), query_dim,
            )
        scored.sort(key=lambda x: x["score"], reverse=True)
        final = scored[:top_k]
        self._track_recalls(final, f"[vector:{len(query_vec)}d]", source="vector")
        return final

    def vector_count(self) -> int:
        """How many docs have embeddings."""
        row = self._conn.execute("SELECT COUNT(*) as c FROM brain_vectors").fetchone()
        return row["c"] if row else 0

    # ═══════════════════════════════════════════════════════════════
    #  RECALL TRACKING — Active Memory feedback loop
    # ═══════════════════════════════════════════════════════════════

    def _track_recalls(self, results: list, query: str, source: str = "search") -> None:
        """
        Non-blocking recall tracking. Fire and forget.
        Records which documents were returned by a search so the promotion
        pipeline can score memories by actual usage patterns.
        """
        if not results:
            return
        try:
            from core.memory.recall_tracker import RecallTracker
            tracker = RecallTracker(self.db_path)
            tracker.record(results, query, source)
        except Exception:
            pass  # Never block search on tracking failure

    # ═══════════════════════════════════════════════════════════════
    #  ENTITY GRAPH — Temporal knowledge triples
    # ═══════════════════════════════════════════════════════════════

    def rebuild_entity_graph(self):
        """
        Rebuild entity graph from Brain wikilinks.

        Creates triples: (source_page, "links_to", target_entity)
        Journals get temporal validity (valid_from = journal date).
        """
        self._conn.execute("DELETE FROM entity_graph")

        rows = self._conn.execute(
            "SELECT name, doc_type, entities, path FROM brain_docs"
        ).fetchall()

        batch = []
        for row in rows:
            entities = json.loads(row["entities"]) if row["entities"] else []
            source = row["name"]

            valid_from = None
            if row["doc_type"] == "journal":
                try:
                    valid_from = source.replace("_", "-")[:10]
                except (ValueError, IndexError):
                    pass

            for entity in entities:
                batch.append((
                    source, "links_to", entity,
                    valid_from, None, 1.0, row["path"]
                ))

        if batch:
            self._conn.executemany("""
                INSERT INTO entity_graph (subject, predicate, object, valid_from, valid_to, confidence, source)
                VALUES (?, ?, ?, ?, ?, ?, ?)
            """, batch)

        self._conn.commit()
        log.info("Entity graph rebuilt: %d triples", len(batch))

    def entity_neighbors(self, entity: str, limit: int = 20) -> List[dict]:
        """Find entities connected to the given entity."""
        rows = self._conn.execute("""
            SELECT subject, predicate, object, valid_from, confidence
            FROM entity_graph
            WHERE subject = ? OR object = ?
            ORDER BY confidence DESC, valid_from DESC
            LIMIT ?
        """, (entity, entity, limit)).fetchall()

        return [dict(r) for r in rows]

    def god_nodes(self, top_n: int = 15) -> List[dict]:
        """Most-referenced entities (highest in-degree)."""
        rows = self._conn.execute("""
            SELECT object AS name, COUNT(*) AS mentions,
                   GROUP_CONCAT(DISTINCT subject) AS sources
            FROM entity_graph
            WHERE predicate = 'links_to'
            GROUP BY object
            ORDER BY mentions DESC
            LIMIT ?
        """, (top_n,)).fetchall()

        return [{"name": r["name"], "mentions": r["mentions"],
                 "sources": (r["sources"] or "").split(",")[:5]} for r in rows]

    def graph_context(self, query: str, top_n: int = 5) -> str:
        """Get graph context for a query — find matching entities and their neighbors."""
        words = set(query.lower().split())
        if not words:
            return ""

        # Find matching entities
        all_entities = self._conn.execute(
            "SELECT DISTINCT object AS name FROM entity_graph UNION SELECT DISTINCT subject FROM entity_graph"
        ).fetchall()

        matches = []
        for row in all_entities:
            name = row["name"]
            name_words = set(name.lower().split())
            overlap = len(words & name_words)
            if overlap > 0 or any(w in name.lower() for w in words):
                count = self._conn.execute(
                    "SELECT COUNT(*) as c FROM entity_graph WHERE object = ?", (name,)
                ).fetchone()["c"]
                matches.append((name, overlap, count))

        matches.sort(key=lambda x: (x[1], x[2]), reverse=True)
        top = matches[:top_n]

        if not top:
            return ""

        parts = []
        for name, _, count in top:
            neighbors = self._conn.execute("""
                SELECT object FROM entity_graph WHERE subject = ? LIMIT 5
            """, (name,)).fetchall()
            refs = self._conn.execute("""
                SELECT subject FROM entity_graph WHERE object = ? LIMIT 5
            """, (name,)).fetchall()

            out = [r["object"] for r in neighbors]
            inc = [r["subject"] for r in refs]
            parts.append(
                f"**{name}** ({count}x)"
                + (f" → {', '.join(out)}" if out else "")
                + (f" ← {', '.join(inc[:3])}" if inc else "")
            )

        return "Graph: " + " | ".join(parts)

    def entity_graph_count(self) -> int:
        """Number of triples in entity_graph (for staleness detection)."""
        return self._conn.execute(
            "SELECT COUNT(*) as c FROM entity_graph"
        ).fetchone()["c"]

    # ═══════════════════════════════════════════════════════════════
    #  EVENTS
    # ═══════════════════════════════════════════════════════════════

    def log_event(self, event_type: str, agent: str, summary: str, details: dict = None):
        """Log a structured event."""
        self._conn.execute(
            "INSERT INTO events (event_type, agent, summary, details) VALUES (?, ?, ?, ?)",
            (event_type, agent, summary, json.dumps(details or {}))
        )
        self._conn.commit()

    def recent_events(self, limit: int = 20, event_type: str = None) -> List[dict]:
        """Get recent events."""
        if event_type:
            rows = self._conn.execute(
                "SELECT * FROM events WHERE event_type = ? ORDER BY occurred_at DESC LIMIT ?",
                (event_type, limit)
            ).fetchall()
        else:
            rows = self._conn.execute(
                "SELECT * FROM events ORDER BY occurred_at DESC LIMIT ?", (limit,)
            ).fetchall()
        return [dict(r) for r in rows]

    # ═══════════════════════════════════════════════════════════════
    #  CACHE
    # ═══════════════════════════════════════════════════════════════

    def cache_get(self, key: str) -> Optional[str]:
        """Get cached value if not expired."""
        row = self._conn.execute(
            "SELECT value, expires_at FROM cache WHERE key = ?", (key,)
        ).fetchone()
        if not row:
            return None
        if row["expires_at"] and row["expires_at"] < datetime.now().isoformat():
            self._conn.execute("DELETE FROM cache WHERE key = ?", (key,))
            return None
        return row["value"]

    def cache_set(self, key: str, value: str, ttl_seconds: int = 300):
        """Set cached value with TTL."""
        expires = datetime.now().isoformat() if ttl_seconds == 0 else \
            datetime.fromtimestamp(time.time() + ttl_seconds).isoformat()
        self._conn.execute(
            "INSERT OR REPLACE INTO cache (key, value, expires_at) VALUES (?, ?, ?)",
            (key, value, expires)
        )
        self._conn.commit()

    # ═══════════════════════════════════════════════════════════════
    #  STATS & MAINTENANCE
    # ═══════════════════════════════════════════════════════════════

    def stats(self) -> dict:
        """Current store statistics."""
        docs = self._conn.execute("SELECT COUNT(*) as c FROM brain_docs").fetchone()["c"]
        pages = self._conn.execute("SELECT COUNT(*) as c FROM brain_docs WHERE doc_type='page'").fetchone()["c"]
        journals = self._conn.execute("SELECT COUNT(*) as c FROM brain_docs WHERE doc_type='journal'").fetchone()["c"]
        vectors = self.vector_count()
        triples = self._conn.execute("SELECT COUNT(*) as c FROM entity_graph").fetchone()["c"]
        events = self._conn.execute("SELECT COUNT(*) as c FROM events").fetchone()["c"]
        db_size = os.path.getsize(self.db_path) if os.path.exists(self.db_path) else 0

        return {
            "docs": docs, "pages": pages, "journals": journals,
            "vectors": vectors, "triples": triples, "events": events,
            "db_size_mb": round(db_size / 1024 / 1024, 2),
        }

    def close(self):
        """Close database connection."""
        self._conn.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


# ─────────────────────────────────────────────────────────────────────────────
#  CLI
# ─────────────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import sys
    logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(name)s] %(message)s")

    store = SuperbrainStore()

    if len(sys.argv) < 2 or sys.argv[1] == "--help":
        print("Usage:")
        print("  python3 store.py sync [--force]     # index Brain into FTS5")
        print("  python3 store.py search \"query\"      # FTS5 search")
        print("  python3 store.py stats               # show stats")
        print("  python3 store.py graph               # rebuild entity graph")
        print("  python3 store.py gods                # top god nodes")
        sys.exit(0)

    cmd = sys.argv[1]

    if cmd == "sync":
        force = "--force" in sys.argv
        result = store.sync_brain(force=force)
        print(json.dumps(result, indent=2))
        store.rebuild_entity_graph()

    elif cmd == "search":
        query = " ".join(sys.argv[2:])
        results = store.search(query, top_k=10)
        for r in results:
            print(f"  [{r['doc_type']:7}] {r['name']:<40} score={r['score']:.3f}  entities={r['entities'][:3]}")
            # Show snippet
            content = r['content']
            q_lower = query.lower()
            idx = content.lower().find(q_lower.split()[0] if q_lower.split() else "")
            start = max(0, idx - 50) if idx >= 0 else 0
            print(f"           {content[start:start+200].strip()}")
            print()

    elif cmd == "stats":
        s = store.stats()
        print(f"\n  Superbrain Store")
        print(f"  ─────────────────────")
        print(f"  Pages:    {s['pages']:>4}")
        print(f"  Journals: {s['journals']:>4}")
        print(f"  Vectors:  {s['vectors']:>4}")
        print(f"  Triples:  {s['triples']:>4}")
        print(f"  Events:   {s['events']:>4}")
        print(f"  DB size:  {s['db_size_mb']:.1f} MB\n")

    elif cmd == "graph":
        store.sync_brain()
        store.rebuild_entity_graph()
        gods = store.god_nodes(top_n=15)
        print(f"\nTop entities:")
        for g in gods:
            print(f"  {g['name']:<35} {g['mentions']:>3}x  sources: {', '.join(g['sources'][:3])}")

    elif cmd == "gods":
        gods = store.god_nodes(top_n=15)
        for g in gods:
            print(f"  {g['name']:<35} {g['mentions']:>3}x")

    store.close()
