"""
backfill_anchor_vectors — one-shot script to embed existing anchors into
brain_anchor_vectors. Phase D2 of the brain-anchors memory system.

Runs after Phase C (text anchor backfill) completes. Walks every row in
brain_docs with a non-NULL anchor and no corresponding row in
brain_anchor_vectors (or with an anchor_hash mismatch if --force is set),
embeds the anchor via switchAILocal's qwen3-embedding:0.6b, and persists.

Safe to re-run — idempotent via LEFT JOIN on brain_anchor_vectors.
Safe to interrupt — each row commits individually.

Usage:
    python -m harvey_os.core.superbrain.backfill_anchor_vectors [--dry-run] [--limit N] [--force]
"""

from __future__ import annotations

import argparse
import logging
import sqlite3
import sys
import time
from pathlib import Path

_THIS = Path(__file__).resolve()
_CORE_DIR = _THIS.parent
sys.path.insert(0, str(_CORE_DIR.parent.parent))

from core.superbrain.store import SuperbrainStore  # noqa: E402
from core.superbrain.embeddings import CURRENT_MODEL  # noqa: E402

DEFAULT_DB = "/Users/sebastian/MAKAKOO/data/superbrain.db"

log = logging.getLogger("backfill_anchor_vectors")


def _parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--db", type=str, default=DEFAULT_DB)
    p.add_argument("--limit", type=int, default=0)
    p.add_argument("--dry-run", action="store_true")
    p.add_argument(
        "--force",
        action="store_true",
        help="re-embed even for docs that already have a vector with matching anchor_hash",
    )
    p.add_argument("--verbose", action="store_true")
    return p.parse_args()


def main() -> int:
    args = _parse_args()
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    if not Path(args.db).exists():
        log.error("db not found: %s", args.db)
        return 2

    store = SuperbrainStore(db_path=args.db)

    # Candidates: rows with an anchor but no vector (or hash mismatch if --force)
    if args.force:
        sql = (
            "SELECT d.id, d.name, d.anchor, d.anchor_hash "
            "FROM brain_docs d "
            "WHERE d.anchor IS NOT NULL "
            "ORDER BY d.id"
        )
    else:
        sql = (
            "SELECT d.id, d.name, d.anchor, d.anchor_hash "
            "FROM brain_docs d "
            "LEFT JOIN brain_anchor_vectors v ON v.doc_id = d.id "
            "WHERE d.anchor IS NOT NULL "
            "  AND (v.doc_id IS NULL OR v.anchor_hash IS NOT d.anchor_hash) "
            "ORDER BY d.id"
        )
    if args.limit and args.limit > 0:
        sql += f" LIMIT {int(args.limit)}"

    rows = store._conn.execute(sql).fetchall()
    total = len(rows)
    log.info("selected %d anchors to embed (model=%s, force=%s)", total, CURRENT_MODEL, args.force)

    if args.dry_run:
        for r in rows[:10]:
            print(f"WOULD EMBED: id={r['id']} name={r['name'][:50]}")
        if total > 10:
            print(f"... and {total - 10} more")
        return 0

    ok = 0
    failed = 0
    t0 = time.time()

    for i, row in enumerate(rows, start=1):
        doc_id = row["id"]
        anchor = row["anchor"]
        anchor_hash = row["anchor_hash"]
        if not anchor:
            continue
        try:
            success = store.embed_and_store_anchor(doc_id, anchor, anchor_hash)
            if success:
                ok += 1
                store._conn.commit()  # commit per-row for interrupt safety
            else:
                failed += 1
        except Exception as e:
            log.exception("embed failed on doc_id=%s: %s", doc_id, e)
            failed += 1

        if i % 25 == 0 or i == total:
            elapsed = time.time() - t0
            rate = i / elapsed if elapsed > 0 else 0
            eta = (total - i) / rate if rate > 0 else 0
            log.info(
                "progress %d/%d ok=%d failed=%d rate=%.2f/s eta=%.0fs",
                i, total, ok, failed, rate, eta,
            )

    elapsed = time.time() - t0
    log.info("DONE total=%d ok=%d failed=%d elapsed=%.1fs", total, ok, failed, elapsed)

    # Events row for observability
    try:
        store._conn.execute(
            """
            INSERT INTO events (event_type, agent, summary, details, occurred_at)
            VALUES (?, ?, ?, ?, datetime('now'))
            """,
            (
                "backfill_anchor_vectors",
                "brain-anchors",
                f"embed {ok}/{total} anchors ok, {failed} failed in {elapsed:.0f}s via {CURRENT_MODEL}",
                "{}",
            ),
        )
        store._conn.commit()
    except sqlite3.OperationalError as e:
        log.warning("events row failed: %s", e)

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
