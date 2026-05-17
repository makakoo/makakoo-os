"""
GYM autoreseach-style fixed eval harness.

Maps autoreseach's fixed-time + single-metric model to GYM task evaluation.

autoreseach model (train.py):
  - 5-minute fixed time budget
  - val_bpb metric (bits per byte, lower is better)
  - Fixed eval on pinned shard_06542 (never changes)
  - Result = single float

GYM model:
  - No external deps (autoreseach constraint: no new packages)
  - Score = completeness * 0.4 + quality_indicators * 0.4 + simplicity * 0.2
  - Benchmark = static skill quality rubric (keyword presence + structure)
  - result = single float in [0, 1]

The autoreseach eval harness runs LOCAL — it never calls out. This harness
mirrors that constraint. All scoring is deterministic, no subprocess calls.
"""
from __future__ import annotations
from dataclasses import dataclass
from pathlib import Path
from typing import List, Dict, Optional, Union
import json, time, re

# Default benchmark rubric — a pinned set of quality indicators
# These represent "known-good skill" quality markers.
# Any degradation here = signal to freeze/revert the hypothesis.
#
# Autoreseach benchmark = pinned shard_06542 + evaluate_bpb()
# GYM benchmark = static rubric applied to persona.md content

DEFAULT_BENCHMARK = [
    {
        "id": "has_persona_role",
        "check": "persona",
        "description": "Has a persona role defined in first 5 lines",
        "weight": 0.15,
    },
    {
        "id": "has_task_description",
        "check": "task",
        "description": "Describes the specific task or responsibility",
        "weight": 0.15,
    },
    {
        "id": "has_constraints",
        "check": "constraint|rule|never|always|must|should not",
        "description": "Has behavioral constraints or rules",
        "weight": 0.15,
    },
    {
        "id": "has_tools_or_methods",
        "check": "tool|method|skill|handler|command",
        "description": "Lists tools or methods available",
        "weight": 0.15,
    },
    {
        "id": "has_output_format",
        "check": "output|return|format|json|result",
        "description": "Specifies output format or return value",
        "weight": 0.15,
    },
    {
        "id": "is_not_too_short",
        "check": "min_length:200",
        "description": "At least 200 characters — substantive",
        "weight": 0.25,
    },
]


@dataclass
class EvalResult:
    task_id: str
    quality: float       # 0..1
    latency_ms: float
    passed: bool
    detail: str = ""

    def score(self) -> float:
        """Higher is better."""
        return self.quality  # no speed penalty in local mode


@dataclass
class HarnessResult:
    composite: float       # weighted avg, 0..1
    task_results: List[EvalResult]
    elapsed_seconds: float
    budget_hit: bool = False
    total_tokens: int = 0
    simplicity_score: float = 0.0

    def to_dict(self) -> dict:
        return {
            "composite": round(self.composite, 4),
            "simplicity_score": round(self.simplicity_score, 4),
            "task_results": [
                {"id": r.task_id, "quality": r.quality, "passed": r.passed, "detail": r.detail}
                for r in self.task_results
            ],
            "elapsed_seconds": round(self.elapsed_seconds, 2),
            "budget_hit": self.budget_hit,
        }


def _read_skill_text(skill_path: Path) -> tuple[str, float]:
    """Read skill file, return (text, latency_ms)."""
    t0 = time.time()
    if skill_path.is_dir():
        # Try persona.md first, then README.md
        for fname in ["persona.md", "README.md", "SKILL.md"]:
            p = skill_path / fname
            if p.exists():
                text = p.read_text()
                return text, (time.time() - t0) * 1000
        return "", (time.time() - t0) * 1000
    elif skill_path.exists():
        return skill_path.read_text(), (time.time() - t0) * 1000
    return "", 0.0


def _score_rubric(text: str, rubric: Dict) -> tuple[float, str]:
    """Score text against a single rubric item."""
    check = rubric.get("check", "")

    if check.startswith("min_length:"):
        min_len = int(check.split(":")[1])
        ok = len(text) >= min_len
        return (1.0 if ok else 0.0, f"len={len(text)}, min={min_len}")

    # Pattern match
    pattern = re.compile(check, re.IGNORECASE)
    match = pattern.search(text)
    ok = bool(match)
    detail = f"matched: {match.group(0)[:30]!r}" if match else "no match"
    return (1.0 if ok else 0.0, detail)


