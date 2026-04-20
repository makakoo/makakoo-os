"""
Layer 3 of Harvey's Mascot GYM — hypothesis generator.

Takes the day's skill-class clusters from Layer 2, runs autoimprover to
propose a SKILL.md edit for each one, scores baseline vs improved via
meta-harness-agent, and writes any delta > 0 hypotheses as draft lope
sprints with a full provenance record.

This runs once per night (02:00–04:00 window). It is slow by design —
each scored hypothesis can take minutes. Never put this on a hot path.

Injection points for testability:
    improve_fn(skill_content, gap_id, gap_desc, skill_name) -> Optional[str]
    score_fn(skill_content, gap_id, skill_loaded, skill_path) -> int

Defaults pull from skills/meta/autoimprover/evaluate_skill.py at
first call time so the heavy imports stay lazy.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import sys
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
ERRORS_DIR = Path(HARVEY_HOME) / "data" / "errors"
IMPROVEMENTS_DIR = Path(HARVEY_HOME) / "data" / "improvements"
SKILLS_ROOT = Path(HARVEY_HOME) / "harvey-os" / "skills"

DEFAULT_TOP_N = 5
DEFAULT_MIN_SAMPLE_SIZE = 2  # require at least this many entries per cluster
DEFAULT_MAX_SAMPLES_IN_GAP = 5


@dataclass
class HypothesisRecord:
    id: str
    cluster_id: str
    source_error_ids: List[str]
    skill: str
    drafter_model: str
    baseline_score: int
    improved_score: int
    delta: int
    gap_desc: str
    sprint_path: str
    generated_at: str
    gym_version: str = "0.1"

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


# ----------------------------------------------------------------------------
# Default injection — resolved lazily so imports stay cheap
# ----------------------------------------------------------------------------


def _default_improve_fn() -> Callable:
    sys.path.insert(0, os.path.join(HARVEY_HOME, "harvey-os"))
    from skills.meta.autoimprover.evaluate_skill import improve_gap
    return improve_gap


def _default_score_fn() -> Callable:
    sys.path.insert(0, os.path.join(HARVEY_HOME, "harvey-os"))
    from skills.meta.autoimprover.evaluate_skill import evaluate_with_llm
    return evaluate_with_llm


def _default_drafter_model() -> str:
    return os.environ.get("LLM_MODEL", "minimax:MiniMax-M2.7")


# ----------------------------------------------------------------------------
# Helpers
# ----------------------------------------------------------------------------


def _load_clusters(date: str) -> Optional[Dict[str, Any]]:
    path = ERRORS_DIR / date / "clustered.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def _resolve_skill_path(skill: str) -> Optional[Path]:
    """
    skill strings look like "meta/caveman-voice" — resolve to SKILL.md on disk.

    We accept both "<category>/<name>" and "<name>" (with a scan fallback)
    because different capture sources may know different amounts about the
    skill at the time of failure.
    """
    if "/" in skill:
        cand = SKILLS_ROOT / skill / "SKILL.md"
        if cand.exists():
            return cand
    # Fallback: scan categories
    if SKILLS_ROOT.exists():
        for cat_dir in SKILLS_ROOT.iterdir():
            if not cat_dir.is_dir():
                continue
            cand = cat_dir / skill / "SKILL.md"
            if cand.exists():
                return cand
    return None


def _build_gap_desc(cluster: Dict[str, Any], max_samples: int = DEFAULT_MAX_SAMPLES_IN_GAP) -> str:
    """
    Turn a cluster record into a short natural-language gap description
    that autoimprover.improve_gap can reason over.
    """
    count = cluster.get("count", 0)
    cmd = cluster.get("sample_cmd", "")[:200]
    stderr = cluster.get("sample_stderr", "")[:400]
    first = cluster.get("first_seen", "")
    last = cluster.get("last_seen", "")
    skills = cluster.get("skills_in_scope", [])
    return (
        f"Recurring skill failure observed {count}x between {first} and {last}.\n"
        f"Skill(s) in scope: {', '.join(skills) if skills else '(unknown)'}\n"
        f"Sample command: {cmd}\n"
        f"Sample error: {stderr}\n"
        "The current SKILL.md guidance did not prevent this failure. "
        "Update the skill doc so that future executions of this pattern "
        "either avoid the failure mode or handle it cleanly."
    )


def _hypothesis_id(cluster_id: str, ts: Optional[datetime] = None) -> str:
    ts = ts or datetime.now(timezone.utc)
    stamp = ts.strftime("%Y%m%dT%H%M%S")
    slug = cluster_id[:10]
    return f"{stamp}-{slug}"


def _slug(skill: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", skill.lower()).strip("-")


def _draft_sprint_md(hyp: HypothesisRecord, cluster: Dict[str, Any]) -> str:
    """One-phase lope sprint doc — the format Layer 4 will feed to lope negotiate."""
    return f"""# SPRINT-GYM-{hyp.id}

## Origin
Harvey's Mascot GYM — autogenerated hypothesis from cluster `{hyp.cluster_id}`
on skill `{hyp.skill}`. Baseline score {hyp.baseline_score}/100, improved
score {hyp.improved_score}/100 (delta +{hyp.delta}).

