"""Config for native Makakoo Cortex memory."""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Any, Mapping


def _coerce_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    return bool(value)


def _env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _env_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def _env_float(name: str, default: float) -> float:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        return float(raw)
    except ValueError:
        return default


@dataclass
class CortexConfig:
    enabled: bool = False
    memory_limit: int = 5
    min_confidence: float = 0.70
    min_importance: float = 0.40
    pii_scrubbing: bool = True
    max_memory_chars: int = 500
    max_prompt_memory_chars: int = 1200
    max_memory_age_days: int = 365
    app_id: str = "makakoo-harveychat"

    @classmethod
    def from_mapping(cls, raw: Mapping[str, Any] | None) -> "CortexConfig":
        cfg = cls()
        if not raw:
            return cfg
        for key in (
            "enabled",
            "memory_limit",
            "min_confidence",
            "min_importance",
            "pii_scrubbing",
            "max_memory_chars",
            "max_prompt_memory_chars",
            "max_memory_age_days",
            "app_id",
        ):
            if key in raw:
                setattr(cfg, key, raw[key])
        cfg.enabled = _coerce_bool(cfg.enabled)
        cfg.memory_limit = int(cfg.memory_limit)
        cfg.min_confidence = float(cfg.min_confidence)
        cfg.min_importance = float(cfg.min_importance)
        cfg.pii_scrubbing = _coerce_bool(cfg.pii_scrubbing)
        cfg.max_memory_chars = int(cfg.max_memory_chars)
        cfg.max_prompt_memory_chars = int(cfg.max_prompt_memory_chars)
        cfg.max_memory_age_days = int(cfg.max_memory_age_days)
        return cfg

    def apply_env(self) -> "CortexConfig":
        # MAKAKOO_* wins; HARVEY_* kept as compatibility alias for enabled only.
        alias_enabled = _env_bool("HARVEY_CORTEX_ENABLED", self.enabled)
        self.enabled = _env_bool("MAKAKOO_CORTEX_ENABLED", alias_enabled)
        self.memory_limit = _env_int("MAKAKOO_CORTEX_MEMORY_LIMIT", self.memory_limit)
        self.min_confidence = _env_float("MAKAKOO_CORTEX_MIN_CONFIDENCE", self.min_confidence)
        self.min_importance = _env_float("MAKAKOO_CORTEX_MIN_IMPORTANCE", self.min_importance)
        self.pii_scrubbing = _env_bool("MAKAKOO_CORTEX_PII_SCRUBBING", self.pii_scrubbing)
        self.max_memory_chars = _env_int("MAKAKOO_CORTEX_MAX_MEMORY_CHARS", self.max_memory_chars)
        self.max_prompt_memory_chars = _env_int("MAKAKOO_CORTEX_MAX_PROMPT_CHARS", self.max_prompt_memory_chars)
        self.max_memory_age_days = _env_int("MAKAKOO_CORTEX_MAX_AGE_DAYS", self.max_memory_age_days)
        return self
