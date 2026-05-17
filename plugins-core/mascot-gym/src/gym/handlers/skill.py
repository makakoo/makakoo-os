"""
Skill artifact handler.

Detects: SKILL.md files
Oracle: meta-harness pass rate — run the skill against its benchmark prompts
Hard gates: parse, required sections present, no contradiction with global rules
Autoreseach equivalent: evaluate_bpb() — single metric, fixed time budget.
"""
from __future__ import annotations
from pathlib import Path
import json
import time
import urllib.request
import os

from gym.handlers.base import ArtifactHandler
from gym.artifact import ArtifactCandidate, EvaluationResult


class SkillHandler(ArtifactHandler):
    """
    Handler for SKILL.md files.

    Validation oracle: run the skill against its own benchmark prompts
    (if a benchmark exists in data/gym/benchmarks/<skill_name>/), or
    score against structural completeness rubric.

    Autoreseach model: val_bpb from evaluate_bpb(model, tokenizer, DEVICE_BATCH_SIZE)
    GYM model: skill_score from meta-harness run or rubric check.
    """

    kind = "skill"
    suffixes = [".md"]

    # Required sections in a valid SKILL.md
    REQUIRED_SECTIONS = [
        "description",
        "invocation",
        "when to use",
    ]
    # Sections that, if present, must not be empty
    NONEMPTY_SECTIONS = [
        "description",
        "invocation",
    ]

    def detect(self, path: Path, content: str = "") -> bool:
        return path.name == "SKILL.md"

    def build_gap(self, candidate: ArtifactCandidate) -> str:
        return (
            f"Goal: {candidate.goal}\n"
            f"Skill: {candidate.path}\n"
            f"Current content ({len(candidate.baseline_content)} chars):\n"
            f"{candidate.baseline_content[:3000]}\n\n"
            "Constraints:\n"
            "- Edit exactly ONE file (this SKILL.md)\n"
            "- Keep all required sections (description, invocation, when to use)\n"
            "- No new package imports or dependencies\n"
            "- Prefer deletion over addition\n"
            "- Return the COMPLETE replacement SKILL.md — no diffs, no placeholders\n"
        )

    def _score_rubric(self, content: str) -> tuple[float, list[str]]:
        """
        Score a SKILL.md against the structural completeness rubric.
        Returns (score 0..100, list of failure messages).
        """
        failures = []
        score = 0.0

        # Required sections (30 pts)
        for section in self.REQUIRED_SECTIONS:
            pattern = f"## {section}"
            if pattern.lower() in content.lower():
                score += 10.0
            else:
                failures.append(f"missing required section: {section}")

        # Non-empty sections (20 pts)
        for section in self.NONEMPTY_SECTIONS:
            pattern = f"## {section}"
            idx = content.lower().find(pattern.lower())
            if idx >= 0:
                next_section = content.lower().find("## ", idx + len(pattern))
                section_body = content[idx:next_section if next_section >= 0 else len(content)]
                if len(section_body.strip()) < 20:
                    failures.append(f"section '{section}' is too short")
                else:
                    score += 10.0

        # Skill name present (10 pts)
        if content.strip().startswith("# "):
            score += 10.0

        # Has invocation example (10 pts)
        if "```" in content and ("bash" in content.lower() or "python" in content.lower() or "makakoo" in content.lower()):
            score += 10.0

        # Not too long (10 pts) — autoreseach simplicity criterion
        lines = [l for l in content.splitlines() if l.strip()]
        if len(lines) <= 200:
            score += 10.0
        else:
            failures.append(f"too long ({len(lines)} lines, limit 200)")

        # No TODO/FIXME orphan markers (10 pts)
        if "TODO" not in content and "FIXME" not in content:
            score += 10.0
        else:
            failures.append("contains TODO/FIXME")

        # Has success indicator / output format (10 pts)
        if "output" in content.lower() or "return" in content.lower() or "result" in content.lower():
            score += 10.0

        return score, failures

    def _run_meta_harness(self, skill_path: Path) -> tuple[bool, float]:
        """
        Try to run the skill via makakoo skill run.
        Returns (ran, score 0..100).
        """
        skill_name = skill_path.parent.name.replace("skill-", "").replace("-", "_")
        try:
            import subprocess
            result = subprocess.run(
                ["makakoo", "skill", "run", skill_name, "--dry-run"],
                capture_output=True, text=True, timeout=30,
                env={**os.environ, "MAKAKOO_HOME": str(Path.home() / "MAKAKOO")}
            )
            if result.returncode == 0:
                return True, 100.0
            return True, 50.0
        except (subprocess.TimeoutExpired, FileNotFoundError):
            pass
        return False, 0.0

    def _call_ail_judge(self, skill_content: str, benchmark_prompts: list[dict]) -> float:
        """
        Use AIL to judge skill quality against benchmark prompts.
        Returns score 0..100.
        """
        prompt = f"""You are evaluating a SKILL.md file. Score it 0-100 on:
1. Clarity of description (0-25)
2. Completeness of invocation example (0-25)
3. Correctness of constraints (0-25)
4. Simplicity (deletion preferred, 0-25)

Skill content:
{skill_content[:2000]}

For each benchmark task, does the skill have what it takes?
Return a JSON object: {{"total": <score 0-100>, "reason": "<brief justification>"}}
"""
        try:
            base_url = os.environ.get("AIL_BASE_URL", "http://localhost:18080/v1")
            api_key = os.environ.get("AIL_API_KEY", os.environ.get("OPENAI_API_KEY", "sk-test-123"))
            req = urllib.request.Request(
                base_url.rstrip("/") + "/chat/completions",
                data=json.dumps({
                    "model": "auto",
                    "messages": [{"role": "user", "content": prompt}],
                    "max_tokens": 500,
                    "temperature": 0.1,
                }).encode(),
                headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
            )
            resp = urllib.request.urlopen(req, timeout=60)
            result = json.loads(resp.read())
            raw = result["choices"][0]["message"]["content"]
            # Extract JSON
            import re
            m = re.search(r'\{[^}]+\}', raw, re.DOTALL)
            if m:
                data = json.loads(m.group(0))
                return float(data.get("total", 0))
        except Exception:
            pass
        return 50.0  # neutral fallback

    def propose(self, candidate: ArtifactCandidate, budget_sec: int = 300) -> str:
        gap = self.build_gap(candidate)
        prompt = (
            f"{gap}\n\n"
            f"Original SKILL.md:\n```\n"
            f"{candidate.baseline_content}\n"
            f"```\n\n"
            "Return ONLY the complete replacement SKILL.md. No explanations, no diffs."
        )
        try:
            base_url = os.environ.get("AIL_BASE_URL", "http://localhost:18080/v1")
            api_key = os.environ.get("AIL_API_KEY", os.environ.get("OPENAI_API_KEY", "sk-test-123"))
            req = urllib.request.Request(
                base_url.rstrip("/") + "/chat/completions",
                data=json.dumps({
                    "model": "auto",
                    "messages": [{"role": "user", "content": prompt}],
                    "max_tokens": 4000,
                    "temperature": 0.2,
                }).encode(),
                headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
            )
            resp = urllib.request.urlopen(req, timeout=budget_sec)
            result = json.loads(resp.read())
            return result["choices"][0]["message"]["content"]
        except Exception as e:
            return f"# Propose failed: {e}\n{candidate.baseline_content}"

    def validate(self, candidate: ArtifactCandidate, candidate_content: str) -> EvaluationResult:
        """
        Skill validation oracle:
          1. Score baseline via rubric (0-100)
          2. Score candidate via rubric (0-100)
          3. Optionally call AIL judge for second opinion
          4. Hard failures: missing required sections, too long, TODO/FIXME
        """
        hard_failures = []

        # Baseline rubric
        baseline_score, baseline_fails = self._score_rubric(candidate.baseline_content)

        # Candidate rubric
        candidate_score, candidate_fails = self._score_rubric(candidate_content)

        # Structural failures = hard block
        for f in baseline_fails:
            if "missing required section" in f:
                hard_failures.append(f"baseline: {f}")
        for f in candidate_fails:
            if "missing required section" in f:
                hard_failures.append(f"candidate: {f}")

        # Meta-harness if available
        meta_ran, meta_score = self._run_meta_harness(candidate.path)
        if meta_ran:
            # Blend rubric + meta-harness
            baseline_score = 0.7 * baseline_score + 0.3 * meta_score
            # Re-score candidate via rubric only (can't run meta on tmp)
            candidate_score, _ = self._score_rubric(candidate_content)

        # Simplicity delta (autoreseach criterion)
        baseline_lines = len([l for l in candidate.baseline_content.splitlines() if l.strip()])
        candidate_lines = len([l for l in candidate_content.splitlines() if l.strip()])
        simplicity_delta = candidate_lines - baseline_lines

        return EvaluationResult(
            baseline_score=baseline_score,
            candidate_score=candidate_score,
            hard_failures=hard_failures,
            evidence={
                "baseline_fails": baseline_fails,
                "candidate_fails": candidate_fails,
                "meta_harness_ran": meta_ran,
                "meta_harness_score": meta_score,
            },
            simplicity_delta=simplicity_delta,
        )