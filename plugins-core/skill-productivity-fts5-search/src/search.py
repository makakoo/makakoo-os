"""
FTS5 Conversation Search API

Usage:
    from search import search
    results = search("code review patterns")
"""

import sqlite3
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

DB_PATH = Path.home() / "HARVEY" / "data" / "fts5" / "conversations.db"


@dataclass
class SearchResult:
    content: str
    session_id: str
    agent_id: str
    timestamp: str
    date: str
    journal_path: str
    score: float
    snippet: str


def search(
    query: str,
    limit: int = 10,
    date_from: Optional[str] = None,
    date_to: Optional[str] = None,
    session_id: Optional[str] = None,
) -> list[SearchResult]:
    """
    BM25-ranked full-text search across conversation entries.

    Args:
        query: Search query string
        limit: Max results to return
        date_from: Filter entries since YYYY-MM-DD
        date_to: Filter entries until YYYY-MM-DD
        session_id: Filter by specific session

    Returns:
        List of SearchResult objects sorted by BM25 relevance score
    """
    if not DB_PATH.exists():
        return []

    conn = sqlite3.connect(DB_PATH)
    conn.execute("PRAGMA journal_mode=WAL")

    # Build query with optional filters
    where_clauses = ["conversation_fts MATCH ?"]
    params = [query]

    if session_id:
        where_clauses.append("session_id = ?")
        params.append(session_id)

    if date_from:
        where_clauses.append("date >= ?")
        params.append(date_from.replace('-', '_'))

    if date_to:
        where_clauses.append("date <= ?")
        params.append(date_to.replace('-', '_'))

    where = " AND ".join(where_clauses)

    sql = f"""
        SELECT
            content,
            session_id,
            agent_id,
            timestamp,
            date,
            journal_path,
            bm25(conversation_fts, 10) AS score,
            snippet(conversation_fts, 3, '<mark>', '</mark>', '...', 30) AS snippet
        FROM conversation_fts
        WHERE {where}
        ORDER BY score
        LIMIT ?
    """
    params.append(limit)

    cursor = conn.execute(sql, params)
    rows = cursor.fetchall()
    conn.close()

    return [
        SearchResult(
            content=row[0],
            session_id=row[1],
            agent_id=row[2],
            timestamp=row[3],
            date=row[4],
            journal_path=row[5],
            score=row[6],
            snippet=row[7],
        )
        for row in rows
    ]


def get_total_entries() -> int:
    """Return total number of indexed entries."""
    if not DB_PATH.exists():
        return 0
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.execute("SELECT COUNT(*) FROM conversation_fts")
    count = cursor.fetchone()[0]
    conn.close()
    return count


def get_index_stats() -> dict:
    """Return index statistics."""
    if not DB_PATH.exists():
        return {"error": "index not initialized"}

    conn = sqlite3.connect(DB_PATH)
    cursor = conn.execute("SELECT COUNT(*) FROM conversation_fts")
    total = cursor.fetchone()[0]

    cursor = conn.execute("SELECT COUNT(DISTINCT date) FROM conversation_fts")
    days = cursor.fetchone()[0]

    cursor = conn.execute("SELECT COUNT(DISTINCT session_id) FROM conversation_fts")
    sessions = cursor.fetchone()[0]

    conn.close()
    return {"total_entries": total, "days_indexed": days, "sessions": sessions}
