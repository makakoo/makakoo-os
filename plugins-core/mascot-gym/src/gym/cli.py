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
    improve <path> --goal ...   Run GYM on an arbitrary artifact (autoreseach model)

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
import time
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
        tofile=f"b/{new_path.name}",
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


def _cmd_improve(args: argparse.Namespace) -> int:
    """
    Run GYM on an arbitrary artifact (autoreseach model).

    Usage:
      harvey improve /path/to/file.py --goal "make more concise"
      harvey improve /path/to/SKILL.md --goal "improve instructions"
      harvey improve /path/to/config.toml --goal "simplify" --budget 300

    Pipeline:
      1. Create ArtifactCandidate from path
      2. Detect handler (code/skill/config/text/agent_flow/generic)
      3. Propose improvement via AIL (within budget_sec)
      4. Validate both baseline and candidate
      5. Output: score, val_bpb, hard failures, risk warnings

    Autoreseach equivalent: train.py → evaluate_bpb() → val_bpb score.
    """
    from gym.handlers import run_gym_improve, detect_handler
    from gym.artifact import ArtifactCandidate

    artifact_path = Path(args.path).expanduser().resolve()
    if not artifact_path.exists():
        print(f"error: file not found: {artifact_path}", file=sys.stderr)
        return 1

    goal = args.goal or "improve this artifact"
    budget = args.budget or 300

    print(f"GYM improve: {artifact_path}")
    print(f"  goal:    {goal}")
    print(f"  budget:  {budget}s")

    # Detect handler
    content = artifact_path.read_text(errors="ignore")
    handler = detect_handler(artifact_path, content)
    handler_name = type(handler).__name__ if handler else "GenericHandler"
    print(f"  handler: {handler_name}")
    print()

    # Run pipeline
    t0 = time.time()
    result = run_gym_improve(artifact_path, goal=goal, budget_sec=budget)
    elapsed = time.time() - t0

    if "error" in result:
        print(f"\nerror: {result['error']}")
        return 1

    eval_result = result["evaluation"]

    print()
    print(f"  elapsed:     {elapsed:.1f}s")
    print(f"  baseline:    {eval_result['baseline_score']:.4f}")
    print(f"  candidate:   {eval_result['candidate_score']:.4f}")
    print(f"  val_bpb:     {eval_result['val_bpb']:+.4f}")
    print(f"  simplicity:  {eval_result['simplicity_delta']:+.0f} lines")
    print(f"  hard fail:   {len(eval_result['hard_failures'])}")
    print(f"  risks:       {len(eval_result['risk_warnings'])}")
    print(f"  passed:      {eval_result['passed']}")

    if eval_result["hard_failures"]:
        print()
        print("  HARD FAILURES:")
        for f in eval_result["hard_failures"]:
            print(f"    - {f}")

    if eval_result["risk_warnings"]:
        print()
        print("  RISK WARNINGS:")
        for r in eval_result["risk_warnings"]:
            print(f"    - {r}")

    if args.diff:
        print()
        print("  CANDIDATE PREVIEW (first 40 lines):")
        print("  ---")
        for i, line in enumerate(result["proposal_preview"].splitlines()[:40]):
            print(f"    {i+1:3d} {line}")

    if not args.dry_run and eval_result["passed"]:
        print()
        print(f"  [READY FOR REVIEW] Run: harvey improve review")
    elif not args.dry_run and not eval_result["passed"]:
        print()
        print(f"  [NOT PASSED] val_bpb={eval_result['val_bpb']:.4f} — not auto-approvable")

    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="harvey improve",
        description="Human-in-the-loop merge gate for Harvey's Mascot GYM",
    )
    sub = parser.add_subparsers(dest="subcommand", required=True)

    p_review = sub.add_parser("review", help="List approved hypotheses awaiting review")
    p_review.add_argument("--diff", action="store_true", help="Show full diff for each")
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

    p_improve = sub.add_parser(
        "improve",
        help="Run GYM on an arbitrary artifact (autoreseach model)",
    )
    p_improve.add_argument(
        "path",
        help="Path to artifact (code, SKILL.md, config, text, agent flow)",
    )
    p_improve.add_argument(
        "--goal",
        help="What to improve (default: 'improve this artifact')",
    )
    p_improve.add_argument(
        "--budget", type=int, default=300,
        help="Max seconds (default: 300)",
    )
    p_improve.add_argument(
        "--diff", action="store_true",
        help="Show candidate preview",
    )
    p_improve.add_argument(
        "--dry-run", action="store_true",
        help="Score only, no proposal",
    )
    p_improve.set_defaults(func=_cmd_improve)

    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())