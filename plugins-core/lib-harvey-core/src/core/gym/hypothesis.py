"""
Layer 3 of Harvey's Mascot GYM — hypothesis generator.

Two-class hypothesis generation:

  SKILL — skill-class clusters → SKILL.md edits via autoimprover.
           Scored by meta-harness-agent. Passes if delta > 0.

  CODE  — code-class clusters → Python patches via LLM diff generation.
           Extracted from CODE-classified error clusters by code_targets
           in clustered.json. The LLM receives the full file content and
           the error context, and returns a minimal unified diff targeting
           only the root cause.

This runs once per night (02:00–04:00 window). It is slow by design —
each scored hypothesis can take minutes. Never put this on a hot path.

Injection points for testability:
    skill_improve_fn  — improve SKILL.md content
    skill_score_fn    — score a SKILL.md via meta-harness
    code_patch_fn     — generate a Python patch for a code error
    drafter_model     — LLM model string
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Literal, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
ERRORS_DIR = Path(HARVEY_HOME) / "data" / "errors"
IMPROVEMENTS_DIR = Path(HARVEY_HOME) / "data" / "improvements"
SKILLS_ROOT = Path(HARVEY_HOME) / "plugins-core"

DEFAULT_TOP_N = 5
DEFAULT_MIN_SAMPLE_SIZE = 2
DEFAULT_MAX_SAMPLES_IN_GAP = 5

LLM_BASE_URL = os.environ.get("AIL_BASE_URL", "http://localhost:18080/v1")
LLM_API_KEY = os.environ.get("AIL_API_KEY", "")
LLM_TIMEOUT = int(os.environ.get("SANCHO_LLM_TIMEOUT", "120"))


# ─────────────────────────────────────────────────────────────────────────────
# Hypothesis records
# ─────────────────────────────────────────────────────────────────────────────

@dataclass
class HypothesisRecord:
    id: str
    cluster_id: str
    source_error_ids: List[str]
    skill: str  # skill name for skill patches; file_path for code patches
    patch_type: Literal["skill", "code"] = "skill"
    drafter_model: str = "minimax:MiniMax-M2.7"
    baseline_score: int = 0
    improved_score: int = 0
    delta: int = 0
    gap_desc: str = ""
    sprint_path: str = ""
    skill_path: str = ""          # for skill patches: path to SKILL.md
    improved_blob_path: str = ""  # for skill patches: path to improved SKILL.md blob
    code_patch_path: str = ""     # for code patches: path to unified diff
    code_file_path: str = ""      # for code patches: absolute path to the file to patch
    generated_at: str = ""
    gym_version: str = "0.2"

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


# ─────────────────────────────────────────────────────────────────────────────
# Default injection — resolved lazily so imports stay cheap
# ─────────────────────────────────────────────────────────────────────────────


def _default_skill_improve_fn() -> Callable:
    # autoimprover was moved out of skills/ into skills-shared/harvey/meta/
    # during the 2026-04-20 harvey-os retirement. Inject the new path
    # before resolving so stale `from skills...` imports keep failing
    # with a loud ImportError instead of silently loading the wrong copy.
    _ensure_autoimprover_on_path()
    from meta.autoimprover.evaluate_skill import improve_gap  # type: ignore[import-not-found]
    return improve_gap


def _default_skill_score_fn() -> Callable:
    _ensure_autoimprover_on_path()
    from meta.autoimprover.evaluate_skill import evaluate_with_llm  # type: ignore[import-not-found]
    return evaluate_with_llm


def _ensure_autoimprover_on_path() -> None:
    import sys
    candidate = os.path.join(
        os.environ.get("MAKAKOO_HOME", os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))),
        "skills-shared",
        "harvey",
    )
    if candidate not in sys.path:
        sys.path.insert(0, candidate)


def _default_drafter_model() -> str:
    return os.environ.get("LLM_MODEL", "minimax:MiniMax-M2.7")


# ─────────────────────────────────────────────────────────────────────────────
# Helpers — skill patches
# ─────────────────────────────────────────────────────────────────────────────


def _load_clusters_doc(date: str) -> Optional[Dict[str, Any]]:
    path = ERRORS_DIR / date / "clustered.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def _resolve_skill_path(skill: str) -> Optional[Path]:
    if "/" in skill:
        cand = SKILLS_ROOT / skill / "SKILL.md"
        if cand.exists():
            return cand
    if SKILLS_ROOT.exists():
        for cat_dir in SKILLS_ROOT.iterdir():
            if not cat_dir.is_dir():
                continue
            cand = cat_dir / skill / "SKILL.md"
            if cand.exists():
                return cand
    return None


def _build_gap_desc(cluster: Dict[str, Any], max_samples: int = DEFAULT_MAX_SAMPLES_IN_GAP) -> str:
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


def _draft_skill_sprint_md(hyp: HypothesisRecord, cluster: Dict[str, Any]) -> str:
    return f"""# SPRINT-GYM-{hyp.id}

