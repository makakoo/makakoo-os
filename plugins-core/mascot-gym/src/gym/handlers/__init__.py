"""
GYM Artifact Handlers — typed evaluators per artifact kind.

ArtifactKind -> Handler mapping:
  code        -> CodeHandler     (.py .ts .go .rs .js — compile + test oracle)
  skill       -> SkillHandler    (SKILL.md — rubric + meta-harness oracle)
  agent_flow  -> AgentFlowHandler (YAML/JSON manifest + handler — replay oracle)
  text        -> TextHandler     (.md .txt .prompt — rubric + AIL judge)
  config      -> ConfigHandler   (.toml .yaml .json — parse + schema oracle)
  generic     -> GenericHandler  (unknown — AIL rubric + risk analysis)

Autoreseach model: train.py → val_bpb (single oracle, no LLM judgment)
GYM model: ArtifactHandler → validate() → EvaluationResult with val_bpb
"""
from __future__ import annotations
from pathlib import Path
from typing import Optional

from gym.handlers.base import ArtifactHandler
from gym.handlers.code import CodeHandler
from gym.handlers.skill import SkillHandler
from gym.handlers.agent_flow import AgentFlowHandler
from gym.handlers.config import ConfigHandler
from gym.handlers.text import TextHandler
from gym.handlers.generic import GenericHandler

# Registry — map kind -> handler class
HANDLERS: dict[str, type[ArtifactHandler]] = {
    "code":       CodeHandler,
    "skill":      SkillHandler,
    "agent_flow": AgentFlowHandler,
    "text":       TextHandler,
    "config":     ConfigHandler,
    "generic":    GenericHandler,
}


def detect_handler(path: Path, content: str = "") -> ArtifactHandler | None:
    """
    Auto-detect the correct handler for a file path.

    Detection order matters — most specific first:
      1. SKILL.md → SkillHandler
      2. .py/.ts/.go/.rs → CodeHandler
      3. .toml/.yaml/.json → ConfigHandler
      4. .md/.txt/.prompt → TextHandler
      5. anything else → GenericHandler
    """
    if not content:
        try:
            content = path.read_text(errors="ignore")
        except Exception:
            content = ""

    # 1. SKILL.md
    if path.name == "SKILL.md":
        return SkillHandler()

    # 2. Code by extension
    code_suffixes = [".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".rs",
                     ".java", ".cpp", ".c", ".cs", ".rb"]
    if path.suffix in code_suffixes:
        return CodeHandler()

    # 3. Config by extension
    config_suffixes = [".toml", ".yaml", ".yml", ".json"]
    if path.suffix in config_suffixes:
        handler = ConfigHandler()
        if handler.detect(path, content):
            return handler

    # 4. Text by extension or name
    text_suffixes = [".md", ".txt", ".prompt"]
    if path.suffix in text_suffixes or "README" in path.name:
        return TextHandler()

    # 5. Generic fallback
    return GenericHandler()


def detect_kind(path: Path, content: str = "") -> str:
    """Return the artifact kind string for a path."""
    handler = detect_handler(path, content)
    if handler:
        return handler.kind
    return "generic"


def run_gym_improve(path: Path, goal: str, budget_sec: int = 300) -> dict:
    """
    Run the full GYM pipeline on a single artifact.

    Pipeline:
      1. Create ArtifactCandidate from path
      2. Detect handler
      3. Propose candidate improvement via AIL
      4. Validate both baseline and candidate
      5. Return evaluation result with val_bpb

    Autoreseach equivalent: train.py → evaluate_bpb() → val_bpb → git commit

    Returns:
      {
        "candidate": ArtifactCandidate.to_dict(),
        "evaluation": EvaluationResult.to_dict(),
        "handler": str,
        "elapsed_seconds": float,
        "proposal_preview": str,
        "error": str | None
      }
    """
    from gym.artifact import ArtifactCandidate
    import time

    candidate = ArtifactCandidate.from_path(path, kind="generic", goal=goal)
    handler = detect_handler(path, candidate.baseline_content)

    if handler is None:
        return {"error": "no handler could detect this artifact", "path": str(path)}

    # Update kind from handler
    candidate.kind = handler.kind

    t0 = time.time()

    # Step 1: propose
    try:
        proposed = handler.propose(candidate, budget_sec=budget_sec)
    except Exception as e:
        return {"error": f"propose failed: {e}", "handler": handler.kind}

    # Step 2: validate
    try:
        eval_result = handler.validate(candidate, proposed)
    except Exception as e:
        return {"error": f"validate failed: {e}", "handler": handler.kind}

    elapsed = time.time() - t0

    candidate.candidate_content = proposed
    candidate.handler = handler.kind
    candidate.final_score = eval_result.candidate_score
    candidate.baseline_score = eval_result.baseline_score
    candidate.val_bpb = eval_result.val_bpb
    candidate.hard_failures = eval_result.hard_failures
    candidate.risk_warnings = eval_result.risk_warnings
    candidate.elapsed_seconds = elapsed

    return {
        "candidate": candidate.to_dict(),
        "evaluation": eval_result.to_dict(),
        "handler": handler.kind,
        "elapsed_seconds": round(elapsed, 2),
        "proposal_preview": proposed[:500],
    }