Source errors: {len(hyp.source_error_ids)} entries, first seen
{cluster.get('first_seen')}, last seen {cluster.get('last_seen')}.

## Phase 1: Apply autoimprover-proposed SKILL.md edit

**Goal:** Merge the autoimprover-edited version of `skills/{hyp.skill}/SKILL.md`.
The edit targets the recurring failure captured by cluster `{hyp.cluster_id}`.

**Gap description:**
{hyp.gap_desc}

**Criteria:**
- Replace the current `SKILL.md` content with the improved version stored at
  `data/improvements/pending/{hyp.id}.skill.md`.
- Do not delete or shorten any sections unrelated to this gap.
- Run the skill doc lint (existing tooling) after applying.

**Files:**
- `harvey-os/skills/{hyp.skill}/SKILL.md`

**Tests:**
- Meta-harness scenario scored {hyp.baseline_score} → {hyp.improved_score} (delta +{hyp.delta})
- Cluster `{hyp.cluster_id}` should stop appearing in tomorrow's clustered.json
"""


# ----------------------------------------------------------------------------
# Main entry point
# ----------------------------------------------------------------------------


def generate_hypotheses(
    date: Optional[str] = None,
    top_n: int = DEFAULT_TOP_N,
    improve_fn: Optional[Callable] = None,
    score_fn: Optional[Callable] = None,
    drafter_model: Optional[str] = None,
) -> List[HypothesisRecord]:
    """
    Read clustered.json for `date`, generate hypotheses for up to `top_n`
    skill-class clusters, and write passing ones (delta > 0) as pending
    lope sprint drafts under `data/improvements/pending/`.

    Returns the list of HypothesisRecord objects actually written (i.e.
    those where delta > 0). Failures on a single cluster never abort the
    whole run — we log and continue so one bad hypothesis cannot block
    every other.
    """
    date = date or datetime.now(timezone.utc).strftime("%Y-%m-%d")
    clusters_doc = _load_clusters(date)
    if not clusters_doc or not clusters_doc.get("clusters"):
        return []

    # Filter to skill-class clusters with enough samples to be actionable
    skill_clusters = [
        c for c in clusters_doc["clusters"]
        if c.get("error_class") == "skill"
        and c.get("count", 0) >= DEFAULT_MIN_SAMPLE_SIZE
        and c.get("skills_in_scope")
    ][:top_n]

    if not skill_clusters:
        return []

    _improve = improve_fn or _default_improve_fn()
    _score = score_fn or _default_score_fn()
    _model = drafter_model or _default_drafter_model()

    IMPROVEMENTS_DIR.mkdir(parents=True, exist_ok=True)
    (IMPROVEMENTS_DIR / "pending").mkdir(exist_ok=True)
    (IMPROVEMENTS_DIR / "provenance").mkdir(exist_ok=True)

    written: List[HypothesisRecord] = []

    for cluster in skill_clusters:
        try:
            hyp = _process_cluster(cluster, _improve, _score, _model)
            if hyp is not None:
                written.append(hyp)
        except Exception as exc:
            # Never let one bad cluster kill the whole run
            sys.stderr.write(
                f"gym.hypothesis: cluster {cluster.get('cluster_id')} failed: {exc!r}\n"
            )
            continue

    return written


def _process_cluster(
    cluster: Dict[str, Any],
    improve_fn: Callable,
    score_fn: Callable,
    drafter_model: str,
) -> Optional[HypothesisRecord]:
    skill = cluster["skills_in_scope"][0]
    skill_path = _resolve_skill_path(skill)
    if skill_path is None:
        return None

    current = skill_path.read_text(encoding="utf-8")
    gap_id = f"gym-cluster-{cluster['cluster_id']}"
    gap_desc = _build_gap_desc(cluster)

    improved = improve_fn(current, gap_id, gap_desc, skill)
    if not improved or len(improved.strip()) < 50:
        return None

    baseline = score_fn(current, gap_id, False, skill_path)
    if baseline is None:
        return None
    improved_score = score_fn(improved, gap_id, True, skill_path)
    if improved_score is None:
        return None

    delta = improved_score - baseline
    if delta <= 0:
        return None

    hyp_id = _hypothesis_id(cluster["cluster_id"])
    hyp = HypothesisRecord(
        id=hyp_id,
        cluster_id=cluster["cluster_id"],
        source_error_ids=cluster.get("error_ids", []),
        skill=skill,
        drafter_model=drafter_model,
        baseline_score=baseline,
        improved_score=improved_score,
        delta=delta,
        gap_desc=gap_desc,
        sprint_path=str(IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.md"),
        generated_at=datetime.now(timezone.utc).isoformat(),
    )

    # Write pending sprint + the improved SKILL.md blob + provenance record
    pending_sprint = IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.md"
    pending_sprint.write_text(_draft_sprint_md(hyp, cluster), encoding="utf-8")

    improved_blob = IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.skill.md"
    improved_blob.write_text(improved, encoding="utf-8")

    provenance_path = IMPROVEMENTS_DIR / "provenance" / f"{hyp_id}.json"
    provenance = hyp.to_dict()
    provenance["skill_path"] = str(skill_path)
    provenance["improved_blob_path"] = str(improved_blob)
    provenance_path.write_text(
        json.dumps(provenance, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )

    return hyp
