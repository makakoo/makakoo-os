"""
Layer 4 of Harvey's Mascot GYM — lope validation gate.

Every pending sprint from Layer 3 runs through lope's validator pool.
We reuse lope's ValidatorPool directly instead of shelling out to
`lope negotiate`, because (a) our sprints are pre-drafted so the
drafter/refine loop is wrong for us, and (b) thrashing through the
escalated-with-lint-wedge failure mode of today's lope_negotiate is
actively harmful — we want a one-shot verdict on an existing doc.

Only PASS reaches the human queue. NEEDS_FIX / FAIL / INFRA_ERROR all
route to rejected/ with a critique file, because a human reviewing a
noisy escalated queue will eventually stop reviewing anything.

All lope imports are lazy so the module can be imported in tests
that never touch the real validator pool.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
IMPROVEMENTS_DIR = Path(HARVEY_HOME) / "data" / "improvements"
PENDING_DIR = IMPROVEMENTS_DIR / "pending"
APPROVED_DIR = IMPROVEMENTS_DIR / "approved"
REJECTED_DIR = IMPROVEMENTS_DIR / "rejected"
PROVENANCE_DIR = IMPROVEMENTS_DIR / "provenance"

DEFAULT_TIMEOUT_SECONDS = 300


@dataclass
class ValidationVerdict:
    hypothesis_id: str
    status: str   # "pass" | "needs_fix" | "fail" | "infra_error" | "malformed"
    primary_validator: str
    confidence: float
    rationale: str
    required_fixes: List[str] = field(default_factory=list)
    evaluated_at: str = ""

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass
class ValidationSummary:
    date: str
    approved: List[str]
    rejected: List[str]
    malformed: List[str]
    errors: List[str]

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


# ----------------------------------------------------------------------------
# Lope adapter — lazy import
# ----------------------------------------------------------------------------


def _lope_validate(sprint_text: str, sprint_goal: str, timeout: int) -> Dict[str, Any]:
    """
    Run lope's ValidatorPool on an existing sprint doc. Returns a plain
    dict so callers don't depend on lope's types crossing module boundaries.

    On any lope import or subprocess failure, returns a synthetic
    infra_error verdict — we never let lope crashes take down the gate.
    """
    try:
        sys.path.insert(0, os.path.expanduser("~/.lope"))
        from lope.validators import build_validator_pool, VerdictStatus
        from lope.config import load_layered
        from lope.negotiator import _build_validator_prompt
        from lope.models import Proposal
    except Exception as exc:
        return {
            "status": "infra_error",
            "primary_validator": "pool",
            "confidence": 0.0,
            "rationale": f"lope import failed: {exc!r}",
            "required_fixes": [],
        }

    try:
        cfg = load_layered()
        pool = build_validator_pool(cfg)
    except Exception as exc:
        return {
            "status": "infra_error",
            "primary_validator": "pool",
            "confidence": 0.0,
            "rationale": f"validator pool build failed: {exc!r}",
            "required_fixes": [],
        }

    try:
        proposal = Proposal(
            round_number=1,
            goal=sprint_goal,
            text=sprint_text,
            prior_feedback="",
        )
        prompt = _build_validator_prompt(sprint_goal, proposal, domain="engineering")
        result = pool.validate(prompt, timeout=timeout)
        verdict = result.verdict
        status_map = {
            VerdictStatus.PASS: "pass",
            VerdictStatus.NEEDS_FIX: "needs_fix",
            VerdictStatus.FAIL: "fail",
            VerdictStatus.INFRA_ERROR: "infra_error",
        }
        return {
            "status": status_map.get(verdict.status, "infra_error"),
            "primary_validator": pool.primary_validator().name if hasattr(pool, "primary_validator") else "unknown",
            "confidence": float(getattr(verdict, "confidence", 0.0) or 0.0),
            "rationale": verdict.rationale or "",
            "required_fixes": list(getattr(verdict, "required_fixes", []) or []),
        }
    except Exception as exc:
        return {
            "status": "infra_error",
            "primary_validator": "pool",
            "confidence": 0.0,
            "rationale": f"pool.validate failed: {exc!r}",
            "required_fixes": [],
        }


# ----------------------------------------------------------------------------
# Sprint doc helpers
# ----------------------------------------------------------------------------


def _extract_goal(sprint_text: str) -> str:
    """Best-effort: extract the Origin section as the validator's 'goal'."""
    lines = sprint_text.splitlines()
    title = "Harvey GYM hypothesis"
    for line in lines:
        if line.startswith("# "):
            title = line[2:].strip()
            break
    # Pull the Origin block
    in_origin = False
    origin_lines: List[str] = []
    for line in lines:
        if line.strip().startswith("## Origin"):
            in_origin = True
            continue
        if in_origin and line.startswith("## "):
            break
        if in_origin:
            origin_lines.append(line)
    origin = "\n".join(origin_lines).strip()
    if origin:
        return f"{title}\n\n{origin}"
    return title


