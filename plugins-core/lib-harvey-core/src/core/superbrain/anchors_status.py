#!/usr/bin/env python3
"""
anchors_status — observability dashboard for the brain-anchors memory system.

Read-only. Safe to run any time (during backfill, after, in CI).

Usage:
    python3 -m core.superbrain.anchors_status             # full dashboard
    python3 -m core.superbrain.anchors_status --json      # machine-readable
    python3 -m core.superbrain.anchors_status --sample 5  # show 5 random anchors
    python3 -m core.superbrain.anchors_status --model     # model distribution only
    python3 -m core.superbrain.anchors_status --tail      # tail the backfill log
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import sys
import time
from datetime import datetime
from pathlib import Path

DEFAULT_DB = "/Users/sebastian/MAKAKOO/data/superbrain.db"
BACKFILL_LOG = "/Users/sebastian/MAKAKOO/tmp/backfill_anchors.log"


def _human_pct(n: int, d: int) -> str:
    if d == 0:
        return "0%"
    return f"{(n / d) * 100:.1f}%"


def _connect(db_path: str) -> sqlite3.Connection:
    if not Path(db_path).exists():
        print(f"ERROR: database not found: {db_path}", file=sys.stderr)
        sys.exit(2)
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    return conn


def gather_status(db_path: str = DEFAULT_DB, sample_n: int = 0) -> dict:
    conn = _connect(db_path)
    try:
        # Required columns check (graceful if migration hasn't run)
        cols = {row[1] for row in conn.execute("PRAGMA table_info(brain_docs)").fetchall()}
        if "anchor" not in cols:
            return {
                "db": db_path,
                "migration_status": "NOT APPLIED — run migrations/001_add_anchor_columns.py",
                "total_docs": conn.execute("SELECT count(*) FROM brain_docs").fetchone()[0],
                "anchored_docs": 0,
                "coverage": "0%",
            }

        total = conn.execute("SELECT count(*) FROM brain_docs").fetchone()[0]
        anchored = conn.execute(
            "SELECT count(*) FROM brain_docs WHERE anchor IS NOT NULL"
        ).fetchone()[0]

        # Per-doc-type breakdown
        by_type = {}
        for row in conn.execute(
            "SELECT doc_type, count(*) AS n, "
            "SUM(CASE WHEN anchor IS NOT NULL THEN 1 ELSE 0 END) AS anchored "
            "FROM brain_docs GROUP BY doc_type"
        ).fetchall():
            by_type[row["doc_type"]] = {
                "total": row["n"],
                "anchored": row["anchored"] or 0,
                "coverage": _human_pct(row["anchored"] or 0, row["n"]),
            }

        # Model distribution (which extractor wrote each anchor)
        model_dist = {}
        for row in conn.execute(
            "SELECT anchor_model, count(*) AS n FROM brain_docs "
            "WHERE anchor_model IS NOT NULL GROUP BY anchor_model ORDER BY n DESC"
        ).fetchall():
            model_dist[row["anchor_model"]] = row["n"]

        # Recent anchor activity (last 10 by generation time)
        recent_rows = conn.execute(
            "SELECT id, name, anchor, anchor_model, anchor_generated_at "
            "FROM brain_docs WHERE anchor IS NOT NULL "
            "ORDER BY anchor_generated_at DESC LIMIT 10"
        ).fetchall()
        recent = [
            {
                "id": r["id"],
                "name": r["name"][:60],
                "anchor": (r["anchor"] or "")[:140],
                "model": r["anchor_model"],
                "at": r["anchor_generated_at"],
            }
            for r in recent_rows
        ]

        # Anchor text length stats (in chars)
        stats_row = conn.execute(
            "SELECT AVG(length(anchor)) AS avg_len, MIN(length(anchor)) AS min_len, "
            "MAX(length(anchor)) AS max_len FROM brain_docs WHERE anchor IS NOT NULL"
        ).fetchone()
        avg_len = int(stats_row["avg_len"] or 0)
        min_len = stats_row["min_len"] or 0
        max_len = stats_row["max_len"] or 0

        # FTS5 anchor index health
        try:
            fts_count = conn.execute(
                "SELECT count(*) FROM brain_anchors_fts WHERE anchor IS NOT NULL"
            ).fetchone()[0]
            fts_ok = True
        except sqlite3.OperationalError as e:
            fts_count = 0
            fts_ok = f"FTS error: {e}"

        # Last backfill_anchors event from events table
        last_event = None
        try:
            row = conn.execute(
                "SELECT summary, occurred_at, details FROM events "
                "WHERE event_type = 'backfill_anchors' "
                "ORDER BY occurred_at DESC LIMIT 1"
            ).fetchone()
            if row:
                last_event = {
                    "summary": row["summary"],
                    "at": row["occurred_at"],
                    "details": row["details"],
                }
        except sqlite3.OperationalError:
            pass

        # Optional random sample of live anchors
        sample = []
        if sample_n > 0:
            for r in conn.execute(
                "SELECT id, substr(name, 1, 50) AS name, "
                "substr(anchor, 1, 180) AS anchor, anchor_model "
                "FROM brain_docs WHERE anchor IS NOT NULL "
                "ORDER BY RANDOM() LIMIT ?",
                (sample_n,),
            ).fetchall():
                sample.append({
                    "id": r["id"],
                    "name": r["name"],
                    "anchor": r["anchor"],
                    "model": r["anchor_model"],
                })

        return {
            "db": db_path,
            "migration_status": "applied",
            "total_docs": total,
            "anchored_docs": anchored,
            "unanchored_docs": total - anchored,
            "coverage": _human_pct(anchored, total),
            "anchor_read_path_active": anchored * 2 >= total and total > 0,
            "by_doc_type": by_type,
            "model_distribution": model_dist,
            "anchor_length_chars": {"avg": avg_len, "min": min_len, "max": max_len},
            "brain_anchors_fts_healthy": fts_ok,
            "brain_anchors_fts_rows": fts_count,
            "recent_anchors": recent,
            "sample_anchors": sample,
            "last_backfill_event": last_event,
            "generated_at": datetime.now().isoformat(timespec="seconds"),
        }
    finally:
        conn.close()


def _format_human(status: dict) -> str:
    lines = []
    lines.append("═" * 72)
    lines.append("  Harvey Brain Anchors — status dashboard")
    lines.append("═" * 72)
    lines.append(f"  db:               {status['db']}")
    lines.append(f"  migration:        {status['migration_status']}")
    lines.append("")
    total = status.get("total_docs", 0)
    anchored = status.get("anchored_docs", 0)
    lines.append(f"  brain_docs total: {total}")
    lines.append(f"  anchored:         {anchored} ({status.get('coverage', '?')})")
    lines.append(f"  unanchored:       {status.get('unanchored_docs', 0)}")
    active = status.get("anchor_read_path_active", False)
    gate = "ACTIVE (coverage ≥50%)" if active else "standby (coverage <50% — classic query path still used)"
    lines.append(f"  read-path gate:   {gate}")
    lines.append("")
    bt = status.get("by_doc_type", {})
    if bt:
        lines.append("  by doc_type:")
        for k, v in bt.items():
            lines.append(f"    {k:<10} {v['anchored']:>5} / {v['total']:<5} ({v['coverage']})")
        lines.append("")
    md = status.get("model_distribution", {})
    if md:
        lines.append("  anchor_model distribution:")
        for k, v in md.items():
            lines.append(f"    {k:<35} {v:>5}")
        lines.append("")
    stats = status.get("anchor_length_chars", {})
    if stats:
        lines.append(
            f"  anchor length:    avg={stats.get('avg', 0)}ch  "
            f"min={stats.get('min', 0)}ch  max={stats.get('max', 0)}ch"
        )
    fts_ok = status.get("brain_anchors_fts_healthy")
    if fts_ok is True:
        lines.append(f"  brain_anchors_fts: ok ({status.get('brain_anchors_fts_rows', 0)} indexed)")
    else:
        lines.append(f"  brain_anchors_fts: {fts_ok}")
    lines.append("")
    recent = status.get("recent_anchors", [])
    if recent:
        lines.append("  recent anchors (latest 5):")
        for r in recent[:5]:
            lines.append(f"    [{r['id']}] ({r['model']}) {r['name']}")
            lines.append(f"        {r['anchor'][:110]}")
    sample = status.get("sample_anchors", [])
    if sample:
        lines.append("")
        lines.append("  random sample:")
        for r in sample:
            lines.append(f"    [{r['id']}] ({r['model']}) {r['name']}")
            lines.append(f"        {r['anchor'][:160]}")
    lbe = status.get("last_backfill_event")
    if lbe:
        lines.append("")
        lines.append(f"  last backfill event: {lbe['at']}")
        lines.append(f"    {lbe['summary']}")
    lines.append("")
    lines.append(f"  generated at: {status.get('generated_at', '?')}")
    lines.append("═" * 72)
    return "\n".join(lines)


def _tail_backfill_log(n: int = 20) -> str:
    p = Path(BACKFILL_LOG)
    if not p.exists():
        return f"(no backfill log at {BACKFILL_LOG})"
    try:
        with p.open("r", encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
    except Exception as e:
        return f"(error reading log: {e})"
    if not lines:
        return "(log is empty)"
    out = ["── backfill log tail ──"]
    out.extend(line.rstrip() for line in lines[-n:])
    # Also extract latest progress line if any
    progress_re = re.compile(r"progress (\d+)/(\d+)")
    latest_progress = None
    for line in reversed(lines):
        m = progress_re.search(line)
        if m:
            latest_progress = (int(m.group(1)), int(m.group(2)))
            break
    if latest_progress:
        done, total = latest_progress
        out.append(f"── latest progress line: {done}/{total} ({_human_pct(done, total)}) ──")
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description="brain-anchors status dashboard")
    ap.add_argument("--db", default=DEFAULT_DB)
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    ap.add_argument("--sample", type=int, default=0, help="include N random anchors")
    ap.add_argument("--model", action="store_true", help="model distribution only")
    ap.add_argument("--tail", action="store_true", help="also tail the backfill log")
    args = ap.parse_args()

    status = gather_status(args.db, sample_n=args.sample)

    if args.model:
        print(json.dumps(status.get("model_distribution", {}), indent=2))
        return 0

    if args.json:
        print(json.dumps(status, indent=2, default=str))
    else:
        print(_format_human(status))

    if args.tail:
        print()
        print(_tail_backfill_log(30))

    return 0


if __name__ == "__main__":
    sys.exit(main())