def _compute_simplicity(text: str) -> float:
    """
    Simplicity score: 0..1 where 1 = most simple.
    Autoreseach principle: code deletion is a win.
    We score skill text simplicity as: more content per character.
    """
    if not text:
        return 0.0
    lines = [l for l in text.splitlines() if l.strip()]
    words = len(text.split())
    # High ratio of words per line = dense/concise = good
    avg_words_per_line = words / max(len(lines), 1)
    # Penalize very long files (autoreseach: shorter is better)
    line_count_penalty = max(0, (len(lines) - 500) / 500) * 0.2
    score = min(1.0, avg_words_per_line / 15) - line_count_penalty
    return max(0.0, score)


def evaluate_skill(
    skill_path: Union[str, Path],
    benchmark: Optional[List[Dict]] = None,
    time_budget: float = 300.0,
    verbose: bool = False,
) -> HarnessResult:
    """
    Evaluate a skill/GYM artifact against the benchmark rubric.

    Autoreseach equivalent:
      val_bpb = evaluate_bpb(model, tokenizer, DEVICE_BATCH_SIZE)

    GYM equivalent:
      composite = evaluate_skill("mascot-pixel/").composite

    Scoring model:
      composite = sum(quality * weight) + simplicity_bonus
    where quality comes from rubric checks against the persona text.
    """
    benchmark = benchmark or DEFAULT_BENCHMARK
    skill_path = Path(skill_path)
    t_start = time.time()

    text, read_ms = _read_skill_text(skill_path)
    results: List[EvalResult] = []
    total_weight = 0.0
    weighted_sum = 0.0

    for rubric in benchmark:
        elapsed = time.time() - t_start
        if elapsed >= time_budget:
            results.append(EvalResult(
                task_id=rubric["id"],
                quality=0.0,
                latency_ms=0,
                passed=False,
                detail="BUDGET_HIT",
            ))
            continue

        quality, detail = _score_rubric(text, rubric)
        weight = rubric.get("weight", 0.1)
        passed = quality >= 0.5

        result = EvalResult(
            task_id=rubric["id"],
            quality=quality,
            latency_ms=read_ms,
            passed=passed,
            detail=detail,
        )
        results.append(result)
        weighted_sum += quality * weight
        total_weight += weight

    total_elapsed = time.time() - t_start
    rubric_score = weighted_sum / total_weight if total_weight > 0 else 0.0
    simplicity = _compute_simplicity(text)
    # composite: rubric 80% + simplicity 20%
    composite = 0.8 * rubric_score + 0.2 * simplicity
    budget_hit = total_elapsed >= time_budget

    if verbose:
        for r in results:
            status = "✓" if r.passed else "✗"
            print(f"  {status} {r.task_id}: q={r.quality:.2f} ({r.detail})")
        print(f"  Rubric: {rubric_score:.3f}  Simplicity: {simplicity:.3f}  Composite: {composite:.3f}")

    return HarnessResult(
        composite=composite,
        task_results=results,
        elapsed_seconds=total_elapsed,
        budget_hit=budget_hit,
        simplicity_score=simplicity,
    )


def load_or_init_benchmark_cache() -> Dict:
    """
    Load or initialize the benchmark result cache at:
      data/gym/eval_cache.json

    Autoreseach has results.tsv.  GYM has eval_cache.json.
    Both track: tag → score over time.
    """
    cache_path = Path.home() / "MAKAKOO" / "data" / "gym" / "eval_cache.json"
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    if cache_path.exists():
        with open(cache_path) as f:
            return json.load(f)
    return {}


def save_benchmark_result(benchmark_id: str, result: HarnessResult, tag: str = "current"):
    """Append a result to the benchmark cache."""
    cache = load_or_init_benchmark_cache()
    if benchmark_id not in cache:
        cache[benchmark_id] = []
    cache[benchmark_id].append({
        "tag": tag,
        "timestamp": time.time(),
        **result.to_dict(),
    })
    cache_path = Path.home() / "MAKAKOO" / "data" / "gym" / "eval_cache.json"
    with open(cache_path, "w") as f:
        json.dump(cache, f, indent=2)


if __name__ == "__main__":
    import sys
    skill = sys.argv[1] if len(sys.argv) > 1 else "mascot-pixel"
    r = evaluate_skill(skill, verbose=True)
    print(f"\nComposite: {r.composite:.4f}  Budget hit: {r.budget_hit}  Elapsed: {r.elapsed_seconds:.1f}s")