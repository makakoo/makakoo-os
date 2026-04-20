"""
backfill_anchors — one-shot script to generate anchors for existing brain_docs.

Phase C of the brain-anchors migration. After Phase A (schema migration) has
added the `anchor*` columns, this script walks every row in brain_docs and
calls anchor_extractor.extract_anchor_safe() for any row that doesn't yet
have an anchor. Writes anchor fields back in-place.

Safe to re-run — idempotent via the `anchor` IS NULL filter.
Safe to interrupt — each row is committed individually.

Usage:
    python -m harvey_os.core.superbrain.backfill_anchors [--dry-run] [--limit N] [--db PATH] [--model MODEL] [--force]
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import sqlite3
import sys
import time
from datetime import datetime
from pathlib import Path

# Allow running as a script from anywhere
_THIS = Path(__file__).resolve()
_CORE_DIR = _THIS.parent  # .../plugins-core/lib-harvey-core/src/core/superbrain/
sys.path.insert(0, str(_CORE_DIR))

from anchor_extractor import (  # noqa: E402
    AnchorExtractionError,
    extract_anchor_safe,
    PRIMARY_MODEL,
)

DEFAULT_DB = "/Users/sebastian/MAKAKOO/data/superbrain.db"

log = logging.getLogger("backfill_anchors")


def _parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--db", type=str, default=DEFAULT_DB, help="path to superbrain.db")
    p.add_argument("--limit", type=int, default=0, help="max docs to process (0 = all)")
    p.add_argument("--dry-run", action="store_true", help="report what would run, don't call LLM or write")
    p.add_argument("--force", action="store_true", help="re-extract even for docs that already have an anchor")
    p.add_argument("--model", type=str, help="override BRAIN_ANCHOR_MODEL_PRIMARY for this run")
    p.add_argument("--only-type", type=str, choices=("page", "journal"), help="filter by doc_type")
    p.add_argument("--verbose", action="store_true")
    return p.parse_args()


def _select_rows(conn: sqlite3.Connection, force: bool, only_type: str | None, limit: int) -> list[tuple]:
    sql = "SELECT id, name, doc_type, content FROM brain_docs"
    conds = []
    params: list = []
    if not force:
        conds.append("anchor IS NULL")
    if only_type:
        conds.append("doc_type = ?")
        params.append(only_type)
    if conds:
        sql += " WHERE " + " AND ".join(conds)
    sql += " ORDER BY id"
    if limit and limit > 0:
        sql += f" LIMIT {int(limit)}"
    return conn.execute(sql, params).fetchall()


def _apply_anchor(conn: sqlite3.Connection, doc_id: int, result: dict) -> None:
    conn.execute(
        """
        UPDATE brain_docs SET
            anchor = ?,
            anchor_level = ?,
            anchor_hash = ?,
            anchor_keywords = ?,
            anchor_entities = ?,
            anchor_generated_at = datetime('now'),
            anchor_model = ?
        WHERE id = ?
        """,
        (
            result["anchor"],
            result.get("anchor_level", "atomic"),
            result["anchor_hash"],
            json.dumps(result.get("keywords", [])),
            json.dumps(result.get("entities", [])),
            result.get("anchor_model", "unknown"),
            doc_id,
        ),
    )
    conn.commit()


def main() -> int:
    args = _parse_args()
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    if args.model:
        os.environ["BRAIN_ANCHOR_MODEL_PRIMARY"] = args.model
        # Re-import with the new env — simple approach: tell user to set env before running
        log.info("Note: --model sets env but anchor_extractor already imported. Re-run with BRAIN_ANCHOR_MODEL_PRIMARY=%s instead for reliable override.", args.model)

    db_path = args.db
    if not Path(db_path).exists():
        log.error("database not found: %s", db_path)
        return 2

    log.info("db=%s dry_run=%s force=%s only_type=%s limit=%s primary_model=%s",
             db_path, args.dry_run, args.force, args.only_type, args.limit or "all", PRIMARY_MODEL)

    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row

    # Verify the anchor column exists
    cols = {row[1] for row in conn.execute("PRAGMA table_info(brain_docs)").fetchall()}
    required = {"anchor", "anchor_level", "anchor_hash", "anchor_keywords", "anchor_entities", "anchor_generated_at", "anchor_model"}
    missing = required - cols
    if missing:
        log.error("brain_docs missing required anchor columns: %s. Run migrations/001_add_anchor_columns.py first.", missing)
        return 3

    rows = _select_rows(conn, args.force, args.only_type, args.limit)
    total = len(rows)
    log.info("selected %d docs to process", total)

    if args.dry_run:
        for r in rows[:10]:
            print(f"WOULD EXTRACT: id={r[0]} type={r[2]} name={r[1]} chars={len(r[3])}")
        if total > 10:
            print(f"... and {total - 10} more")
        return 0

    ok = 0
    failed = 0
    skipped_short = 0
    t0 = time.time()

    for i, (doc_id, name, doc_type, content) in enumerate(rows, start=1):
        if not content or len(content.strip()) < 20:
            skipped_short += 1
            continue

        try:
            result = extract_anchor_safe(name, content, doc_type)
        except Exception as e:
            log.exception("extractor raised unexpectedly on doc_id=%s: %s", doc_id, e)
            failed += 1
            continue

        if result is None:
            failed += 1
            log.warning("extract_anchor returned None for doc_id=%s name=%r", doc_id, name)
            continue

        try:
            _apply_anchor(conn, doc_id, result)
            ok += 1
        except Exception as e:
            log.exception("db update failed on doc_id=%s: %s", doc_id, e)
            failed += 1
            continue

        if i % 10 == 0 or i == total:
            elapsed = time.time() - t0
            rate = i / elapsed if elapsed > 0 else 0
            eta = (total - i) / rate if rate > 0 else 0
            log.info(
                "progress %d/%d ok=%d failed=%d skipped_short=%d elapsed=%.1fs rate=%.2f/s eta=%.0fs",
                i, total, ok, failed, skipped_short, elapsed, rate, eta,
            )

    elapsed = time.time() - t0
    log.info(
        "DONE total=%d ok=%d failed=%d skipped_short=%d elapsed=%.1fs",
        total, ok, failed, skipped_short, elapsed,
    )

    # Persist a run record to the events table for Brain observability.
    # events schema: (event_type, agent, summary, details, occurred_at)
    try:
        conn.execute(
            """
            INSERT INTO events (event_type, agent, summary, details, occurred_at)
            VALUES (?, ?, ?, ?, datetime('now'))
            """,
            (
                "backfill_anchors",
                "brain-anchors",
                f"backfill {ok}/{total} ok, {failed} failed, {skipped_short} skipped in {elapsed:.0f}s",
                json.dumps({
                    "total": total,
                    "ok": ok,
                    "failed": failed,
                    "skipped_short": skipped_short,
                    "elapsed_sec": round(elapsed, 1),
                    "primary_model": PRIMARY_MODEL,
                    "timestamp": datetime.now().isoformat(),
                }),
            ),
        )
        conn.commit()
    except sqlite3.OperationalError as e:
        log.warning("could not write events row (schema mismatch?): %s", e)

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
