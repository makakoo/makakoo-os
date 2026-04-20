#!/usr/bin/env python3
"""
Migration 001: Add anchor columns + brain_anchors_fts to superbrain.db

Adds seven anchor_* columns to brain_docs, creates a new FTS5 virtual table
(brain_anchors_fts) indexing anchor/keywords/entities, mirrors the existing
brain_fts trigger pattern to keep the new FTS in sync, and adds an index on
anchor_hash for dedup lookups.

Idempotent: safe to re-run. Wraps all DDL in a single transaction.

Usage:
    python 001_add_anchor_columns.py [--dry-run] [--db /path/to/db]
"""

from __future__ import annotations

import argparse
import sqlite3
import sys
from pathlib import Path

DEFAULT_DB = "/Users/sebastian/MAKAKOO/data/superbrain.db"

NEW_COLUMNS = [
    ("anchor", "TEXT"),
    ("anchor_level", "TEXT DEFAULT 'atomic'"),
    ("anchor_hash", "TEXT"),
    ("anchor_keywords", "TEXT"),  # JSON array
    ("anchor_entities", "TEXT"),  # JSON array
    ("anchor_generated_at", "TIMESTAMP"),
    ("anchor_model", "TEXT"),
]

CREATE_ANCHORS_FTS = """
CREATE VIRTUAL TABLE IF NOT EXISTS brain_anchors_fts USING fts5(
    anchor, anchor_keywords, anchor_entities,
    content='brain_docs', content_rowid='id',
    tokenize='porter unicode61'
)
""".strip()

# Mirrors the brain_fts trigger pattern (AI/AD/AU), but targets brain_anchors_fts
# and uses the anchor_* columns.
TRIGGER_AI = """
CREATE TRIGGER IF NOT EXISTS brain_docs_anchors_ai AFTER INSERT ON brain_docs BEGIN
    INSERT INTO brain_anchors_fts(rowid, anchor, anchor_keywords, anchor_entities)
    VALUES (new.id, new.anchor, new.anchor_keywords, new.anchor_entities);
END
""".strip()

TRIGGER_AD = """
CREATE TRIGGER IF NOT EXISTS brain_docs_anchors_ad AFTER DELETE ON brain_docs BEGIN
    INSERT INTO brain_anchors_fts(brain_anchors_fts, rowid, anchor, anchor_keywords, anchor_entities)
    VALUES ('delete', old.id, old.anchor, old.anchor_keywords, old.anchor_entities);
END
""".strip()

TRIGGER_AU = """
CREATE TRIGGER IF NOT EXISTS brain_docs_anchors_au AFTER UPDATE ON brain_docs BEGIN
    INSERT INTO brain_anchors_fts(brain_anchors_fts, rowid, anchor, anchor_keywords, anchor_entities)
    VALUES ('delete', old.id, old.anchor, old.anchor_keywords, old.anchor_entities);
    INSERT INTO brain_anchors_fts(rowid, anchor, anchor_keywords, anchor_entities)
    VALUES (new.id, new.anchor, new.anchor_keywords, new.anchor_entities);
END
""".strip()

CREATE_INDEX = (
    "CREATE INDEX IF NOT EXISTS idx_brain_docs_anchor_hash "
    "ON brain_docs(anchor_hash)"
)

# FTS5 index must be rebuilt for any pre-existing brain_docs rows after
# the virtual table is created. Without this, UPDATE triggers fail with
# "database disk image is malformed" when they try to `delete` an OLD
# row that was never indexed. Safe to run repeatedly — rebuild is a
# no-op on a fully-consistent index.
REBUILD_ANCHORS_FTS = (
    "INSERT INTO brain_anchors_fts(brain_anchors_fts) VALUES('rebuild')"
)

EXPECTED_TRIGGERS = [
    "brain_docs_anchors_ai",
    "brain_docs_anchors_ad",
    "brain_docs_anchors_au",
]


def existing_columns(cur: sqlite3.Cursor, table: str) -> set[str]:
    cur.execute(f"PRAGMA table_info({table})")
    return {row[1] for row in cur.fetchall()}


