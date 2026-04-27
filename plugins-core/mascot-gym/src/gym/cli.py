"""
`harvey improve` subcommand — Layer 5 of Harvey's Mascot GYM.

This is the human-in-the-loop merge gate. Every autonomous code path
in the GYM pipeline exists so you can invoke *this* command in the
morning with confidence.

Subcommands:
    review              List approved hypotheses with diffs + scores
    approve <id> [--note ...]   Git-commit the edit, tag, journal entry
    reject  <id> [--note ...]   Move to rejected/, log reason
    stats   [--since 7d]        Rollup counts + avg delta + revert rate

Hard rules (enforced inside core/gym/approval.py, not just here):
    - `approve ALL` is blocked with a hard error
    - Rejects to plugins-core/lib-harvey-core/src/core/* are blocked (human must hand-write)
    - Every approval must be explicit per hypothesis id
"""

from __future__ import annotations

import argparse
import difflib
import json
import sys
from pathlib import Path

try:
    from core.gym.approval import (
        ApprovalError,
        approve,
        list_approved,
        reject,
        stats,
    )
except ImportError:
    # Direct execution fallback — post-harvey-os retirement (2026-04-20).
    import os
    _home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME") \
        or os.path.expanduser("~/MAKAKOO")
    sys.path.insert(0, os.path.join(_home, "plugins-core", "lib-harvey-core", "src"))
    from core.gym.approval import (
        ApprovalError,
        approve,
        list_approved,
        reject,
        stats,
    )


def _cmd_review(args: argparse.Namespace) -> int:
    pending = list_approved()
    if not pending:
        print("No approved hypotheses awaiting review.")
        print("Run `harvey improve stats` to see the full pipeline state.")
        return 0

    print(f"{len(pending)} hypothesis(es) awaiting review:\n")
    for p in pending:
        print(f"  {p.id}")
        print(f"    skill:     {p.skill}")
        print(f"    delta:     +{p.delta}  ({p.baseline_score} → {p.improved_score})")
        print(f"    validator: {p.primary_validator} (conf {p.confidence:.2f})")
        if p.rationale:
            print(f"    rationale: {p.rationale[:120]}")
        if args.diff:
            print()
            print(_render_diff(p))
        print()
    print(f"To approve:  harvey improve approve <id>")
    print(f"To reject:   harvey improve reject <id> --note '<reason>'")
    return 0


def _render_diff(pending) -> str:
    old_path = Path(pending.skill_path)
    new_path = Path(pending.improved_blob_path)
    if not old_path.exists() or not new_path.exists():
        return "    (diff unavailable — files missing)"
    old_lines = old_path.read_text().splitlines(keepends=True)
    new_lines = new_path.read_text().splitlines(keepends=True)
    diff = difflib.unified_diff(
        old_lines,
        new_lines,
        fromfile=f"a/{old_path.name}",
        tofile=f"b/{old_path.name}",
        n=3,
    )
    indented = "".join("    " + line for line in diff)
    return indented or "    (no diff)"


def _cmd_approve(args: argparse.Namespace) -> int:
    try:
        result = approve(
            args.id,
            reviewer_note=args.note or "",
            dry_run=args.dry_run,
        )
    except ApprovalError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    if args.dry_run:
        print(f"DRY RUN — would apply {result.id} to {result.skill_path}")
        return 0

    print(f"Approved {result.id}")
    print(f"  git sha:  {result.git_sha}")
    print(f"  git tag:  {result.git_tag}")
    print(f"  skill:    {result.skill_path}")
    print()
    print("Journal entry:")
    for line in result.journal_entry.splitlines():
        print(f"  {line}")
    return 0


def _cmd_reject(args: argparse.Namespace) -> int:
    try:
        reject(args.id, reviewer_note=args.note or "")
    except ApprovalError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    print(f"Rejected {args.id}")
    return 0


def _cmd_stats(args: argparse.Namespace) -> int:
    s = stats()
    print("Harvey's Mascot GYM — pipeline stats\n")
    print("  Pipeline state:")
    for k, v in s["counts"].items():
        print(f"    {k:28s} {v}")
    print(f"\n  Hypotheses generated total: {s['hypotheses_generated_total']}")
    print(f"  Average delta (all hypotheses): {s['avg_delta']:.1f}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="harvey improve",
        description="Human-in-the-loop merge gate for Harvey's Mascot GYM",
    )
    sub = parser.add_subparsers(dest="subcommand", required=True)

    p_review = sub.add_parser("review", help="List approved hypotheses awaiting review")
    p_review.add_argument("--diff", action="store_true", help="Show full SKILL.md diff for each")
    p_review.set_defaults(func=_cmd_review)

    p_approve = sub.add_parser("approve", help="Git-commit an approved hypothesis")
    p_approve.add_argument("id", help="Hypothesis id (timestamp-hash). 'ALL' is refused.")
    p_approve.add_argument("--note", help="Reviewer note attached to the commit + journal entry")
    p_approve.add_argument("--dry-run", action="store_true", help="Preview without writing")
    p_approve.set_defaults(func=_cmd_approve)

    p_reject = sub.add_parser("reject", help="Move an approved hypothesis to rejected/")
    p_reject.add_argument("id")
    p_reject.add_argument("--note", help="Reason")
    p_reject.set_defaults(func=_cmd_reject)

    p_stats = sub.add_parser("stats", help="Pipeline counts + delta rollup")
    p_stats.add_argument("--since", help="Rolling window, e.g. '7d' (not yet wired)")
    p_stats.set_defaults(func=_cmd_stats)

    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
