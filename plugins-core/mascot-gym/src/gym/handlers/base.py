"""
Base artifact handler — protocol for all GYM artifact handlers.

Every artifact kind (code, skill, agent_flow, text, config, generic)
implements this protocol. The pipeline calls detect(), build_gap(),
propose(), validate(), risk_check() in sequence. Only validate() and
propose() are required; others have default stubs.

Autoreseach model: train.py is the only editable file, val_bpb is the oracle.
GYM model: ArtifactHandler is the typed wrapper around any artifact kind,
unified by the same 5-layer pipeline.
"""
from __future__ import annotations
from abc import ABC, abstractmethod
from pathlib import Path
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from gym.artifact import ArtifactCandidate, EvaluationResult


class ArtifactHandler(ABC):
    """
    Protocol for GYM artifact handlers.

    Implement one handler per artifact kind (code, skill, agent_flow, etc.).
    The handler knows:
      - how to detect if a file is its kind
      - how to build a "gap" prompt for the proposer
      - how to propose an improved version
      - how to validate the candidate
      - what hard gates to enforce

    Autoreseach equivalent: train.py's eval harness (evaluate_bpb).
    GYM equivalent: this handler's validate() method.
    """

    kind: str  # e.g. "code", "skill", "agent_flow"

    # ── Detection ──────────────────────────────────────────────────────────

    def detect(self, path: Path, content: str) -> bool:
        """
        Return True if this handler owns this file type.
        Default: extension check. Override for custom detection.
        """
        suffixes = getattr(self, "suffixes", [])
        return any(str(path).endswith(s) for s in suffixes)

    # ── Gap building ──────────────────────────────────────────────────────

    def build_gap(self, candidate: "ArtifactCandidate") -> str:
        """
        Build the gap prompt that goes to the LLM proposer.
        Default: generic gap. Override for handler-specific context.
        """
        return (
            f"Goal: {candidate.goal}\n"
            f"File: {candidate.path}\n"
            f"Current content:\n{candidate.baseline_content[:1000]}\n"
            "Produce a complete replacement file that achieves the goal."
        )

    # ── Proposal ──────────────────────────────────────────────────────────

    @abstractmethod
    def propose(self, candidate: "ArtifactCandidate", budget_sec: int = 300) -> str:
        """
        Generate a candidate improvement.
        Returns the complete replacement content for candidate.path.
        Must respect:
          - budget_sec (hard wall clock)
          - no new packages
          - exactly one file (the artifact's primary file)
        """
        ...

    # ── Validation ────────────────────────────────────────────────────────

    @abstractmethod
    def validate(self, candidate: "ArtifactCandidate", candidate_content: str) -> "EvaluationResult":
        """
        Run the oracle — score both baseline and candidate.
        Returns EvaluationResult with baseline_score, candidate_score,
        hard_failures (must be empty for merge), simplicity_delta, risk_warnings.

        Autoreseach equivalent: evaluate_bpb(model, tokenizer, DEVICE_BATCH_SIZE)
        which returns a single val_bpb float.
        """
        ...

    # ── Risk ──────────────────────────────────────────────────────────────

    def risk_check(self, candidate: "ArtifactCandidate", candidate_content: str) -> list[str]:
        """
        Return list of risk warnings (empty = safe to merge).
        Default implementation checks common patterns.
        Override for handler-specific risks.
        """
        risks = []
        # No new package imports (autoreseach constraint)
        if "import " in candidate_content and "import " not in candidate.baseline_content:
            risks.append("introduces new import — review for new packages")
        # Path traversal risk
        if "../" in candidate_content and "../" not in candidate.baseline_content:
            risks.append("contains path traversal — security review needed")
        # Secrets introduced
        if any(kw in candidate_content for kw in ["API_KEY", "SECRET", "PASSWORD", "PRIVATE_KEY"]):
            if not any(kw in candidate.baseline_content for kw in ["API_KEY", "SECRET", "PASSWORD", "PRIVATE_KEY"]):
                risks.append("introduces secret/API key — blocked unless goal explicitly requires it")
        return risks

    # ── Render ─────────────────────────────────────────────────────────────

    def render_review(self, candidate: "ArtifactCandidate", candidate_content: str,
                      eval_result: "EvaluationResult") -> str:
        """
        Human-readable review summary for the lope gate.
        """
        return (
            f"[{self.kind}] {candidate.path.name}\n"
            f"Goal: {candidate.goal}\n"
            f"Baseline score: {eval_result.baseline_score:.4f}\n"
            f"Candidate score: {eval_result.candidate_score:.4f}\n"
            f"Hard failures: {len(eval_result.hard_failures)}\n"
            f"Simplicity delta: {eval_result.simplicity_delta:+.0f} lines\n"
            f"Val_bpb: {eval_result.val_bpb:+.4f}\n"
            f"Risks: {len(eval_result.risk_warnings)}\n"
            f"Passed: {eval_result.passed}\n"
        )