## Origin
Harvey's Mascot GYM — autogenerated SKILL hypothesis from cluster
`{hyp.cluster_id}` on skill `{hyp.skill}`. Baseline score
{hyp.baseline_score}/100, improved score {hyp.improved_score}/100 (delta +{hyp.delta}).

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
- Run the skill doc lint after applying.

**Files:**
- `plugins-core/{hyp.skill}/SKILL.md`

**Tests:**
- Meta-harness scenario scored {hyp.baseline_score} → {hyp.improved_score} (delta +{hyp.delta})
- Cluster `{hyp.cluster_id}` should stop appearing in tomorrow's clustered.json
"""


# ─────────────────────────────────────────────────────────────────────────────
# Helpers — code patches
# ─────────────────────────────────────────────────────────────────────────────

_CODE_FILE_RE = re.compile(
    r"File \"([^\"]*(?:plugins-core|harvey-os|agents|makakoo-os|skills|core)[^\"]*\.py)\""
)


def _resolve_code_file_path(file_path: str) -> Optional[Path]:
    """
    Resolve a file path from a traceback into an absolute Path.
    Accepts both absolute paths and paths relative to HARVEY_HOME.
    """
    p = Path(file_path)
    if p.is_absolute():
        return p if p.exists() else None
    # Resolve relative to HARVEY_HOME
    resolved = Path(HARVEY_HOME) / file_path
    return resolved if resolved.exists() else None


def _read_file_safe(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""


def _llm_patch(prompt: str, model: str) -> str:
    """
    Call the LLM (switchAILocal) with a prompt and return the text response.
    Used for code patch generation. Raises on failure so callers can catch
    and skip gracefully.
    """
    if not LLM_API_KEY:
        raise RuntimeError(
            "AIL_API_KEY env var not set — cannot generate CODE patches. "
            "Run `makakoo secret set AIL_API_KEY` to configure."
        )
    try:
        from openai import OpenAI
    except ImportError:
        raise RuntimeError("openai package required for code patch generation")

    client = OpenAI(api_key=LLM_API_KEY, base_url=LLM_BASE_URL)
    response = client.chat.completions.create(
        model=model,
        messages=[
            {
                "role": "system",
                "content": (
                    "You are Harvey's code patch generator. You respond ONLY with "
                    "a unified diff targeting the specific error described. "
                    "No explanation, no markdown fences, no preamble. "
                    "Just the raw unified diff. If the error cannot be fixed with "
                    "a simple patch, say CANNOT_PATCH on its own line."
                ),
            },
            {"role": "user", "content": prompt},
        ],
        timeout=LLM_TIMEOUT,
    )
    content = response.choices[0].message.content
    return content or ""


def _generate_code_patch(
    target: Dict[str, Any],
    model: str,
) -> Optional[str]:
    """
    Generate a unified diff for one code target.

    target keys:
        file_path     — absolute path to the Python file to patch
        exception     — exception type string
        stderr_samples — list of stderr snippets
        count         — how many times this error occurred
        line_no       — optional line number hint

    Returns the unified diff text, or None if the LLM refused/couldn't patch.
    """
    file_path = target.get("file_path", "")
    resolved = _resolve_code_file_path(file_path)
    if resolved is None:
        sys.stderr.write(
            f"gym.hypothesis: code target file not found: {file_path}\n"
        )
        return None

    current_content = _read_file_safe(resolved)
    if not current_content:
        sys.stderr.write(
            f"gym.hypothesis: could not read code target file: {file_path}\n"
        )
        return None

    exception = target.get("exception", "Error")
    stderr_snippet = "\n---\n".join(target.get("stderr_samples", [])[:2])
    count = target.get("count", 1)
    line_hint = target.get("line_no") or "unknown"

    prompt = f"""File to patch: {resolved}
Exception observed: {exception} (occurred {count}x)

Traceback:
{stderr_snippet}