def existing_triggers(cur: sqlite3.Cursor) -> set[str]:
    cur.execute("SELECT name FROM sqlite_master WHERE type='trigger'")
    return {row[0] for row in cur.fetchall()}


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

    # autocommit=False is the default for sqlite3.connect (isolation_level=''
    # means implicit transaction). We use an explicit BEGIN for clarity.
    conn = sqlite3.connect(db_path, isolation_level="DEFERRED")
    conn.execute("PRAGMA foreign_keys = ON")
    cur = conn.cursor()

    statements: list[str] = []

    try:
        # --- Pre-state ---
        cur.execute("SELECT count(*) FROM brain_docs")
        rows_before = cur.fetchone()[0]

        have_cols = existing_columns(cur, "brain_docs")
        have_trigs = existing_triggers(cur)

        # --- 1. ALTER TABLE for new columns (idempotent) ---
        columns_added: list[str] = []
        for col_name, col_decl in NEW_COLUMNS:
            if col_name in have_cols:
                continue
            stmt = f"ALTER TABLE brain_docs ADD COLUMN {col_name} {col_decl}"
            statements.append(stmt)
            columns_added.append(col_name)

        # --- 2. CREATE FTS5 virtual table ---
        if not table_exists(cur, "brain_anchors_fts"):
            statements.append(CREATE_ANCHORS_FTS)

        # --- 3. Triggers ---
        triggers_added: list[str] = []
        for trig_name, trig_sql in (
            ("brain_docs_anchors_ai", TRIGGER_AI),
            ("brain_docs_anchors_ad", TRIGGER_AD),
            ("brain_docs_anchors_au", TRIGGER_AU),
        ):
            if trig_name in have_trigs:
                continue
            statements.append(trig_sql)
            triggers_added.append(trig_name)

        # --- 4. Index on anchor_hash ---
        statements.append(CREATE_INDEX)

        # --- 5. Rebuild FTS5 index for existing brain_docs rows ---
        # Always run — safe no-op if already consistent.
        statements.append(REBUILD_ANCHORS_FTS)

        if dry_run:
            print("=== DRY RUN: SQL that would be executed ===")
            for i, stmt in enumerate(statements, 1):
                print(f"\n-- [{i}] --")
                print(stmt + ";")
            print("\n=== DRY RUN summary ===")
            print(f"db:             {db_path}")
            print(f"rows_before:    {rows_before}")
            print(f"columns_added:  {columns_added or '(none — already present)'}")
            print(f"triggers_added: {triggers_added or '(none — already present)'}")
            print(f"statements:     {len(statements)}")
            conn.rollback()
            return 0

        # --- Execute inside a single transaction ---
        cur.execute("BEGIN")
        for stmt in statements:
            cur.execute(stmt)

        # --- Post-migration sanity checks BEFORE commit ---
        cur.execute("SELECT count(*) FROM brain_docs")
        rows_after = cur.fetchone()[0]
        if rows_after != rows_before:
            raise RuntimeError(
                f"row count mismatch: before={rows_before} after={rows_after}"
            )

        have_cols_after = existing_columns(cur, "brain_docs")
        for col_name, _ in NEW_COLUMNS:
            if col_name not in have_cols_after:
                raise RuntimeError(f"column missing after migration: {col_name}")

        if not table_exists(cur, "brain_anchors_fts"):
            raise RuntimeError("brain_anchors_fts missing after migration")

        have_trigs_after = existing_triggers(cur)
        for trig in EXPECTED_TRIGGERS:
            if trig not in have_trigs_after:
                raise RuntimeError(f"trigger missing after migration: {trig}")

        # Verify anchor_hash index exists
        cur.execute(
            "SELECT 1 FROM sqlite_master WHERE type='index' "
            "AND name='idx_brain_docs_anchor_hash'"
        )
        if cur.fetchone() is None:
            raise RuntimeError("idx_brain_docs_anchor_hash missing after migration")

        # Verify FTS can be queried (and can take a rebuild-style no-op)
        cur.execute("SELECT count(*) FROM brain_anchors_fts")
        fts_count = cur.fetchone()[0]

        conn.commit()

        print("=== Migration 001: SUCCESS ===")
        print(f"db:                {db_path}")
        print(f"rows_before:       {rows_before}")
        print(f"rows_after:        {rows_after}")
        print(f"columns_added:     {columns_added or '(none — already present)'}")
        print(f"triggers_added:    {triggers_added or '(none — already present)'}")
        print(f"brain_anchors_fts: present (rows={fts_count})")
        print(f"index:             idx_brain_docs_anchor_hash present")
        print(f"statements_run:    {len(statements)}")
        return 0

    except Exception as exc:  # noqa: BLE001
        conn.rollback()
        print(f"ERROR: migration failed, rolled back: {exc}", file=sys.stderr)
        return 1
    finally:
        conn.close()


def main() -> int:
    ap = argparse.ArgumentParser(description="Add anchor columns and brain_anchors_fts")
    ap.add_argument("--dry-run", action="store_true", help="print SQL without executing")
    ap.add_argument("--db", default=DEFAULT_DB, help=f"sqlite path (default: {DEFAULT_DB})")
    args = ap.parse_args()
    return run(args.db, args.dry_run)


if __name__ == "__main__":
    sys.exit(main())
