"""
Anti-Lazy Delegator — Prevents raw-output pass-through between workers.

The cardinal sin of multi-agent systems: dumping one agent's raw output
into the next agent's prompt. This module forces structured digestion
and specific instruction crafting between every pipeline stage.

Rule: Never pass raw output. Always specify exactly what to do.
"""

import logging
import os

import httpx

log = logging.getLogger("harvey.coordinator.anti_lazy")

SWITCH_AI_URL = os.environ.get("SWITCH_AI_URL", "http://localhost:18080/v1")
LLM_MODEL = os.environ.get("LLM_MODEL", "auto")


class AntiLazyDelegator:
    """
    Transforms raw worker output into structured findings and actionable
    instructions for downstream workers. Prevents lazy delegation.
    """

    def __init__(self, model: str = "", timeout: float = 60.0):
        self.model = model or LLM_MODEL
        self.timeout = timeout

    def _call_llm(self, system: str, user: str) -> str:
        """Make a chat completion call to switchAILocal."""
        payload = {
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": 0.2,
        }
        with httpx.Client(timeout=self.timeout) as client:
            resp = client.post(
                f"{SWITCH_AI_URL}/chat/completions",
                json=payload,
            )
            resp.raise_for_status()
            data = resp.json()

        choices = data.get("choices", [])
        if not choices:
            raise ValueError("LLM returned no choices in anti-lazy call")
        return choices[0]["message"]["content"]

    def digest_findings(self, raw_text: str) -> str:
        """
        Extract structured key findings from raw research output.

        Takes messy, verbose agent output and distills it into a clean
        list of facts, insights, and actionable data points.

        Args:
            raw_text: Raw output from a research or analysis worker.

        Returns:
            Structured digest as markdown with numbered findings.
        """
        system = (
            "You are a research analyst. Your job is to extract structured, "
            "actionable key findings from raw research output. "
            "Output a numbered list of findings. Each finding must be: "
            "1) Specific (no vague claims) "
            "2) Factual (cite sources/evidence from the input) "
            "3) Actionable (clear what to do with this information) "
            "Strip opinions, filler, and redundancy. Keep only signal."
        )
        user = f"Extract key findings from this research output:\n\n{raw_text}"
        result = self._call_llm(system, user)
        log.info("Digested findings: %d chars -> %d chars", len(raw_text), len(result))
        return result

    def craft_instructions(
        self,
        findings_digest: str,
        objective: str,
        target_role: str,
    ) -> str:
        """
        Produce specific, actionable instructions for the next worker.

        Takes a structured findings digest and the overall objective,
        then generates precise instructions tailored to the target role.
        No ambiguity, no "figure it out" — every step is explicit.

        Args:
            findings_digest: Output from digest_findings().
            objective: The top-level task objective.
            target_role: The role of the worker who will receive these
                         instructions (e.g. "implementer", "verifier").

        Returns:
            Detailed, step-by-step instructions as markdown.
        """
        system = (
            f"You are a technical lead writing instructions for a {target_role}. "
            f"You must produce SPECIFIC, ACTIONABLE instructions. "
            f"Never say 'use the research' or 'based on findings' without "
            f"quoting the exact findings. Never delegate thinking — "
            f"specify exactly what to do, how to do it, and what the output "
            f"should look like. Include concrete examples where helpful. "
            f"Rule: Never pass raw output. Always specify exactly what to do."
        )
        user = (
            f"## Objective\n{objective}\n\n"
            f"## Key Findings\n{findings_digest}\n\n"
            f"Write detailed instructions for a {target_role} agent to "
            f"accomplish the objective using these findings. "
            f"Be exhaustively specific."
        )
        result = self._call_llm(system, user)
        log.info(
            "Crafted %s instructions: %d chars",
            target_role,
            len(result),
        )
        return result