Current file content (READ ONLY — do not reproduce in your diff):
```
{current_content[:8000]}
```

Instructions:
1. Identify the root cause of the {exception} in this file.
2. Write a MINIMAL unified diff that fixes ONLY the root cause.
3. Do NOT refactor, reformat, or improve unrelated code.
4. Return ONLY the unified diff. Start with `--- a/...` and end with the last `@@`.
5. If the fix requires more than 20 lines of changed context, say CANNOT_PATCH.
6. If the exception is not fixable in this file (e.g. upstream API change, missing env var), say CANNOT_PATCH.
"""

    try:
        diff_text = _llm_patch(prompt, model)
    except Exception as exc:
        sys.stderr.write(f"gym.hypothesis: LLM patch call failed: {exc!r}\n")
        return None

    if "CANNOT_PATCH" in diff_text or len(diff_text) < 20:
        return None

    # Validate it looks like a real diff
    if "--- a/" not in diff_text and "--- " not in diff_text:
        return None

    return diff_text


def _draft_code_sprint_md(hyp: HypothesisRecord, target: Dict[str, Any]) -> str:
    """Sprint doc for a code patch hypothesis."""
    exception = target.get("exception", "Error")
    count = target.get("count", 1)
    return f"""# SPRINT-GYM-{hyp.id}

## Origin
Harvey's Mascot GYM — autogenerated CODE hypothesis for file
`{hyp.code_file_path}`. Exception `{exception}` observed {count}x.

Source errors: {len(hyp.source_error_ids)} entries.

## Phase 1: Apply LLM-generated unified diff

**Goal:** Apply the patch at `data/improvements/pending/{hyp.id}.patch`
to fix the recurring `{exception}` in `{hyp.code_file_path}`.

**Patch file:**
`data/improvements/pending/{hyp.id}.patch`

**Criteria:**
- Apply the diff with `patch -p1 < data/improvements/pending/{hyp.id}.patch`.
- Verify the file still parses: `python3 -m py_compile {hyp.code_file_path}`.
- If the patch fails or the file no longer parses, this hypothesis is REJECTED.
- Run the agent that produced the original error to confirm the fix.

**Files:**
- `{hyp.code_file_path}` (relative to HARVEY_HOME)

**Tests:**
- Cluster `{hyp.cluster_id}` should stop appearing in tomorrow's clustered.json
- The same `{exception}` should not recur within 48h

