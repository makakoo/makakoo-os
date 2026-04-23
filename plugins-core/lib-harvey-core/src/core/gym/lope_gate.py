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

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
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
    behavioral_passed: int = 0   # CODE sprints that passed behavioral validation
    behavioral_failed: int = 0  # CODE sprints that failed behavioral validation

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
    if len(text.strip()) < 100:
        return False
    # Skill sprints have "Phase 1", code sprints have "Apply LLM-generated unified diff"
    if "## Phase 1" not in text and "Apply LLM-generated unified diff" not in text:
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
        behavioral_passed=0,
        behavioral_failed=0,
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
            # ── CODE-class: run behavioral validation ───────────────────────
            if _is_code_sprint(sprint_path):
                beh = _behavioral_validate(hyp_id, timeout=timeout)
                prov = {}
                prov_path_local = PROVENANCE_DIR / f"{hyp_id}.json"
                if prov_path_local.exists():
                    try:
                        prov = json.loads(prov_path_local.read_text(encoding="utf-8"))
                    except (OSError, json.JSONDecodeError):
                        pass

                if beh is None:
                    # Soft skip: no test command available — approve anyway
                    # (lope gate already validated the sprint doc)
                    _move_to_approved(hyp_id, sprint_path, verdict)
                    summary.approved.append(hyp_id)
                elif not beh.passed:
                    reason = (
                        f"behavioral FAIL: `{beh.test_command}` "
                        f"still raises {beh.exception_still_present or 'error'} "
                        f"(exit {beh.behavioral_exit_code})"
                    )
                    verdict.rationale += f" | {reason}"
                    verdict.status = "behavioral_fail"
                    _move_to_rejected(hyp_id, sprint_path, reason=reason, verdict=verdict)
                    summary.rejected.append(hyp_id)
                    summary.behavioral_failed += 1
                else:
                    # Behavioral PASS — error is gone after patch
                    verdict.rationale += (
                        f" | behavioral PASS: `{beh.test_command}` "
                        f"no longer raises {prov.get('skill', 'the error')} "
                        f"(exit {beh.behavioral_exit_code})"
                    )
                    _move_to_approved(hyp_id, sprint_path, verdict)
                    summary.approved.append(hyp_id)
                    summary.behavioral_passed += 1
            else:
                # SKILL-class: lope gate PASS is sufficient
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
# Behavioral validation — CODE-class sprints only
# ----------------------------------------------------------------------------
#
# After the lope gate passes a CODE-class sprint, we run a behavioral test:
#   1. Load the original error entries from error_ids to extract the failing cmd
#   2. Apply the patch to a temporary copy of the target file
#   3. Run the original failing command
#   4. If the same exception still fires → behavioral FAIL → reject
#   5. If the error is gone → behavioral PASS → approve
#
# This uses the Meta-Harness approach (tmux sandbox) for isolation, but since
# we already have the error context, a simple subprocess in /tmp is sufficient
# for Python module-level errors. Full tmux sandbox is reserved for when
# agent-level behavioral tests are needed.



def _load_error_entry(error_id: str) -> Dict[str, Any]:
    """
    Reconstruct error entry from an error_id like "bash:3".

    error_id format: "<source>:<line_index>"
    e.g. "bash:3" → line 3 in bash.jsonl under the current day's errors dir.
    """
    parts = error_id.rsplit(":", 1)
    if len(parts) != 2:
        return {}
    source, idx_str = parts
    try:
        idx = int(idx_str)
    except ValueError:
        return {}

    errors_dir = Path(HARVEY_HOME) / "data" / "errors"
    if not errors_dir.exists():
        return {}

    # Find most recent day directory
    days = sorted(errors_dir.iterdir(), reverse=True)
    for day_dir in days:
        if not day_dir.is_dir():
            continue
        jsonl_path = day_dir / f"{source}.jsonl"
        if not jsonl_path.exists():
            continue
        try:
            with open(jsonl_path, "r", encoding="utf-8") as f:
                for line_idx, line in enumerate(f):
                    if line_idx != idx:
                        continue
                    return json.loads(line.strip())
        except (OSError, json.JSONDecodeError, ValueError):
            continue
    return {}


