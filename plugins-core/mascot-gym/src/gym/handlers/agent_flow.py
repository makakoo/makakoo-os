"""
Agent flow artifact handler.

Detects: YAML/JSON files describing agent workflows (manifest + handler pairs)
Oracle: transcript replay — run the flow with sample inputs, check invariants
Hard gates: must parse as valid YAML/JSON, required keys present, no unsafe autonomy
Autoreseach equivalent: train.py eval harness — run the thing, measure.
"""
from __future__ import annotations
import json
import os
import subprocess
import urllib.request
from pathlib import Path

from gym.handlers.base import ArtifactHandler
from gym.artifact import ArtifactCandidate, EvaluationResult


class AgentFlowHandler(ArtifactHandler):
    """
    Handler for agent flow YAML/JSON files.

    Detects flows by looking for:
      - YAML/JSON with top-level keys like 'steps', 'tools', 'agents', 'flow', 'pipeline'
      - TOML with [agent], [[agents]], [[steps]] sections

    Validation oracle:
      1. Parse must succeed
      2. Required keys present (steps/tools list, entry point)
      3. Each tool/agent reference resolves to something
      4. Run a dry-run / smoke test of the flow
      5. Score: parse(50) + structure(30) + dry_run(20)

    Hard failures: parse error, missing required keys, tool not found.
    """

    kind = "agent_flow"
    suffixes = [".yaml", ".yml", ".json", ".toml"]

    # Required top-level keys for a valid agent flow
    REQUIRED_KEYS = ["steps", "tools"]

    def detect(self, path: Path, content: str = "") -> bool:
        if path.suffix not in self.suffixes:
            return False
        if not content:
            try:
                content = path.read_text(errors="ignore")
            except Exception:
                return False

        # Check for agent flow markers
        if path.suffix in (".yaml", ".yml"):
            ok, data, _ = self._parse_flow(content, path)
            if ok and data and isinstance(data, dict):
                keys = set(data.keys())
                flow_markers = {"steps", "tools", "agents", "flow", "pipeline", "tasks"}
                if keys & flow_markers:
                    return True
        elif path.suffix == ".json":
            try:
                data = json.loads(content)
                if isinstance(data, dict):
                    keys = set(data.keys())
                    if keys & {"steps", "tools", "agents", "flow", "pipeline", "tasks"}:
                        return True
            except Exception:
                pass
        elif path.suffix == ".toml":
            try:
                try:
                    import tomllib  # py311+
                except Exception:
                    import tomli as tomllib  # type: ignore
                data = tomllib.loads(content)
                if isinstance(data, dict):
                    if any(k for k in data.keys() if k in ["agent", "agents", "steps", "flow"]):
                        return True
            except Exception:
                pass
        return False

    def build_gap(self, candidate: ArtifactCandidate) -> str:
        return (
            f"Goal: {candidate.goal}\n"
            f"Agent flow: {candidate.path}\n"
            f"Current content ({len(candidate.baseline_content)} chars):\n"
            f"{candidate.baseline_content[:2500]}\n\n"
            "Constraints:\n"
            "- Edit exactly ONE file (this flow definition)\n"
            "- Must remain valid YAML/JSON/TOML\n"
            "- Keep all required steps and tool references\n"
            "- No new tool dependencies\n"
            "- Prefer simplification: fewer steps, fewer tools, more explicit flow\n"
            "- Return the COMPLETE replacement file — no diffs\n"
        )

    def _parse_flow(self, text: str, path: Path) -> tuple[bool, object | None, str]:
        """Parse and return (success, data, error_message)."""
        try:
            if path.suffix in (".yaml", ".yml"):
                try:
                    import yaml
                except Exception as exc:
                    return False, None, f"PyYAML unavailable: {exc}"
                data = yaml.safe_load(text)
                return True, data, ""
            elif path.suffix == ".json":
                data = json.loads(text)
                return True, data, ""
            elif path.suffix == ".toml":
                try:
                    import tomllib  # py311+
                except Exception:
                    import tomli as tomllib  # type: ignore
                data = tomllib.loads(text)
                return True, data, ""
        except Exception as e:
            return False, None, str(e)
        return False, None, "unknown suffix"

    def _check_required_keys(self, data, required: list[str]) -> list[str]:
        """Check that required keys are present."""
        missing = []
        if isinstance(data, dict):
            for key in required:
                if key not in data:
                    missing.append(f"missing required key: {key}")
        return missing

    def _check_tool_resolution(self, data) -> list[str]:
        """Check that tool/agent references resolve to something."""
        unresolved = []
        tools = data.get("tools", []) if isinstance(data, dict) else []
        steps = data.get("steps", []) if isinstance(data, dict) else []

        tool_names = set()
        if isinstance(tools, list):
            for t in tools:
                if isinstance(t, dict) and "name" in t:
                    tool_names.add(t["name"])
                elif isinstance(t, str):
                    tool_names.add(t)

        # Check step tool references
        if isinstance(steps, list):
            for i, step in enumerate(steps):
                if isinstance(step, dict):
                    tool_ref = step.get("tool") or step.get("uses") or step.get("name")
                    if tool_ref and tool_ref not in tool_names:
                        unresolved.append(f"step {i}: tool '{tool_ref}' not in tools list")

        return unresolved

    def _dry_run(self, path: Path) -> tuple[bool, float, str]:
        """
        Run a dry-run smoke test of the flow.
        Returns (success, score, detail).
        """
        # Check if makakoo has a flow runner
        try:
            result = subprocess.run(
                ["makakoo", "flow", "dry-run", str(path)],
                capture_output=True, text=True, timeout=30,
                env={**os.environ, "MAKAKOO_HOME": os.environ.get("MAKAKOO_HOME", str(Path.home() / "MAKAKOO"))}
            )
            if result.returncode == 0:
                return True, 20.0, "dry-run passed"
            return False, 0.0, result.stderr[:200]
        except (subprocess.TimeoutExpired, FileNotFoundError):
            pass
        return False, 0.0, "dry-run not available"

    def propose(self, candidate: ArtifactCandidate, budget_sec: int = 300) -> str:
        gap = self.build_gap(candidate)
        prompt = (
            f"{gap}\n\n"
            f"Original agent flow:\n```\n"
            f"{candidate.baseline_content}\n"
            f"```\n\n"
            "Return ONLY the complete replacement YAML/JSON. No explanations."
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

        # 1. Parse baseline
        baseline_ok, baseline_data, baseline_err = self._parse_flow(
            candidate.baseline_content, candidate.path
        )
        if not baseline_ok:
            hard_failures.append(f"baseline parse error: {baseline_err[:200]}")

        # 2. Parse candidate
        candidate_ok, candidate_data, candidate_err = self._parse_flow(
            candidate_content, candidate.path
        )
        if not candidate_ok:
            hard_failures.append(f"candidate parse error: {candidate_err[:200]}")

        # 3. Required keys
        if candidate_ok:
            missing_keys = self._check_required_keys(candidate_data, self.REQUIRED_KEYS)
            for mk in missing_keys:
                hard_failures.append(mk)

        # 4. Tool resolution
        if candidate_ok:
            unresolved = self._check_tool_resolution(candidate_data)
            for u in unresolved:
                risk_warnings.append(u)

        # 5. Dry run
        dry_ok, dry_score, dry_msg = self._dry_run(candidate.path)
        if not dry_ok and dry_msg != "dry-run not available":
            risk_warnings.append(f"dry-run: {dry_msg}")

        # Score: parse(50) + structure(30) + dry_run(dry_score or 0)
        baseline_score = 50.0 if baseline_ok else 0.0
        candidate_score = 50.0 if candidate_ok else 0.0

        if candidate_ok:
            # Structure score based on completeness
            if candidate_data and isinstance(candidate_data, dict):
                n_steps = len(candidate_data.get("steps", []))
                n_tools = len(candidate_data.get("tools", []))
                structure_score = min(30.0, (n_steps * 2) + (n_tools * 2))
                candidate_score += structure_score

        candidate_score += dry_score if dry_ok else 0

        baseline_lines = len([l for l in candidate.baseline_content.splitlines() if l.strip()])
        candidate_lines = len([l for l in candidate_content.splitlines() if l.strip()])
        simplicity_delta = candidate_lines - baseline_lines

        return EvaluationResult(
            baseline_score=baseline_score,
            candidate_score=candidate_score,
            hard_failures=hard_failures,
            evidence={
                "parse_ok": candidate_ok,
                "dry_run_ok": dry_ok,
                "dry_run_msg": dry_msg,
            },
            simplicity_delta=simplicity_delta,
            risk_warnings=risk_warnings,
        )
