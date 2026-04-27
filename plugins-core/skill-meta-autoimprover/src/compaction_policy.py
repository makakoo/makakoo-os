#!/usr/bin/env python3
"""
Session Compaction Policy — Sprint 3

Defines when sessions should be rotated/compacted.
Based on Paperclip's SessionCompactionPolicy interface.

Thresholds:
  - max_session_runs: rotate after N turns in this session
  - max_raw_input_tokens: rotate when accumulated input tokens exceed this
  - max_session_age_hours: rotate after N hours since session start
  - max_total_tokens: rotate when total tokens (in+out) exceed this
"""

from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Optional


@dataclass
class CompactionPolicy:
    """Session compaction policy with per-threshold configuration."""
    enabled: bool = True

    # Threshold: rotate after N tool-call iterations in this session
    max_session_runs: int = 50

    # Threshold: rotate when input tokens exceed this
    max_raw_input_tokens: int = 150_000

    # Threshold: rotate when total tokens (in + out) exceed this
    max_total_tokens: int = 500_000

    # Threshold: rotate after N hours since session start
    max_session_age_hours: float = 2.0

    # Grace period: minimum turns before first compaction can fire
    min_session_runs_before_compaction: int = 10


@dataclass
class CompactionState:
    """Tracks compaction state for one session."""
    session_id: str
    started_at: datetime = field(default_factory=datetime.now)
    runs: int = 0
    input_tokens: int = 0
    output_tokens: int = 0

    # Cumulative per-run tracking
    _run_tokens: list = field(default_factory=list)  # [(input, output), ...]

    def should_compact(self, policy: CompactionPolicy) -> bool:
        """Check all thresholds. Returns True if ANY threshold is exceeded."""
        if not policy.enabled:
            return False

        if self.runs < policy.min_session_runs_before_compaction:
            return False

        if policy.max_session_runs > 0 and self.runs >= policy.max_session_runs:
            return True

        if (
            policy.max_raw_input_tokens > 0
            and self.input_tokens >= policy.max_raw_input_tokens
        ):
            return True

        if (
            policy.max_total_tokens > 0
            and self.total_tokens >= policy.max_total_tokens
        ):
            return True

        if (
            policy.max_session_age_hours > 0
            and self.session_age_hours >= policy.max_session_age_hours
        ):
            return True

        return False

    def record_run(self, input_tokens: int, output_tokens: int) -> None:
        """Record tokens for one run."""
        self._run_tokens.append((input_tokens, output_tokens))
        self.input_tokens += input_tokens
        self.output_tokens += output_tokens
        self.runs += 1

    @property
    def total_tokens(self) -> int:
        """Total tokens (in + out)."""
        return self.input_tokens + self.output_tokens

    @property
    def session_age_hours(self) -> float:
        """Hours since session started."""
        delta = datetime.now() - self.started_at
        return delta.total_seconds() / 3600.0

    def reset(self) -> None:
        """Reset state for a new session."""
        self.runs = 0
        self.input_tokens = 0
        self.output_tokens = 0
        self._run_tokens.clear()
        self.started_at = datetime.now()


def resolve_compaction_policy(
    runtime_config: Optional[dict] = None,
    default: Optional[CompactionPolicy] = None,
) -> CompactionPolicy:
    """Resolve policy from runtime config, with defaults.

    runtime_config example:
        {"maxSessionRuns": 30, "maxRawInputTokens": 100000}
    """
    base = default if default is not None else CompactionPolicy()

    if runtime_config is None:
        return base

    cfg = runtime_config if isinstance(runtime_config, dict) else {}

    enabled = cfg.get("enabled")
    max_session_runs = cfg.get("maxSessionRuns")
    max_raw_input_tokens = cfg.get("maxRawInputTokens")
    max_total_tokens = cfg.get("maxTotalTokens")
    max_session_age_hours = cfg.get("maxSessionAgeHours")
    min_session_runs_before_compaction = cfg.get("minSessionRunsBeforeCompaction")

    return CompactionPolicy(
        enabled=base.enabled if enabled is None else bool(enabled),
        max_session_runs=base.max_session_runs if max_session_runs is None else int(max_session_runs),
        max_raw_input_tokens=base.max_raw_input_tokens if max_raw_input_tokens is None else int(max_raw_input_tokens),
        max_total_tokens=base.max_total_tokens if max_total_tokens is None else int(max_total_tokens),
        max_session_age_hours=base.max_session_age_hours if max_session_age_hours is None else float(max_session_age_hours),
        min_session_runs_before_compaction=base.min_session_runs_before_compaction if min_session_runs_before_compaction is None else int(min_session_runs_before_compaction),
    )