**Risk level:** CODE patches modify runtime behavior. Human review is strongly
recommended before running `patch`. Set `GYM_CODE_PATCH_AUTO_MERGE=false` in
your environment to require explicit approval for code-class sprints.
"""


# ─────────────────────────────────────────────────────────────────────────────
# Main entry point
# ─────────────────────────────────────────────────────────────────────────────


def generate_hypotheses(
    date: Optional[str] = None,
    top_n: int = DEFAULT_TOP_N,
    skill_improve_fn: Optional[Callable] = None,
    skill_score_fn: Optional[Callable] = None,
    code_patch_fn: Optional[Callable] = None,
    drafter_model: Optional[str] = None,
) -> List[HypothesisRecord]:
    """
    Read clustered.json for `date`, generate hypotheses for both SKILL-class
    and CODE-class clusters, and write passing ones as pending lope sprint
    drafts under `data/improvements/pending/`.

    Returns all HypothesisRecord objects written. Failures on a single
    cluster never abort the whole run.
    """
    date = date or datetime.now(timezone.utc).strftime("%Y-%m-%d")
    clusters_doc = _load_clusters_doc(date)
    if not clusters_doc:
        return []

    # ── SKILL-class hypotheses ───────────────────────────────────────────────
    skill_clusters = [
        c for c in clusters_doc.get("clusters", [])
        if c.get("error_class") == "skill"
        and c.get("count", 0) >= DEFAULT_MIN_SAMPLE_SIZE
        and c.get("skills_in_scope")
    ][:top_n]

    # ── CODE-class hypotheses ────────────────────────────────────────────────
    code_targets = clusters_doc.get("code_targets", [])[:top_n]

    if not skill_clusters and not code_targets:
        return []

    _improve = skill_improve_fn or _default_skill_improve_fn()
    _score = skill_score_fn or _default_skill_score_fn()
    _patch = code_patch_fn or _generate_code_patch
    _model = drafter_model or _default_drafter_model()

    IMPROVEMENTS_DIR.mkdir(parents=True, exist_ok=True)
    pending_dir = IMPROVEMENTS_DIR / "pending"
    pending_dir.mkdir(exist_ok=True)
    (IMPROVEMENTS_DIR / "provenance").mkdir(exist_ok=True)

    written: List[HypothesisRecord] = []

    for cluster in skill_clusters:
        try:
            hyp = _process_skill_cluster(cluster, _improve, _score, _model)
            if hyp is not None:
                written.append(hyp)
        except Exception as exc:
            sys.stderr.write(
                f"gym.hypothesis: skill cluster {cluster.get('cluster_id')} failed: {exc!r}\n"
            )
            continue

    for target in code_targets:
        try:
            hyp = _process_code_target(target, _patch, _model)
            if hyp is not None:
                written.append(hyp)
        except Exception as exc:
            sys.stderr.write(
                f"gym.hypothesis: code target {target.get('file_path')} failed: {exc!r}\n"
            )
            continue

    return written


def _process_skill_cluster(
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
        patch_type="skill",
        drafter_model=drafter_model,
        baseline_score=baseline,
        improved_score=improved_score,
        delta=delta,
        gap_desc=gap_desc,
        sprint_path=str(IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.md"),
        skill_path=str(skill_path),
        improved_blob_path=str(IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.skill.md"),
        generated_at=datetime.now(timezone.utc).isoformat(),
    )

    pending_sprint = IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.md"
    pending_sprint.write_text(_draft_skill_sprint_md(hyp, cluster), encoding="utf-8")

    improved_blob = IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.skill.md"
    improved_blob.write_text(improved, encoding="utf-8")

    provenance = hyp.to_dict()
    provenance["gap_id"] = gap_id
    (IMPROVEMENTS_DIR / "provenance" / f"{hyp_id}.json").write_text(
        json.dumps(provenance, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )

    return hyp


def _process_code_target(
    target: Dict[str, Any],
    patch_fn: Callable,
    drafter_model: str,
) -> Optional[HypothesisRecord]:
    """
    Generate and write a code patch hypothesis for one CODE-class error cluster.

    Unlike skill patches, code patches are not scored (no baseline vs improved
    scoring since there's no meta-harness scenario for arbitrary Python errors).
    Instead, we verify the patch applies cleanly before writing the hypothesis.
    If `patch` fails, we skip this target silently.
    """
    file_path = target.get("file_path", "")
    resolved = _resolve_code_file_path(file_path)
    if resolved is None:
        return None

    cluster_id = hashlib.sha1(file_path.encode()).hexdigest()[:12]
    hyp_id = _hypothesis_id(cluster_id)

    patch_text = patch_fn(target, drafter_model)
    if not patch_text:
        return None

    # Verify the patch applies cleanly before committing
    patch_file = IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.patch"
    patch_file.write_text(patch_text, encoding="utf-8")

    # Dry-run: apply with --dry-run to verify validity
    import subprocess
    result = subprocess.run(
        ["patch", "-p1", "--dry-run", "-i", str(patch_file)],
        capture_output=True, text=True, cwd=HARVEY_HOME,
    )
    if result.returncode != 0:
        sys.stderr.write(
            f"gym.hypothesis: code patch dry-run failed for {file_path}: "
            f"{result.stderr.strip()[:200]}\n"
        )
        patch_file.unlink(missing_ok=True)
        return None

    # Write sprint doc
    hyp = HypothesisRecord(
        id=hyp_id,
        cluster_id=cluster_id,
        source_error_ids=target.get("error_ids", []),
        skill=file_path,
        patch_type="code",
        drafter_model=drafter_model,
        gap_desc=(
            f"Recurring {target.get('exception', 'Error')} in {file_path} "
            f"(observed {target.get('count', 1)}x). "
            f"LLM-generated unified diff applied to fix the root cause."
        ),
        sprint_path=str(IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.md"),
        code_patch_path=str(patch_file),
        code_file_path=str(resolved),
        generated_at=datetime.now(timezone.utc).isoformat(),
    )

    sprint_file = IMPROVEMENTS_DIR / "pending" / f"{hyp_id}.md"
    sprint_file.write_text(_draft_code_sprint_md(hyp, target), encoding="utf-8")

    provenance = hyp.to_dict()
    provenance["file_path"] = file_path
    (IMPROVEMENTS_DIR / "provenance" / f"{hyp_id}.json").write_text(
        json.dumps(provenance, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )

    return hyp
