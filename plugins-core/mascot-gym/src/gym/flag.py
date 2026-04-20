"""
Layer 1b of Harvey's Mascot GYM — manual feedback funnel (`harvey flag`).

Sebastian flags a wrong response with:
    harvey flag "response was wrong because X"
    harvey flag --skill meta/caveman-voice "used full prose for internal update"
    harvey flag                                 # pops $EDITOR if no reason given

The entry lands in data/errors/<date>/manual_flag.jsonl with source="manual_flag"
and error_class pre-set to "skill" — Layer 2's classifier trusts the
pre-label and skips regex cascade, Layer 3's hypothesis generator picks
these up with the same treatment as clustered technical failures.

Why manual flags exist: technical errors (crashes, non-zero exits, raised
exceptions) are a minority of Harvey's actual failures. Most wrong answers
are confidently-stated text that doesn't raise anything. Without manual
flagging, 80% of the real learning signal is invisible.

HARD RULES:
    - Every flag writes exactly one jsonl line. No batching.
    - Flags are append-only. Never rewritten, never deleted autonomously.
    - The reason is mandatory. An empty reason is refused with exit 2.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

try:
    from core.gym.capture import log_error, ErrorSource
except ImportError:
    sys.path.insert(0, os.path.join(
        os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
        "harvey-os",
    ))
    from core.gym.capture import log_error, ErrorSource


MAX_TRANSCRIPT_BYTES = 8192


def add_flag(
    reason: str,
    skill: Optional[str] = None,
    context: Optional[str] = None,
    transcript_path: Optional[str] = None,
    cmd_label: Optional[str] = None,
) -> bool:
    """
    Write a single manual-flag entry into the GYM error funnel.

    Args:
        reason:           Why the response was wrong. Required, non-empty.
        skill:            Optional skill name to scope the flag to.
        context:          Optional additional text (paste of the response, etc.)
        transcript_path:  Optional Claude Code transcript file. If supplied,
                          up to MAX_TRANSCRIPT_BYTES of the tail gets captured
                          into raw.transcript_tail for hypothesis generation.
        cmd_label:        What Harvey did that was wrong (becomes the `cmd`
                          field). Defaults to "flagged response" if omitted.

    Returns True on successful write, False on capture failure. Never raises.
    """
    if not reason or not reason.strip():
        return False

    raw: Dict[str, Any] = {
        "flagged_at": datetime.now(timezone.utc).isoformat(),
        "flagged_by": os.environ.get("USER", "unknown"),
    }

    if context:
        raw["context"] = context[:MAX_TRANSCRIPT_BYTES]

    if transcript_path:
        tail = _read_transcript_tail(transcript_path)
        if tail:
            raw["transcript_tail"] = tail

    return log_error(
        source=ErrorSource.MANUAL_FLAG.value,
        cmd=cmd_label or "flagged response",
        stderr=reason.strip(),
        exit_code=None,
        agent=os.environ.get("HARVEY_AGENT", "harvey"),
        skill_in_scope=skill,
        raw=raw,
        error_class="skill",  # manual flags are always skill-level actionable
    )


def _read_transcript_tail(transcript_path: str) -> Optional[str]:
    """
    Best-effort capture of the last chunk of a Claude Code JSONL transcript.
    Returns None on any failure (missing file, unreadable, oversized).
    """
    try:
        p = Path(transcript_path)
        if not p.exists():
            return None
        size = p.stat().st_size
        if size == 0:
            return None
        with open(p, "rb") as f:
            if size > MAX_TRANSCRIPT_BYTES:
                f.seek(-MAX_TRANSCRIPT_BYTES, os.SEEK_END)
            return f.read().decode("utf-8", errors="replace")
    except OSError:
        return None


def _reason_from_editor() -> str:
    """Pop $EDITOR for an interactive reason. Returns empty string on abort."""
    editor = os.environ.get("EDITOR") or os.environ.get("VISUAL") or "vi"
    with tempfile.NamedTemporaryFile(
        mode="w+", suffix=".md", delete=False, prefix="harvey-flag-"
    ) as f:
        f.write(
            "# Write the reason this Harvey response was wrong.\n"
            "# Lines starting with # are ignored.\n"
            "# Save and quit to submit. Empty submission aborts.\n\n"
        )
        tmp = f.name
    try:
        subprocess.run([editor, tmp], check=False)
        text = Path(tmp).read_text(encoding="utf-8")
    finally:
        try:
            os.unlink(tmp)
        except OSError:
            pass
    lines = [
        line for line in text.splitlines()
        if not line.lstrip().startswith("#")
    ]
    return "\n".join(lines).strip()


# ----------------------------------------------------------------------------
# CLI
# ----------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="harvey flag",
        description=(
            "Flag a wrong Harvey response for the Mascot GYM to learn from. "
            "Feeds the manual-feedback funnel so the nightly improver can "
            "propose an edit to the relevant SKILL.md."
        ),
    )
    parser.add_argument(
        "reason",
        nargs="?",
        help="Why the response was wrong. If omitted, $EDITOR opens for input.",
    )
    parser.add_argument(
        "--skill",
        help="Skill name in scope (e.g. meta/caveman-voice). "
             "Defaults to HARVEY_SKILL_IN_SCOPE env if set.",
    )
    parser.add_argument(
        "--context",
        help="Additional context — paste of the response, stack, notes.",
    )
    parser.add_argument(
        "--transcript",
        help="Path to a Claude Code JSONL transcript for tail capture. "
             "Defaults to CLAUDE_TRANSCRIPT_PATH env if set.",
    )
    parser.add_argument(
        "--cmd",
        dest="cmd_label",
        help="Short label for what Harvey did (becomes the cmd field).",
    )
    return parser


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)

    reason = args.reason
    if not reason:
        reason = _reason_from_editor()

    if not reason or not reason.strip():
        print("error: reason is required (pass as argument or use $EDITOR)", file=sys.stderr)
        return 2

    transcript = args.transcript or os.environ.get("CLAUDE_TRANSCRIPT_PATH")
    skill = args.skill or os.environ.get("HARVEY_SKILL_IN_SCOPE")

    ok = add_flag(
        reason=reason,
        skill=skill,
        context=args.context,
        transcript_path=transcript,
        cmd_label=args.cmd_label,
    )
    if not ok:
        print("error: flag write failed (check stderr for gym.capture warning)", file=sys.stderr)
        return 1

    # Friendly confirmation — user wants to know the flag landed
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    log_file = Path(
        os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
    ) / "data" / "errors" / today / "manual_flag.jsonl"
    print(f"flagged: {reason[:80]}{'...' if len(reason) > 80 else ''}")
    print(f"  skill: {skill or '(none)'}")
    print(f"  logged to: {log_file}")
    print(f"  next classify tick picks this up; nightly gym_hypothesize attempts a fix")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
