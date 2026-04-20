"""
FTS5 Conversation Indexer

Parses Harvey Brain journal files and indexes conversation entries into FTS5.
Run manually or via cron to keep the index fresh.

Usage:
    python3 indexer.py                    # Index all journals
    python3 indexer.py --since 2026-03-01  # Index since date
    python3 indexer.py --journal 2026_03_28.md  # Single journal
"""

import re
import sqlite3
import hashlib
from pathlib import Path
from datetime import datetime
import argparse

BRAIN_JOURNALS = Path.home() / "HARVEY" / "data" / "Brain" / "journals"
DB_PATH = Path.home() / "HARVEY" / "data" / "fts5" / "conversations.db"


def init_db(db_path: Path) -> None:
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("""
        CREATE VIRTUAL TABLE IF NOT EXISTS conversation_fts USING fts5(
            content,
            session_id,
            agent_id,
            timestamp,
            date,
            journal_path,
            tokenize='porter unicode61'
        )
    """)
    conn.execute("""
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            agent_id TEXT,
            started_at TEXT,
            ended_at TEXT,
            journal_path TEXT,
            entry_count INTEGER
        )
    """)
    conn.execute("""
        CREATE TABLE IF NOT EXISTS indexed_entries (
            hash TEXT PRIMARY KEY,
            session_id TEXT,
            indexed_at TEXT
        )
    """)
    conn.commit()
    conn.close()


def strip_logseq(text: str) -> str:
    """Remove Brain syntax: [[links]], #tags, code fences, timestamps."""
    # Remove [[link]] and [[link|alias]]
    text = re.sub(r'\[\[([^\]|]+)(?:\|[^\]]+)?\]\]', r'\1', text)
    # Remove #tags
    text = re.sub(r'#[a-zA-Z0-9_-]+', '', text)
    # Remove code fences
    text = re.sub(r'```[\s\S]*?```', '', text)
    # Remove inline code
    text = re.sub(r'`[^`]+`', '', text)
    # Remove timestamps like [HH:MM]
    text = re.sub(r'\[\d{2}:\d{2}(?::\d{2})?\]', '', text)
    # Remove leading bullet markers
    text = re.sub(r'^-\s+', '', text)
    return text.strip()


def parse_entries(content: str, journal_path: str, date: str) -> list[dict]:
    """Extract bullet point entries from journal content."""
    entries = []
    current_session = f"{date}-001"

    for line in content.split('\n'):
        # Detect session header
        session_match = re.match(r'^##\s+Session\s+(\d+)', line)
        if session_match:
            current_session = f"{date}-{session_match.group(1).zfill(3)}"
            continue

        # Match bullet points with optional timestamps
        # Format: - [HH:MM] content or just - content
        ts_match = re.match(r'^-\s+(?:\[(\d{2}:\d{2}(?::\d{2})?)\])?\s*(.+)$', line)
        if ts_match:
            ts = ts_match.group(1) or "00:00"
            raw_content = ts_match.group(2).strip()

            # Skip empty or very short lines
            if len(raw_content) < 3:
                continue

            # Skip bullet list continuation lines (indented)
            if raw_content.startswith('  '):
                continue

            content_clean = strip_logseq(raw_content)
            if not content_clean:
                continue

            content_hash = hashlib.sha256(
                f"{current_session}{ts}{content_clean}".encode()
            ).hexdigest()[:16]

            entries.append({
                'content': content_clean,
                'session_id': current_session,
                'agent_id': 'harvey',
                'timestamp': ts,
                'date': date,
                'journal_path': str(journal_path),
                'hash': content_hash,
            })

    return entries


def index_journal(db: sqlite3.Connection, path: Path) -> int:
    """Index a single journal file. Returns count of new entries indexed."""
    date = path.stem  # YYYY_MM_DD
    content = path.read_text()
    entries = parse_entries(content, path, date)

    indexed = 0
    for entry in entries:
        # Check if already indexed
        cursor = db.execute(
            "SELECT hash FROM indexed_entries WHERE hash = ?", (entry['hash'],)
        )
        if cursor.fetchone():
            continue

        # Insert into FTS
        db.execute("""
            INSERT INTO conversation_fts (content, session_id, agent_id, timestamp, date, journal_path)
            VALUES (?, ?, ?, ?, ?, ?)
        """, (
            entry['content'],
            entry['session_id'],
            entry['agent_id'],
            entry['timestamp'],
            entry['date'],
            entry['journal_path'],
        ))

        # Mark as indexed
        db.execute(
            "INSERT OR IGNORE INTO indexed_entries (hash, session_id, indexed_at) VALUES (?, ?, ?)",
            (entry['hash'], entry['session_id'], datetime.now().isoformat())
        )
        indexed += 1

    return indexed


def index_all(since: str = None, single_journal: str = None) -> dict:
    """Index all journals or a single one. Returns stats."""
    init_db(DB_PATH)
    conn = sqlite3.connect(DB_PATH)
    conn.execute("PRAGMA journal_mode=WAL")

    total_indexed = 0
    journals_processed = 0

    if single_journal:
        path = BRAIN_JOURNALS / single_journal
        if path.exists():
            count = index_journal(conn, path)
            total_indexed += count
            journals_processed += 1
    else:
        for path in sorted(BRAIN_JOURNALS.glob("*.md")):
            if since and path.stem < since.replace('-', '_'):
                continue
            count = index_journal(conn, path)
            total_indexed += count
            journals_processed += 1

    conn.commit()
    conn.close()

    return {
        'journals_processed': journals_processed,
        'entries_indexed': total_indexed,
        'db_path': str(DB_PATH),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Index Harvey Brain journals to FTS5")
    parser.add_argument("--since", help="Index journals since YYYY-MM-DD")
    parser.add_argument("--journal", help="Single journal file (e.g. 2026_03_28.md)")
    args = parser.parse_args()

    result = index_all(since=args.since, single_journal=args.journal)
    print(f"Indexed {result['entries_indexed']} entries from {result['journals_processed']} journals")
    print(f"DB: {result['db_path']}")
