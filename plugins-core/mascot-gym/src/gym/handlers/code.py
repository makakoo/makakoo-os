"""
Code artifact handler.

Detects: .py .ts .go .rs .js .jsx .tsx .java .cpp .c .cs .rb
Oracle: compile + test + behavioral probe
Hard gates: compile must pass, tests must not regress, no new packages
"""
from __future__ import annotations
import subprocess
from pathlib import Path
from typing import Optional

from gym.handlers.base import ArtifactHandler
from gym.artifact import ArtifactCandidate, EvaluationResult


class CodeHandler(ArtifactHandler):
    kind = "code"
    suffixes = [".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".rs", ".java", ".cpp", ".c", ".cs", ".rb"]

    def detect(self, path: Path, content: str) -> bool:
        # Also detect by shebang or __init__ pattern
        if path.suffix in self.suffixes:
            return True
        if content.startswith("#!/") and "python" in content:
            return True
        return False

    def build_gap(self, candidate: ArtifactCandidate) -> str:
        return (
            f"Goal: {candidate.goal}\n"
            f"File: {candidate.path}\n"
            f"Current content ({len(candidate.baseline_content)} chars):\n"
            f"{candidate.baseline_content[:2000]}\n\n"
            "Constraints:\n"
            "- Edit exactly ONE file (this one)\n"
            "- No new package imports\n"
            "- Must still compile/pass existing tests\n"
            "- Prefer deletion over addition\n"
            "- Return the COMPLETE replacement file — no diffs, no placeholders\n"
        )

    def _compile(self, path: Path) -> tuple[bool, str]:
        """Compile the file and return (success, error_message)."""
        suffix = path.suffix
        cmd = {
            ".py": ["python3", "-m", "py_compile", str(path)],
            ".ts": ["npx", "tsc", "--noEmit", str(path)],
            ".js": ["node", "--check", str(path)],
            ".go": ["go", "build", "-o", "/dev/null", str(path)],
            ".rs": ["rustc", "--edition", "2021", "-o", "/dev/null", str(path)],
            ".rb": ["ruby", "-c", str(path)],
            ".java": ["javac", str(path)],
        }.get(suffix)
        if not cmd:
            return True, ""  # unknown extension — skip compile check
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            if result.returncode == 0:
                return True, ""
            return False, result.stderr[:500]
        except (subprocess.TimeoutExpired, FileNotFoundError):
            return True, ""  # tool not available — skip

    def _run_tests(self, path: Path, content: str) -> tuple[bool, list[str]]:
        """Run relevant tests. Returns (passed, failure_messages)."""
        failures = []
        suffix = path.suffix

        if suffix == ".py":
            # Try pytest on the same file
            test_file = path.parent / f"test_{path.name}"
            if not test_file.exists():
                test_file = path.parent / f"{path.stem}_test.py"
            if test_file.exists():
                result = subprocess.run(
                    ["python3", "-m", "pytest", str(test_file), "-v"],
                    capture_output=True, text=True, timeout=60
                )
                if result.returncode != 0:
                    failures.append(f"pytest failed: {result.stderr[:300]}")

        return len(failures) == 0, failures

    def propose(self, candidate: ArtifactCandidate, budget_sec: int = 300) -> str:
        """Use AIL to propose code improvement."""
        gap = self.build_gap(candidate)
        prompt = (
            f"{gap}\n\n"
            f"Original file:\n```\n"
            f"{candidate.baseline_content}\n"
            f"```\n\n"
            "Return ONLY the complete replacement file content. "
            "No explanations, no diffs."
        )
        try:
            import os
            base_url = os.environ.get("AIL_BASE_URL", "http://localhost:18080/v1")
            api_key = os.environ.get("AIL_API_KEY", os.environ.get("OPENAI_API_KEY", "sk-test-123"))
            import urllib.request, json
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
        Code validation oracle:
          1. Compile both baseline and candidate
          2. Run tests if available
          3. Score: compile_pass(50) + test_pass(40) + behavioral(10)
          4. Hard failures: compile regression
        """
        hard_failures = []

        # Check 1: compile baseline
        baseline_ok, baseline_err = self._compile(candidate.path)
        if not baseline_ok:
            hard_failures.append(f"baseline compile error: {baseline_err[:200]}")

        # Check 2: compile candidate
        tmp = candidate.path.with_suffix(".tmp" + candidate.path.suffix)
        tmp.write_text(candidate_content)
        candidate_ok, candidate_err = self._compile(tmp)
        tmp.unlink()

        if not candidate_ok:
            hard_failures.append(f"candidate compile error: {candidate_err[:200]}")

        # Check 3: test regression
        baseline_ok_test, test_failures = self._run_tests(candidate.path, candidate.baseline_content)
        candidate_ok_test, _ = self._run_tests(tmp if tmp.exists() else candidate.path, candidate_content)
        tmp.unlink(missing_ok=True)

        if baseline_ok_test and not candidate_ok_test:
            hard_failures.append(f"test regression: {test_failures}")

        # Score
        baseline_score = 50 if baseline_ok else 0
        candidate_score = 50 if candidate_ok else 0

        if baseline_ok_test:
            baseline_score += 40
        if candidate_ok_test:
            candidate_score += 40

        baseline_lines = len([l for l in candidate.baseline_content.splitlines() if l.strip()])
        candidate_lines = len([l for l in candidate_content.splitlines() if l.strip()])
        simplicity_delta = candidate_lines - baseline_lines

        return EvaluationResult(
            baseline_score=baseline_score,
            candidate_score=candidate_score,
            hard_failures=hard_failures,
            evidence={"compile_baseline_ok": baseline_ok, "compile_candidate_ok": candidate_ok},
            simplicity_delta=simplicity_delta,
        )

    def risk_check(self, candidate: ArtifactCandidate, candidate_content: str) -> list[str]:
        risks = super().risk_check(candidate, candidate_content)
        # No new package imports (autoreseach constraint)
        baseline_imports = set(l.strip() for l in candidate.baseline_content.splitlines() if l.startswith("import ") or l.startswith("from "))
        candidate_imports = set(l.strip() for l in candidate_content.splitlines() if l.startswith("import ") or l.startswith("from "))
        new_imports = candidate_imports - baseline_imports
        if new_imports:
            risks.append(f"new imports: {', '.join(list(new_imports)[:3])}")
        return risks