#!/usr/bin/env python3
"""
Budget Config — Sprint 4

Default budget policies and configuration resolution.
Merges defaults + runtime config + per-session overrides.
"""

import os
from dataclasses import dataclass, field
from enum import Enum
from typing import Optional, List, Dict, Any

try:
    from .budget_enforcer import BudgetLimit, BudgetState
    from .budget_tracker import BudgetTracker
except ImportError:
    # Stub types until budget_enforcer / budget_tracker are implemented.
    # These will be replaced with real imports when those modules land.
    from dataclasses import dataclass as _dc

    @_dc
    class BudgetLimit:
        name: str
        limit: float
        period: "BudgetPeriod"  # type: ignore

    class BudgetState:  # type: ignore
        pass

    class BudgetTracker:  # type: ignore
        pass


class BudgetPeriod(Enum):
    """Budget period — how often the budget resets."""
    SESSION = "session"   # Reset each session
    DAILY = "daily"        # Reset each day
    MONTHLY = "monthly"    # Reset each month


@dataclass
class BudgetPolicy:
    """Full budget policy for an agent or company."""
    name: str = "default"

    # Token limits
    max_tokens_per_session: Optional[int] = 500_000
    max_tokens_per_day: Optional[int] = 2_000_000
    max_tokens_per_month: Optional[int] = 10_000_000

    # Cost limits (USD)
    max_cost_per_session: Optional[float] = 5.0
    max_cost_per_day: Optional[float] = 20.0
    max_cost_per_month: Optional[float] = 100.0

    # Turn limits
    max_turns_per_session: Optional[int] = 200

    # Warning threshold
    warning_pct: float = 80.0

    # Whether this policy is enabled
    enabled: bool = True

    def to_limits(self) -> List[BudgetLimit]:
        """Convert policy to a list of BudgetLimit objects."""
        limits = []
        # Token limits
        if self.max_tokens_per_session is not None:
            limits.append(BudgetLimit(name="tokens_session", limit=self.max_tokens_per_session, period=BudgetPeriod.SESSION))
        if self.max_tokens_per_day is not None:
            limits.append(BudgetLimit(name="tokens_daily", limit=self.max_tokens_per_day, period=BudgetPeriod.DAILY))
        if self.max_tokens_per_month is not None:
            limits.append(BudgetLimit(name="tokens_monthly", limit=self.max_tokens_per_month, period=BudgetPeriod.MONTHLY))
        # Cost limits
        if self.max_cost_per_session is not None:
            limits.append(BudgetLimit(name="cost_session", limit=self.max_cost_per_session, period=BudgetPeriod.SESSION))
        if self.max_cost_per_day is not None:
            limits.append(BudgetLimit(name="cost_daily", limit=self.max_cost_per_day, period=BudgetPeriod.DAILY))
        if self.max_cost_per_month is not None:
            limits.append(BudgetLimit(name="cost_monthly", limit=self.max_cost_per_month, period=BudgetPeriod.MONTHLY))
        # Turn limits
        if self.max_turns_per_session is not None:
            limits.append(BudgetLimit(name="turns_session", limit=self.max_turns_per_session, period=BudgetPeriod.SESSION))
        return limits


DEFAULT_POLICY = BudgetPolicy()


# Config keys that can come from environment or config file
BUDGET_ENV_VARS = {
    "max_tokens_per_session": "HARVEY_MAX_TOKENS_SESSION",
    "max_cost_per_session": "HARVEY_MAX_COST_SESSION",
    "max_turns_per_session": "HARVEY_MAX_TURNS_SESSION",
    "warning_pct": "HARVEY_BUDGET_WARNING_PCT",
}


