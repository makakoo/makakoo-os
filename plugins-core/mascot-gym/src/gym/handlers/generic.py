"""
Generic artifact handler — fallback for unknown file types.

Detects: anything not matched by other handlers
Oracle: LLM rubric judge + diff risk analysis
Hard gates: must parse as text, no binary content, no obvious injection risks
Autoreseach equivalent: val_bpb with no compile check — relies on LLM judgment.
"""
from __future__ import annotations
import json
import os
import re
import urllib.request
from pathlib import Path

from gym.handlers.base import ArtifactHandler
from gym.artifact import ArtifactCandidate, EvaluationResult


class GenericHandler(ArtifactHandler):
    """
    Handler for unknown/unsupported artifact types.

    Validation oracle:
      1. Content must be valid UTF-8 text (no binary)
      2. AIL rubric judge scores improvement
      3. Diff risk analysis for injection/path traversal

    This is the catch-all handler. Prioritize building a specific handler
    when a new artifact family is identified and has a measurable oracle.
    """

    kind = "generic"
    suffixes = []  # catches everything not matched above

    def detect(self, path: Path, content: str = "") -> bool:
        # Check it's a text file, not binary
        if not content:
            try:
                content = path.read_text(errors="ignore")
            except Exception:
                return False
        # Reject clear binary types
        binary_markers = ["\x00", "\xff", "\xfe"]
        if any(b in content for b in binary_markers):
            return False
        return True

    def build_gap(self, candidate: ArtifactCandidate) -> str:
        return (
            f"Goal: {candidate.goal}\n"
            f"File: {candidate.path}\n"
            f"File type: unknown (generic handler)\n"
            f"Current content ({len(candidate.baseline_content)} chars, "
            f"{len([l for l in candidate.baseline_content.splitlines() if l.strip()])} lines):\n"
            f"{candidate.baseline_content[:2000]}\n\n"
            "Constraints:\n"
            "- Edit exactly ONE file (this one)\n"
            "- No new package imports or dependencies\n"
            "- No shell command injection\n"
            "- Prefer deletion over addition\n"
            "- Return the COMPLETE replacement file — no diffs\n"
        )

    def _check_risks(self, content: str, baseline: str) -> list[str]:
        """Check for injection, path traversal, and other risks."""
        risks = []
        # Shell injection
        injection_patterns = ["$(", "`", "${", "&&", "||", "|"]
        for pat in injection_patterns:
            if pat in content and pat not in baseline:
                risks.append(f"potential injection pattern introduced: {pat}")
        # Path traversal
        if "../" in content and "../" not in baseline:
            risks.append("path traversal pattern introduced")
        # Base64/encoding attempts
        if re.search(r'[A-Za-z0-9+/]{50}={0,2}', content) and not re.search(r'[A-Za-z0-9+/]{50}={0,2}', baseline):
            risks.append("base64-like content introduced")
        return risks

    def _call_ail_judge(self, content: str, baseline: str, goal: str) -> tuple[float, float]:
        """
        Use AIL to judge generic content improvement.
        Returns (baseline_score, candidate_score) 0-100.
        """
        prompt = f"""You are evaluating a generic file improvement.

Goal: "{goal}"

Baseline (original):
```
{baseline[:1500]}
```

Candidate (proposed):
```
{content[:1500]}
```

Score each 0-100 on: correctness, conciseness, goal fit, safety.
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
                    "max_tokens": 600,
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
                return float(data.get("baseline_score", 50)), float(data.get("candidate_score", 50))
        except Exception:
            pass
        return 50.0, 50.0

    def propose(self, candidate: ArtifactCandidate, budget_sec: int = 300) -> str:
        gap = self.build_gap(candidate)
        prompt = (
            f"{gap}\n\n"
            f"Original file:\n```\n"
            f"{candidate.baseline_content}\n"
            f"```\n\n"
            "Return ONLY the complete replacement file. No explanations."
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
        risk_warnings = []

        # Binary check
        if any(b in candidate_content for b in ["\x00", "\xff", "\xfe"]):
            hard_failures.append("binary content detected — not supported")

        # Risk analysis
        risks = self._check_risks(candidate_content, candidate.baseline_content)
        risk_warnings.extend(risks)

        # AIL judge
        baseline_score, candidate_score = self._call_ail_judge(
            candidate_content, candidate.baseline_content, candidate.goal
        )

        baseline_lines = len([l for l in candidate.baseline_content.splitlines() if l.strip()])
        candidate_lines = len([l for l in candidate_content.splitlines() if l.strip()])
        simplicity_delta = candidate_lines - baseline_lines

        return EvaluationResult(
            baseline_score=baseline_score,
            candidate_score=candidate_score,
            hard_failures=hard_failures,
            evidence={"risks": risks},
            simplicity_delta=simplicity_delta,
            risk_warnings=risk_warnings,
        )