def _extract_test_command(error_ids: List[str]) -> Optional[str]:
    """
    Extract a reproducible test command from the original error entries.

    Prefers the most common cmd across entries. Falls back to any cmd
    that exercises the buggy file.
    """
    cmd_counts: Dict[str, int] = {}
    for eid in error_ids:
        entry = _load_error_entry(eid)
        cmd = entry.get("cmd", "")
        if cmd and len(cmd) < 500:  # sanity limit on cmd length
            cmd_counts[cmd] = cmd_counts.get(cmd, 0) + 1

    if cmd_counts:
        return max(cmd_counts, key=lambda c: cmd_counts[c])
    return None


@dataclass
class BehavioralResult:
    passed: bool
    test_command: str
    behavioral_stdout: str
    behavioral_stderr: str
    behavioral_exit_code: int
    exception_still_present: Optional[str]  # e.g. "KeyError" if same exception fires


def _behavioral_validate(
    hypothesis_id: str,
    timeout: int = 60,
) -> Optional[BehavioralResult]:
    """
    Run behavioral validation for a CODE-class sprint.

    Returns a BehavioralResult, or None if validation could not run (e.g.
    no test command could be extracted, patch couldn't be applied).

    On error, returns a BehavioralResult with passed=False.
    """
    prov_path = PROVENANCE_DIR / f"{hypothesis_id}.json"
    if not prov_path.exists():
        return None
    try:
        prov = json.loads(prov_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None

    patch_type = prov.get("patch_type", "")
    if patch_type != "code":
        return None  # Only validate CODE-class sprints

    patch_path_str = prov.get("code_patch_path", "")
    file_path_str = prov.get("code_file_path", "")
    error_ids: List[str] = prov.get("source_error_ids", [])


    if not patch_path_str or not file_path_str:
        return None

    patch_path = Path(patch_path_str)
    file_path = Path(file_path_str)

    if not patch_path.exists() or not file_path.exists():
        return None

    # Extract test command from original error entries
    test_cmd = _extract_test_command(error_ids)
    if not test_cmd:
        # Cannot validate without a known failing command
        # Return a soft skip — this should not happen if the hypothesis
        # was generated correctly (error_ids are always set)
        return None

    # Build a temp directory with the patched file for isolation
    try:
        with tempfile.TemporaryDirectory(prefix="gym_behavioral_") as tmpdir:
            tmpdir_p = Path(tmpdir)
            patched_file = tmpdir_p / file_path.name
            # Copy original file
            shutil.copy2(file_path, patched_file)

            # Apply patch to temp copy
            result = subprocess.run(
                ["patch", "-p1", "-i", str(patch_path)],
                capture_output=True, text=True,
                cwd=str(tmpdir_p),
            )
            if result.returncode != 0:
                return BehavioralResult(
                    passed=False,
                    test_command=test_cmd,
                    behavioral_stdout=result.stdout[:500],
                    behavioral_stderr=result.stderr[:500],
                    behavioral_exit_code=result.returncode,
                    exception_still_present=None,
                )

            # Run the failing command against the patched file
            # Replace absolute paths in the command with the temp copy
            cmd_for_tmp = test_cmd.replace(str(file_path), str(patched_file))

            beh_result = subprocess.run(
                cmd_for_tmp,
                shell=True,
                capture_output=True, text=True, timeout=timeout,
                cwd=str(tmpdir_p),
            )

            stderr_lower = beh_result.stderr.lower()
            stdout_lower = beh_result.stdout.lower()

            # Check if the original exception is still present
            original_exception = prov.get("skill", "")  # skill field = exception type for code patches
            still_present = None
            if original_exception and original_exception in (
                "ImportError", "ModuleNotFoundError", "AttributeError",
                "KeyError", "TypeError", "ValueError", "IndexError",
                "RuntimeError", "TimeoutError", "ConnectionError",
                "FileNotFoundError", "PermissionError",
                "JSONDecodeError", "OperationalError",
            ):
                combined = stderr_lower + stdout_lower
                if original_exception.lower() in combined:
                    still_present = original_exception

            passed = still_present is None and beh_result.returncode == 0

            return BehavioralResult(
                passed=passed,
                test_command=test_cmd,
                behavioral_stdout=beh_result.stdout[:500],
                behavioral_stderr=beh_result.stderr[:500],
                behavioral_exit_code=beh_result.returncode,
                exception_still_present=still_present,
            )
    except subprocess.TimeoutExpired:
        return BehavioralResult(
            passed=False,
            test_command=test_cmd,
            behavioral_stdout="",
            behavioral_stderr="command timed out",
            behavioral_exit_code=-1,
            exception_still_present=None,
        )
    except Exception as exc:
        return BehavioralResult(
            passed=False,
            test_command=test_cmd or "",
            behavioral_stdout="",
            behavioral_stderr=str(exc)[:200],
            behavioral_exit_code=-1,
            exception_still_present=None,
        )


def _is_code_sprint(sprint_path: Path) -> bool:
    """True if this sprint targets a CODE-class hypothesis."""
    try:
        text = sprint_path.read_text(encoding="utf-8")
        return "Apply LLM-generated unified diff" in text
    except OSError:
        return False


def _ensure_dirs() -> None:
    for d in (PENDING_DIR, APPROVED_DIR, REJECTED_DIR, PROVENANCE_DIR):
        d.mkdir(parents=True, exist_ok=True)


def _move_pair(hyp_id: str, sprint_path: Path, target_dir: Path) -> Path:
    """
    Move the sprint .md and its sibling artifacts to target_dir:
      - *.skill.md   — improved SKILL.md blob (skill-class sprints)
      - *.patch      — unified diff (code-class sprints)
    Returns the new sprint path.
    """
    target_dir.mkdir(parents=True, exist_ok=True)
    new_sprint = target_dir / sprint_path.name
    shutil.move(str(sprint_path), str(new_sprint))

    # Move skill improved blob
    skill_blob = sprint_path.with_name(f"{hyp_id}.skill.md")
    if skill_blob.exists():
        shutil.move(str(skill_blob), str(target_dir / skill_blob.name))

    # Move code patch file
    patch_file = sprint_path.with_name(f"{hyp_id}.patch")
    if patch_file.exists():
        shutil.move(str(patch_file), str(target_dir / patch_file.name))

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
    code_targets = clustered.get("code_targets", [])
    if code_targets:
        lines.append(f"  - Code targets detected: {len(code_targets)}")
        for t in code_targets[:3]:
            fp = t.get("file_path", "?")
            exc = t.get("exception", "?")
            cnt = t.get("count", 0)
            lines.append(f"    - {exc} in {fp} (x{cnt})")
    lines.append(f"  - Clusters: {clustered.get('cluster_count', 0)}")
    lines.append(f"  - Hypotheses awaiting review: {len(pending)}")
    lines.append(f"  - Approved overnight: {len(approved)}")
    if approved:
        for p in approved[:5]:
            lines.append(f"    - [[{p.stem}]]")
    lines.append(f"  - Rejected overnight: {len(rejected)}")
    if rejected:
        for p in rejected[:5]:
            # Check if it was a behavioral fail
            verdict_p = REJECTED_DIR / f"{p.stem}.verdict.json"
            suffix = ""
            if verdict_p.exists():
                try:
                    v = json.loads(verdict_p.read_text())
                    if v.get("status") == "behavioral_fail":
                        suffix = " (behavioral)"
                except (OSError, json.JSONDecodeError):
                    pass
            lines.append(f"    - [[{p.stem}]]{suffix}")
    lines.append("  - Next action: `harvey improve review` to inspect approved queue")
    return "\n".join(lines) + "\n"