def resolve_budget_policy(
    runtime_config: Optional[Dict[str, Any]] = None,
    env_override: bool = True,
) -> BudgetPolicy:
    """Resolve budget policy from defaults + runtime config + env vars.

    Priority (highest to lowest):
      1. Runtime config (programmatic override)
      2. Environment variables (HARVEY_MAX_*)
      3. Default policy

    Args:
        runtime_config: Dict with policy overrides (from config file, API, etc.)
        env_override: Whether to read HARVEY_* env vars

    Returns:
        Merged BudgetPolicy
    """
    # Start with default policy
    merged: Dict[str, Any] = {}

    # Collect fields from DEFAULT_POLICY
    for f in DEFAULT_POLICY.__dataclass_fields__.values():
        merged[f.name] = getattr(DEFAULT_POLICY, f.name)

    # Merge environment variables (second priority)
    if env_override:
        for policy_key, env_var in BUDGET_ENV_VARS.items():
            env_val = os.environ.get(env_var)
            if env_val is not None:
                # Convert to appropriate type
                field_type = DEFAULT_POLICY.__dataclass_fields__[policy_key].type
                if field_type is Optional[int] or field_type is int:
                    merged[policy_key] = int(env_val)
                elif field_type is Optional[float] or field_type is float:
                    merged[policy_key] = float(env_val)
                elif field_type is bool:
                    merged[policy_key] = env_val.lower() in ("true", "1", "yes")

    # Merge runtime config (highest priority)
    if runtime_config:
        for key, value in runtime_config.items():
            if key in merged:
                merged[key] = value

    # Reconstruct policy using dataclass replace pattern
    return BudgetPolicy(**merged)


def get_effective_limit(
    policy: BudgetPolicy,
    period: BudgetPeriod,
    metric: str,  # "tokens", "cost", "turns"
) -> Optional[float]:
    """Get the effective limit for a given period and metric.

    Example: get_effective_limit(policy, BudgetPeriod.DAILY, "tokens") → 2000000
    """
    mapping: Dict[tuple, str] = {
        (BudgetPeriod.SESSION, "tokens"): "max_tokens_per_session",
        (BudgetPeriod.DAILY, "tokens"): "max_tokens_per_day",
        (BudgetPeriod.MONTHLY, "tokens"): "max_tokens_per_month",
        (BudgetPeriod.SESSION, "cost"): "max_cost_per_session",
        (BudgetPeriod.DAILY, "cost"): "max_cost_per_day",
        (BudgetPeriod.MONTHLY, "cost"): "max_cost_per_month",
        (BudgetPeriod.SESSION, "turns"): "max_turns_per_session",
    }
    key = mapping.get((period, metric))
    if key is None:
        return None
    return getattr(policy, key, None)


def format_budget_summary(policy: BudgetPolicy) -> str:
    """Format a human-readable summary of a budget policy."""
    parts = []

    # Session limits
    session_parts = []
    if policy.max_tokens_per_session is not None:
        session_parts.append(f"{_format_num(policy.max_tokens_per_session)} tokens")
    if policy.max_cost_per_session is not None:
        session_parts.append(f"${policy.max_cost_per_session:.2f}")
    if policy.max_turns_per_session is not None:
        session_parts.append(f"{policy.max_turns_per_session} turns")
    if session_parts:
        parts.append("Session: " + " / ".join(session_parts))

    # Daily limits
    daily_parts = []
    if policy.max_tokens_per_day is not None:
        daily_parts.append(f"{_format_num(policy.max_tokens_per_day)} tokens")
    if policy.max_cost_per_day is not None:
        daily_parts.append(f"${policy.max_cost_per_day:.2f}")
    if daily_parts:
        parts.append("Daily: " + " / ".join(daily_parts))

    # Monthly limits
    monthly_parts = []
    if policy.max_tokens_per_month is not None:
        monthly_parts.append(f"{_format_num(policy.max_tokens_per_month)} tokens")
    if policy.max_cost_per_month is not None:
        monthly_parts.append(f"${policy.max_cost_per_month:.2f}")
    if monthly_parts:
        parts.append("Monthly: " + " / ".join(monthly_parts))

    # Warning threshold
    parts.append(f"Warning: {policy.warning_pct:.0f}%")

    return " | ".join(parts)


def _format_num(n: int) -> str:
    """Format a large number with K/M suffixes."""
    if n >= 1_000_000:
        return f"{n / 1_000_000:.0f}M"
    elif n >= 1_000:
        return f"{n / 1_000:.0f}K"
    return str(n)
