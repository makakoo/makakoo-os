#!/usr/bin/env python3
"""
Migration 002: Add brain_anchor_vectors table for Phase D2 of brain-anchors.

Stores qwen3-embedding:0.6b (or any configured embedder) outputs computed
over the compressed anchor text rather than full content. Smaller vectors,
faster search, better semantic match on short declarative facts. Used by
Superbrain.query_anchored() when coverage is high enough.

Idempotent: safe to re-run. Wraps DDL in a single transaction.

Usage:
    python 002_add_anchor_vectors.py [--dry-run] [--db /path/to/db]
"""

from __future__ import annotations

import argparse
import sqlite3
import sys
from pathlib import Path

DEFAULT_DB = "/Users/sebastian/MAKAKOO/data/superbrain.db"

CREATE_ANCHOR_VECTORS = """
CREATE TABLE IF NOT EXISTS brain_anchor_vectors (
    doc_id INTEGER PRIMARY KEY REFERENCES brain_docs(id) ON DELETE CASCADE,
    embedding BLOB NOT NULL,
    dim INTEGER NOT NULL,
    model TEXT DEFAULT 'unknown',
    anchor_hash TEXT,
    created_at TEXT DEFAULT (datetime('now'))
)
""".strip()

CREATE_IDX_ANCHOR_VEC_MODEL = (
    "CREATE INDEX IF NOT EXISTS idx_brain_anchor_vectors_model "
    "ON brain_anchor_vectors(model)"
)

CREATE_IDX_ANCHOR_VEC_HASH = (
    "CREATE INDEX IF NOT EXISTS idx_brain_anchor_vectors_anchor_hash "
    "ON brain_anchor_vectors(anchor_hash)"
)


def table_exists(cur: sqlite3.Cursor, name: str) -> bool:
    cur.execute(
        "SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name=?",
        (name,),
    )
    return cur.fetchone() is not None


def run(db_path: str, dry_run: bool) -> int:
    if not Path(db_path).exists():
        print(f"ERROR: database not found: {db_path}", file=sys.stderr)
        return 2

    conn = sqlite3.connect(db_path, isolation_level="DEFERRED")
    conn.execute("PRAGMA foreign_keys = ON")
    cur = conn.cursor()

    statements: list[str] = []

    try:
        cur.execute("SELECT count(*) FROM brain_docs")
        rows_before = cur.fetchone()[0]

        if not table_exists(cur, "brain_anchor_vectors"):
            statements.append(CREATE_ANCHOR_VECTORS)

        statements.append(CREATE_IDX_ANCHOR_VEC_MODEL)
        statements.append(CREATE_IDX_ANCHOR_VEC_HASH)

        if dry_run:
            print("=== DRY RUN: SQL that would be executed ===")
            for i, stmt in enumerate(statements, 1):
                print(f"\n-- [{i}] --")
                print(stmt + ";")
            conn.rollback()
            return 0

        cur.execute("BEGIN")
        for stmt in statements:
            cur.execute(stmt)

        if not table_exists(cur, "brain_anchor_vectors"):
            raise RuntimeError("brain_anchor_vectors missing after migration")

        cur.execute("SELECT count(*) FROM brain_docs")
        rows_after = cur.fetchone()[0]
        if rows_after != rows_before:
            raise RuntimeError(f"row count changed: before={rows_before} after={rows_after}")

        cur.execute("SELECT count(*) FROM brain_anchor_vectors")
        vec_count = cur.fetchone()[0]

        conn.commit()

        print("=== Migration 002: SUCCESS ===")
        print(f"db:                     {db_path}")
        print(f"rows_before:            {rows_before}")
        print(f"rows_after:             {rows_after}")
        print(f"brain_anchor_vectors:   present (rows={vec_count})")
        print(f"statements_run:         {len(statements)}")
        return 0

    except Exception as exc:
        conn.rollback()
        print(f"ERROR: migration failed, rolled back: {exc}", file=sys.stderr)
        return 1
    finally:
        conn.close()


def main() -> int:
    ap = argparse.ArgumentParser(description="Add brain_anchor_vectors for Phase D2")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--db", default=DEFAULT_DB)
    args = ap.parse_args()
    return run(args.db, args.dry_run)


if __name__ == "__main__":
    sys.exit(main())
