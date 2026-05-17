"""
Text artifact handler.

Detects: .md .txt .prompt (non-code text)
Oracle: rubric score (correctness, specificity, compression, structure) + AIL judge
Hard gates: not longer without quality gain (autoreseach simplicity enforced)
Autoreseach equivalent: train.py simplicity criterion — deletion preferred.
"""
from __future__ import annotations
import json
import os
import re
import urllib.request
from pathlib import Path

from gym.handlers.base import ArtifactHandler
from gym.artifact import ArtifactCandidate, EvaluationResult


class TextHandler(ArtifactHandler):
    """
    Handler for .md .txt .prompt and other text artifacts.

    Validation oracle:
      1. Rubric score: correctness, specificity, compression, structure, goal fit
      2. AIL judge (optional, expensive) — second opinion
      3. Compression ratio (lines deleted = win, autoreseach criterion)
      4. No unsupported claims introduced

    Autoreseach model: val_bpb = lower is better for bits/byte.
    GYM model: higher quality score + shorter = better.
    """

    kind = "text"
    suffixes = [".md", ".txt", ".prompt"]

    def detect(self, path: Path, content: str = "") -> bool:
        if path.suffix in self.suffixes:
            return True
        # Also detect markdown by content markers
        if path.name.endswith(".md") or "README" in path.name:
            return True
        return False

    def build_gap(self, candidate: ArtifactCandidate) -> str:
        return (
            f"Goal: {candidate.goal}\n"
            f"Text file: {candidate.path}\n"
            f"Current content ({len(candidate.baseline_content)} chars, "
            f"{len([l for l in candidate.baseline_content.splitlines() if l.strip()])} lines):\n"
            f"{candidate.baseline_content[:3000]}\n\n"
            "Constraints:\n"
            "- Edit exactly ONE file (this text)\n"
            "- Do not broaden behavior or add claims not supported by the goal\n"
            "- Prefer deletion over addition (autoreseach simplicity criterion)\n"
            "- Improve instruction compliance and failure-preventing rules\n"
            "- Return the COMPLETE replacement file — no diffs, no track-changes markup\n"
        )

    def _score_rubric(self, content: str, goal: str = "") -> tuple[float, list[str]]:
        """
        Score text against quality rubric:
          - completeness of response to goal (25 pts)
          - specificity and precision (25 pts)
          - compression (25 pts) — shorter with same info = better
          - structure and formatting (15 pts)
          - no fluff or padding (10 pts)
        """
        failures = []
        score = 0.0

        lines = [l for l in content.splitlines() if l.strip()]

        # Completeness (25 pts)
        if len(content) > 100:
            score += 15.0
            if any(x in content.lower() for x in ["# ", "##", "- ", "* ", "1.", "2.", "3."]):
                score += 10.0
            else:
                failures.append("no structural markers — may be unfocused")
        else:
            failures.append("too short to evaluate")

        # Specificity (25 pts)
        # Count concrete terms vs vague terms
        concrete = len(re.findall(r'\b\d+\b|\b[A-Z][a-z]+\b|\b\w+@\w+\.\w+\b', content))
        vague = len(re.findall(r'\bthing|stuff|some|maybe|perhaps|kinda|sorta\b', content.lower()))
        if concrete > vague:
            score += 20.0
        elif concrete > 0:
            score += 10.0
        else:
            failures.append("no concrete details — overly vague")

        # Compression (25 pts) — autoreseach principle: shorter is better
        if len(lines) > 0:
            avg_line_len = len(content) / len(lines)
            if 40 <= avg_line_len <= 120:  # dense but readable
                score += 15.0
            elif avg_line_len > 120:  # too long, verbose
                score -= 5.0

        # No fluff (10 pts)
        filler = len(re.findall(r'\bbasically|essentially|actually|really|very|quite|rather\b', content.lower()))
        if filler <= 2:
            score += 10.0
        elif filler <= 5:
            score += 5.0
        else:
            failures.append(f"excessive filler words ({filler})")

        return max(0.0, score), failures

    def _call_ail_judge(self, content: str, baseline: str, goal: str) -> float:
        """
        Use AIL to judge text quality vs baseline.
        Returns score delta (candidate - baseline, -100 to +100).
        """
        prompt = f"""Compare two text versions for the goal: "{goal}"

Baseline:
{baseline[:1000]}

Candidate:
{content[:1000]}

Score each 0-100 on: clarity, conciseness, goal fit.
Return JSON: {{"baseline_score": N, "candidate_score": N, "reason": "..."}}
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
            m = re.search(r'\{[^}]+\}', raw, re.DOTALL)
            if m:
                data = json.loads(m.group(0))
                return float(data.get("candidate_score", 50))
        except Exception:
            pass
        return 50.0

    def propose(self, candidate: ArtifactCandidate, budget_sec: int = 300) -> str:
        gap = self.build_gap(candidate)
        prompt = (
            f"{gap}\n\n"
            f"Original text:\n```\n"
            f"{candidate.baseline_content}\n"
            f"```\n\n"
            "Return ONLY the complete replacement text. No explanations, no diffs."
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
        hard_failures = []

        # Rubric scores
        baseline_score, baseline_fails = self._score_rubric(candidate.baseline_content, candidate.goal)
        candidate_score, candidate_fails = self._score_rubric(candidate_content, candidate.goal)

        # Compression = hard gate (autoreseach simplicity criterion)
        baseline_lines = len([l for l in candidate.baseline_content.splitlines() if l.strip()])
        candidate_lines = len([l for l in candidate_content.splitlines() if l.strip()])
        simplicity_delta = candidate_lines - baseline_lines

        # If candidate is longer AND score didn't improve → hard failure
        if simplicity_delta > 20 and candidate_score <= baseline_score:
            hard_failures.append(
                f"bloat without improvement: +{simplicity_delta} lines, "
                f"score {candidate_score:.0f} vs baseline {baseline_score:.0f}"
            )

        # AIL judge (optional second opinion)
        ail_score = self._call_ail_judge(candidate_content, candidate.baseline_content, candidate.goal)
        # Blend: 70% rubric + 30% AIL judge
        candidate_blended = 0.7 * candidate_score + 0.3 * ail_score
        baseline_blended = baseline_score  # no AIL for baseline (performance)

        return EvaluationResult(
            baseline_score=baseline_blended,
            candidate_score=candidate_blended,
            hard_failures=hard_failures,
            evidence={
                "rubric_score": candidate_score,
                "ail_judge_score": ail_score,
                "baseline_fails": baseline_fails,
                "candidate_fails": candidate_fails,
            },
            simplicity_delta=simplicity_delta,
        )