def _is_valid_sprint(text: str) -> bool:
    """Cheap sanity check: Phase 1 header + non-trivial length."""
    if len(text.strip()) < 200:
        return False
    if "## Phase 1" not in text:
        return False
    return True


# ----------------------------------------------------------------------------
# Main entry point
# ----------------------------------------------------------------------------


def validate_pending(
    date: Optional[str] = None,
    timeout: int = DEFAULT_TIMEOUT_SECONDS,
    validate_fn: Optional[Callable[[str, str, int], Dict[str, Any]]] = None,
) -> ValidationSummary:
    """
    Run lope validation across every pending hypothesis.

    `validate_fn` is an optional injection point for tests — production
    leaves it None and the real lope pool is used. Fake implementations
    should accept (sprint_text, sprint_goal, timeout) and return a dict
    matching the shape _lope_validate returns.
    """
    date = date or datetime.now(timezone.utc).strftime("%Y-%m-%d")
    _ensure_dirs()

    validator = validate_fn or _lope_validate
    # Hypothesis IDs start with a timestamp; skip *.skill.md improved blobs
    pending = sorted(
        p for p in PENDING_DIR.glob("2*.md") if not p.name.endswith(".skill.md")
    )

    summary = ValidationSummary(
        date=date,
        approved=[],
        rejected=[],
        malformed=[],
        errors=[],
    )

    for sprint_path in pending:
        hyp_id = sprint_path.stem
        try:
            text = sprint_path.read_text(encoding="utf-8")
        except OSError as exc:
            summary.errors.append(f"{hyp_id}: read failed: {exc}")
            continue

        if not _is_valid_sprint(text):
            _move_to_rejected(hyp_id, sprint_path, reason="malformed sprint doc")
            summary.malformed.append(hyp_id)
            continue

        goal = _extract_goal(text)
        verdict_dict = validator(text, goal, timeout)
        verdict = ValidationVerdict(
            hypothesis_id=hyp_id,
            status=verdict_dict.get("status", "infra_error"),
            primary_validator=verdict_dict.get("primary_validator", "unknown"),
            confidence=float(verdict_dict.get("confidence", 0.0)),
            rationale=verdict_dict.get("rationale", ""),
            required_fixes=list(verdict_dict.get("required_fixes", [])),
            evaluated_at=datetime.now(timezone.utc).isoformat(),
        )

        if verdict.status == "pass":
            _move_to_approved(hyp_id, sprint_path, verdict)
            summary.approved.append(hyp_id)
        else:
            _move_to_rejected(
                hyp_id,
                sprint_path,
                reason=f"{verdict.status} ({verdict.primary_validator}): {verdict.rationale}",
                verdict=verdict,
            )
            summary.rejected.append(hyp_id)

    return summary


# ----------------------------------------------------------------------------
# File moves
# ----------------------------------------------------------------------------


def _ensure_dirs() -> None:
    for d in (PENDING_DIR, APPROVED_DIR, REJECTED_DIR, PROVENANCE_DIR):
        d.mkdir(parents=True, exist_ok=True)


