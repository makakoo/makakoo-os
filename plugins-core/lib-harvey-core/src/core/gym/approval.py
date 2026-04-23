"""
Layer 5 of Harvey's Mascot GYM — human approval API.

This is the autonomy stop point. Nothing merges without an explicit
`harvey improve approve <id>` call. The approval path makes a git
commit + lightweight tag + Brain journal entry, so every change has
three independent audit trails.

Merge policy (hard-coded for Gate 1 — do NOT weaken without a new sprint):
    1. approve() MUST NOT be called in any autonomous code path.
    2. `harvey improve approve ALL` is rejected with a hard error.
    3. Rejects to core plugin code (lib-harvey-core, makakoo-os/makakoo/
       makakoo-core/, makakoo-mcp/) are blocked — humans sign those.
    4. Only files listed in the pending sprint's "Files" section are
       touched. Any drift between the sprint and the improved blob is
       a hard error.

These rules are encoded as pre-approval checks inside approve().
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
IMPROVEMENTS_DIR = Path(HARVEY_HOME) / "data" / "improvements"
APPROVED_DIR = IMPROVEMENTS_DIR / "approved"
REJECTED_DIR = IMPROVEMENTS_DIR / "rejected"
MERGED_DIR = IMPROVEMENTS_DIR / "merged"
PROVENANCE_DIR = IMPROVEMENTS_DIR / "provenance"

# Paths the autoimprover is NEVER allowed to touch — human-signed territory.
# Updated 2026-04-20 after harvey-os retirement: the previous single-string
# prefix pointed at the archived tree and matched nothing post-rename,
# silently disabling this guard. List now enumerates every core tree.
CORE_PATH_PREFIXES: tuple[str, ...] = (
    "plugins-core/lib-harvey-core/src/core/",
    "plugins-core/lib-hte/src/",
    "plugins-core/lib-agent-loop/src/",
    "makakoo/src/",
    "makakoo-core/src/",
    "makakoo-mcp/src/",
)
# Back-compat alias (old name) — keep until external callers migrate.
CORE_PATH_PREFIX = CORE_PATH_PREFIXES[0]


class ApprovalError(Exception):
    """Raised when an approval pre-check fails. Callers should show the message."""


@dataclass
class PendingApproval:
    """Everything a reviewer needs to see in one record."""
    id: str
    skill: str
    patch_type: str  # "skill" or "code"
    delta: int
    baseline_score: int
    improved_score: int
    primary_validator: str
    confidence: float
    rationale: str
    sprint_path: str
    improved_blob_path: str  # for skill patches: improved SKILL.md blob
    skill_path: str           # for skill patches: target SKILL.md
    code_patch_path: str = ""  # for code patches: unified diff path
    code_file_path: str = ""  # for code patches: file being patched

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass
class ApprovalResult:
    id: str
    git_sha: str
    git_tag: str
    committed: bool
    skill_path: str  # for skill patches; for code patches, this is the patched file
    patch_type: str
    journal_entry: str

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


# ----------------------------------------------------------------------------
# Listing
# ----------------------------------------------------------------------------


def list_approved() -> List[PendingApproval]:
    """Return every hypothesis waiting for human approval."""
    if not APPROVED_DIR.exists():
        return []
    out: List[PendingApproval] = []
    # Exclude *.skill.md — that's the improved blob, not a sprint doc
    for sprint_md in sorted(APPROVED_DIR.glob("2*.md")):
        if sprint_md.name.endswith(".skill.md"):
            continue
        hyp_id = sprint_md.stem
        prov_path = PROVENANCE_DIR / f"{hyp_id}.json"
        verdict_path = APPROVED_DIR / f"{hyp_id}.verdict.json"
        prov: Dict[str, Any] = {}
        verdict: Dict[str, Any] = {}
        try:
            if prov_path.exists():
                prov = json.loads(prov_path.read_text())
            if verdict_path.exists():
                verdict = json.loads(verdict_path.read_text())
        except (OSError, json.JSONDecodeError):
            pass

        patch_type = prov.get("patch_type", "skill")
        out.append(
            PendingApproval(
                id=hyp_id,
                skill=prov.get("skill", "unknown"),
                patch_type=patch_type,
                delta=int(prov.get("delta", 0) or 0),
                baseline_score=int(prov.get("baseline_score", 0) or 0),
                improved_score=int(prov.get("improved_score", 0) or 0),
                primary_validator=verdict.get("primary_validator", "unknown"),
                confidence=float(verdict.get("confidence", 0.0) or 0.0),
                rationale=verdict.get("rationale", ""),
                sprint_path=str(sprint_md),
                improved_blob_path=str(APPROVED_DIR / f"{hyp_id}.skill.md"),
                skill_path=prov.get("skill_path", ""),
                code_patch_path=prov.get("code_patch_path", ""),
                code_file_path=prov.get("code_file_path", ""),
            )
        )
    return out


# ----------------------------------------------------------------------------
# Stats
# ----------------------------------------------------------------------------


def stats() -> Dict[str, Any]:
    """Quick rollup across all improvement directories."""
    counts = {
        "pending_lope_gate": _count_md(IMPROVEMENTS_DIR / "pending"),
        "approved_awaiting_review": _count_md(APPROVED_DIR),
        "rejected": _count_md(REJECTED_DIR),
        "merged": _count_md(MERGED_DIR),
    }

    deltas = []
    for p in PROVENANCE_DIR.glob("*.json") if PROVENANCE_DIR.exists() else []:
        try:
            prov = json.loads(p.read_text())
            if "delta" in prov:
                deltas.append(int(prov["delta"]))
        except (OSError, json.JSONDecodeError, ValueError):
            continue

    return {
        "counts": counts,
        "hypotheses_generated_total": len(deltas),
        "avg_delta": (sum(deltas) / len(deltas)) if deltas else 0,
    }


def _count_md(directory: Path) -> int:
    if not directory.exists():
        return 0
    return len([p for p in directory.glob("2*.md") if not p.name.endswith(".skill.md")])


# ----------------------------------------------------------------------------
# Approval — the one autonomy stop point
# ----------------------------------------------------------------------------


def approve(
    hypothesis_id: str,
    reviewer_note: str = "",
    *,
    dry_run: bool = False,
    git_runner=subprocess.run,
) -> ApprovalResult:
    """
    Apply either a SKILL.md edit (skill-class) or a unified diff (code-class),
    git-commit, tag, and write a journal entry.

    Raises ApprovalError on any pre-check failure — the caller should surface
    the message and exit non-zero. Never routes around a pre-check.
    """
    if hypothesis_id.strip().upper() == "ALL":
        raise ApprovalError(
            "approve ALL is refused. Approve one hypothesis at a time."
        )

    sprint_path = APPROVED_DIR / f"{hypothesis_id}.md"
    prov_path = PROVENANCE_DIR / f"{hypothesis_id}.json"

    if not sprint_path.exists():
        raise ApprovalError(f"no approved sprint found for id: {hypothesis_id}")
    if not prov_path.exists():
        raise ApprovalError(
            f"approved sprint {hypothesis_id} missing provenance at {prov_path}"
        )

    prov = json.loads(prov_path.read_text())
    patch_type = prov.get("patch_type", "skill")
    sprint_text = sprint_path.read_text()

    if patch_type == "code":
        return _approve_code_patch(hypothesis_id, prov, sprint_text, sprint_path,
                                   reviewer_note, dry_run, git_runner)
    else:
        return _approve_skill_patch(hypothesis_id, prov, sprint_text, sprint_path,
                                    reviewer_note, dry_run, git_runner)


def _approve_skill_patch(
    hypothesis_id: str,
    prov: Dict[str, Any],
    sprint_text: str,
    sprint_path: Path,
    reviewer_note: str,
    dry_run: bool,
    git_runner,
) -> ApprovalResult:
    """Apply an improved SKILL.md from the autoimprover."""
    blob_path = APPROVED_DIR / f"{hypothesis_id}.skill.md"
    if not blob_path.exists():
        raise ApprovalError(
            f"approved sprint {hypothesis_id} missing its improved blob at {blob_path}"
        )

    skill_path = Path(prov.get("skill_path", ""))
    if not skill_path.exists():
        raise ApprovalError(f"target SKILL.md not found at {skill_path}")

    try:
        rel = skill_path.relative_to(HARVEY_HOME)
    except ValueError:
        rel = Path(str(skill_path))
    rel_str = str(rel).replace(os.sep, "/")

    # HARD RULE: reject core changes
    if any(rel_str.startswith(p) for p in CORE_PATH_PREFIXES):
        raise ApprovalError(
            f"refusing to merge change to core path: {rel_str}. "
            "Core changes require a hand-written sprint, not an autonomous hypothesis."
        )

    # HARD RULE: sprint must only name files outside the core plugin/kernel tree.
    claimed_files = _extract_claimed_files(sprint_text)
    for claimed in claimed_files:
        if any(claimed.startswith(p) for p in CORE_PATH_PREFIXES):
            raise ApprovalError(
                f"sprint claims it will modify core file {claimed}. Refused."
            )

    new_content = blob_path.read_text()
    if dry_run:
        return ApprovalResult(
            id=hypothesis_id,
            git_sha="(dry-run)",
            git_tag="(dry-run)",
            committed=False,
            skill_path=str(skill_path),
            patch_type="skill",
            journal_entry=f"DRY RUN: would apply {hypothesis_id} to {rel_str}",
        )

    skill_path.write_text(new_content, encoding="utf-8")

    delta = int(prov.get("delta", 0))
    skill = prov.get("skill", "unknown")
    message_lines = _build_commit_message(hypothesis_id, prov, skill, delta, reviewer_note)
    commit_message = "\n".join(message_lines)
    tag = f"improve/{hypothesis_id}"

    cwd = str(_git_root_for(skill_path))
    git_runner(["git", "-C", cwd, "add", str(skill_path)], check=True, capture_output=True, text=True)
    git_runner(["git", "-C", cwd, "commit", "-m", commit_message], check=True, capture_output=True, text=True)
    result = git_runner(
        ["git", "-C", cwd, "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    )
    sha = (result.stdout or "").strip()
    git_runner(["git", "-C", cwd, "tag", tag, sha], check=True, capture_output=True, text=True)

    MERGED_DIR.mkdir(parents=True, exist_ok=True)
    _move_pair_to(MERGED_DIR, hypothesis_id)

    journal_entry = _write_journal_entry(hypothesis_id, skill, delta, sha, reviewer_note)
    return ApprovalResult(
        id=hypothesis_id,
        git_sha=sha,
        git_tag=tag,
        committed=True,
        skill_path=str(skill_path),
        patch_type="skill",
        journal_entry=journal_entry,
    )


def _approve_code_patch(
    hypothesis_id: str,
    prov: Dict[str, Any],
    sprint_text: str,
    sprint_path: Path,
    reviewer_note: str,
    dry_run: bool,
    git_runner,
) -> ApprovalResult:
    """Apply a unified diff to fix a code-level error."""
    patch_path = Path(prov.get("code_patch_path", ""))
    code_file_path = Path(prov.get("code_file_path", ""))

    if not patch_path.exists():
        raise ApprovalError(f"code patch not found at {patch_path}")
    if not code_file_path.exists():
        raise ApprovalError(f"target file not found at {code_file_path}")

    try:
        rel = code_file_path.relative_to(HARVEY_HOME)
    except ValueError:
        rel = Path(str(code_file_path))
    rel_str = str(rel).replace(os.sep, "/")

    # HARD RULE: reject core changes
    if any(rel_str.startswith(p) for p in CORE_PATH_PREFIXES):
        raise ApprovalError(
            f"refusing to merge change to core path: {rel_str}. "
            "Core changes require a hand-written sprint, not an autonomous hypothesis."
        )

    # HARD RULE: sprint must only name files outside the core plugin/kernel tree.
    claimed_files = _extract_claimed_files(sprint_text)
    for claimed in claimed_files:
        if any(claimed.startswith(p) for p in CORE_PATH_PREFIXES):
            raise ApprovalError(
                f"sprint claims it will modify core file {claimed}. Refused."
            )

    if dry_run:
        return ApprovalResult(
            id=hypothesis_id,
            git_sha="(dry-run)",
            git_tag="(dry-run)",
            committed=False,
            skill_path=str(code_file_path),
            patch_type="code",
            journal_entry=f"DRY RUN: would apply {hypothesis_id} diff to {rel_str}",
        )

    # Apply the unified diff
    result = git_runner(
        ["patch", "-p1", "-i", str(patch_path)],
        check=False, capture_output=True, text=True, cwd=HARVEY_HOME,
    )
    if result.returncode != 0:
        raise ApprovalError(
            f"patch application failed (exit {result.returncode}): "
            f"{result.stderr.strip()[:200]}"
        )

    # Verify the patched file parses
    compile_result = git_runner(
        ["python3", "-m", "py_compile", str(code_file_path)],
        check=False, capture_output=True, text=True,
    )
    if compile_result.returncode != 0:
        # Revert the patch by restoring from git
        git_runner(
            ["git", "-C", HARVEY_HOME, "checkout", "--", str(rel)],
            check=True, capture_output=True, text=True,
        )
        raise ApprovalError(
            f"patched file fails py_compile: {compile_result.stderr.strip()[:200]}. "
            "Patch has been reverted."
        )

    # Git commit
    exception = prov.get("skill", "unknown")  # skill field holds the exception type for code patches
    message_lines = [
        f"fix: {hypothesis_id} — {exception} in {rel_str}",
        "",
        "Autonomously generated by Harvey's Mascot GYM CODE-class hypothesis. "
        "Manually approved.",
    ]
    if reviewer_note:
        message_lines.extend(["", f"Reviewer note: {reviewer_note}"])
    message_lines.extend([
        "",
        f"Source cluster: {prov.get('cluster_id', 'unknown')}",
        f"Patch type: unified diff (gym v0.2)",
        "",
        "Co-Authored-By: Harvey Mascot GYM <gym@harvey-os.local>",
    ])
    commit_message = "\n".join(message_lines)
    tag = f"fix/{hypothesis_id}"

    cwd = HARVEY_HOME
    git_runner(["git", "-C", cwd, "add", str(code_file_path)], check=True, capture_output=True, text=True)
    git_runner(["git", "-C", cwd, "commit", "-m", commit_message], check=True, capture_output=True, text=True)
    result2 = git_runner(
        ["git", "-C", cwd, "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    )
    sha = (result2.stdout or "").strip()
    git_runner(["git", "-C", cwd, "tag", tag, sha], check=True, capture_output=True, text=True)

    MERGED_DIR.mkdir(parents=True, exist_ok=True)
    _move_pair_to(MERGED_DIR, hypothesis_id)

    delta = int(prov.get("delta", 0))
    journal_entry = _write_journal_entry(
        hypothesis_id,
        f"code: {exception} in {rel_str}",
        delta,
        sha,
        reviewer_note,
    )
    return ApprovalResult(
        id=hypothesis_id,
        git_sha=sha,
        git_tag=tag,
        committed=True,
        skill_path=str(code_file_path),
        patch_type="code",
        journal_entry=journal_entry,
    )


def _build_commit_message(
    hypothesis_id: str,
    prov: Dict[str, Any],
    skill: str,
    delta: int,
    reviewer_note: str,
) -> List[str]:
    lines = [
        f"improve: {hypothesis_id} — {skill} (delta +{delta})",
        "",
        "Autonomously generated by Harvey's Mascot GYM and manually approved.",
    ]
    if reviewer_note:
        lines.extend(["", f"Reviewer note: {reviewer_note}"])
    lines.extend([
        "",
        f"Source cluster: {prov.get('cluster_id', 'unknown')}",
        f"Baseline score: {prov.get('baseline_score')} → improved {prov.get('improved_score')}",
        f"Drafter model: {prov.get('drafter_model', 'unknown')}",
        "",
        "Co-Authored-By: Harvey Mascot GYM <gym@harvey-os.local>",
    ])
    return lines


def reject(hypothesis_id: str, reviewer_note: str = "") -> None:
    """Move an approved hypothesis to rejected/ after human review."""
    sprint_path = APPROVED_DIR / f"{hypothesis_id}.md"
    if not sprint_path.exists():
        raise ApprovalError(f"no approved sprint found for id: {hypothesis_id}")
    REJECTED_DIR.mkdir(parents=True, exist_ok=True)
    _move_pair_to(REJECTED_DIR, hypothesis_id, from_dir=APPROVED_DIR)
    critique = REJECTED_DIR / f"{hypothesis_id}.human-rejection.md"
    critique.write_text(
        f"# Human rejection for {hypothesis_id}\n\n"
        f"Rejected at: {datetime.now(timezone.utc).isoformat()}\n\n"
        f"Reason: {reviewer_note or '(no reason given)'}\n",
        encoding="utf-8",
    )


# ----------------------------------------------------------------------------
# Helpers
# ----------------------------------------------------------------------------


_FILES_SECTION_RE = re.compile(r"\*\*Files:\*\*\s*\n((?:- .*\n?)+)", re.MULTILINE)


def _extract_claimed_files(sprint_text: str) -> List[str]:
    m = _FILES_SECTION_RE.search(sprint_text)
    if not m:
        return []
    out = []
    for line in m.group(1).splitlines():
        line = line.strip()
        if line.startswith("- "):
            path = line[2:].strip("` ")
            if path:
                out.append(path)
    return out


def _move_pair_to(target: Path, hyp_id: str, from_dir: Path = APPROVED_DIR) -> None:
    import shutil
    target.mkdir(parents=True, exist_ok=True)
    for suffix in (".md", ".skill.md", ".patch", ".verdict.json", ".human-rejection.md"):
        src = from_dir / f"{hyp_id}{suffix}"
        if src.exists():
            shutil.move(str(src), str(target / src.name))


def _git_root_for(path: Path) -> Path:
    """
    Find the nearest git repo containing `path`. Walks up parents until
    a .git entry shows up. Falls back to HARVEY_HOME so we never call
    git -C on a non-repo directory.
    """
    cur = path.resolve()
    for parent in [cur] + list(cur.parents):
        if (parent / ".git").exists():
            return parent
    return Path(HARVEY_HOME)


def _write_journal_entry(
    hyp_id: str,
    skill: str,
    delta: int,
    git_sha: str,
    reviewer_note: str,
) -> str:
    today = datetime.now().strftime("%Y_%m_%d")
    journal = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"
    journal.parent.mkdir(parents=True, exist_ok=True)
    entry_lines = [
        f"- [[Harvey Mascot GYM]] approved improvement [[{hyp_id}]] for [[{skill}]]",
        f"  - Delta: +{delta} (autoimprover-generated, lope-validated, human-approved)",
        f"  - Git: {git_sha[:10]} tag `improve/{hyp_id}`",
    ]
    if reviewer_note:
        entry_lines.append(f"  - Reviewer note: {reviewer_note}")
    entry_text = "\n".join(entry_lines) + "\n"
    existing = journal.read_text() if journal.exists() else ""
    with open(journal, "a", encoding="utf-8") as f:
        if existing and not existing.endswith("\n"):
            f.write("\n")
        f.write(entry_text)
    return entry_text
