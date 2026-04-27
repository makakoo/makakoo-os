"""Per-slot LLM config reader.

Reads the locked `MAKAKOO_LLM_*` env vars set by the supervisor on
gateway spawn. Returns an `LlmConfig` dataclass the dispatch layer
can pass straight into the SwitchAILocal client.

Locked env keys (Phase 4):
  MAKAKOO_LLM_MODEL
  MAKAKOO_LLM_MAX_TOKENS
  MAKAKOO_LLM_TEMPERATURE
  MAKAKOO_LLM_REASONING_EFFORT  (low | medium | high)
  MAKAKOO_LLM_TOP_P
"""

from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass
class LlmConfig:
    model: str
    max_tokens: int
    temperature: float
    reasoning_effort: str
    top_p: float

    @classmethod
    def from_env(cls, env: dict | None = None) -> "LlmConfig":
        e = env if env is not None else os.environ
        return cls(
            model=e.get("MAKAKOO_LLM_MODEL", "ail-compound"),
            max_tokens=int(e.get("MAKAKOO_LLM_MAX_TOKENS", "4096")),
            temperature=float(e.get("MAKAKOO_LLM_TEMPERATURE", "0.7")),
            reasoning_effort=e.get("MAKAKOO_LLM_REASONING_EFFORT", "medium"),
            top_p=float(e.get("MAKAKOO_LLM_TOP_P", "1.0")),
        )