def _move_pair(hyp_id: str, sprint_path: Path, target_dir: Path) -> Path:
    """
    Move the sprint .md and its sibling .skill.md (if present) to
    target_dir. Returns the new sprint path.
    """
    target_dir.mkdir(parents=True, exist_ok=True)
    new_sprint = target_dir / sprint_path.name
    shutil.move(str(sprint_path), str(new_sprint))
    skill_blob = sprint_path.with_name(f"{hyp_id}.skill.md")
    if skill_blob.exists():
        shutil.move(str(skill_blob), str(target_dir / skill_blob.name))
    return new_sprint


def _move_to_approved(hyp_id: str, sprint_path: Path, verdict: ValidationVerdict) -> None:
    _move_pair(hyp_id, sprint_path, APPROVED_DIR)
    (APPROVED_DIR / f"{hyp_id}.verdict.json").write_text(
        json.dumps(verdict.to_dict(), indent=2, ensure_ascii=False),
        encoding="utf-8",
    )


def _move_to_rejected(
    hyp_id: str,
    sprint_path: Path,
    reason: str,
    verdict: Optional[ValidationVerdict] = None,
) -> None:
    _move_pair(hyp_id, sprint_path, REJECTED_DIR)
    critique = [
        f"# Rejection for {hyp_id}",
        "",
        f"**Reason:** {reason}",
        "",
    ]
    if verdict:
        critique.extend([
            f"**Primary validator:** {verdict.primary_validator}",
            f"**Confidence:** {verdict.confidence:.2f}",
            f"**Rationale:** {verdict.rationale}",
            "",
        ])
        if verdict.required_fixes:
            critique.append("**Required fixes:**")
            for f in verdict.required_fixes:
                critique.append(f"- {f}")
    (REJECTED_DIR / f"{hyp_id}.critique.md").write_text("\n".join(critique), encoding="utf-8")
    if verdict:
        (REJECTED_DIR / f"{hyp_id}.verdict.json").write_text(
            json.dumps(verdict.to_dict(), indent=2, ensure_ascii=False),
            encoding="utf-8",
        )


# ----------------------------------------------------------------------------
# Morning report (Brain journal rollup)
# ----------------------------------------------------------------------------


def build_morning_report(date: Optional[str] = None) -> str:
    """
    Produce a Logseq-outliner-format block summarizing the GYM's overnight
    work. Every line starts with `- ` so it drops cleanly into a journal.
    """
    date = date or datetime.now(timezone.utc).strftime("%Y-%m-%d")
    _ensure_dirs()

    # Today's clustered errors
    clustered_path = Path(HARVEY_HOME) / "data" / "errors" / date / "clustered.json"
    clustered: Dict[str, Any] = {}
    if clustered_path.exists():
        try:
            clustered = json.loads(clustered_path.read_text())
        except (OSError, json.JSONDecodeError):
            pass

    approved = sorted(
        p for p in APPROVED_DIR.glob("2*.md") if not p.name.endswith(".skill.md")
    )
    rejected = sorted(
        p for p in REJECTED_DIR.glob("2*.md") if not p.name.endswith(".skill.md")
    )
    pending = sorted(
        p for p in PENDING_DIR.glob("2*.md") if not p.name.endswith(".skill.md")
    )

    lines: List[str] = []
    lines.append(f"- [[Harvey Mascot GYM]] morning report — {date}")
    lines.append(f"  - Errors captured: {clustered.get('total_entries', 0)}")
    by_class = clustered.get("by_class", {})
    if by_class:
        parts = ", ".join(f"{k}={v}" for k, v in by_class.items() if v)
        lines.append(f"  - By class: {parts or 'none'}")
    lines.append(f"  - Clusters: {clustered.get('cluster_count', 0)}")
    lines.append(f"  - Hypotheses awaiting review: {len(pending)}")
    lines.append(f"  - Approved overnight: {len(approved)}")
    if approved:
        for p in approved[:5]:
            lines.append(f"    - [[{p.stem}]]")
    lines.append(f"  - Rejected overnight: {len(rejected)}")
    if rejected:
        for p in rejected[:5]:
            lines.append(f"    - [[{p.stem}]]")
    lines.append("  - Next action: `harvey improve review` to inspect approved queue")
    return "\n".join(lines) + "\n"
