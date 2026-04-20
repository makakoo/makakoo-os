"""
Harvey OS — Feature Flags

GrowthBook-style feature flags with percentage rollouts.
Matches Claude Code's experiment/flag system.

Features:
- Built-in flags for Harvey features
- Percentage rollouts (e.g., {"pct": 20} = first 20% of users)
- User targeting (e.g., {"users": ["sebastian"]})
- Dev convenience methods (enable/disable)
- Persistence to ~/.harvey/feature_flags.json

Path: harvey-os/core/telemetry/feature_flags.py
"""

from __future__ import annotations

import os
import json
import hashlib
import time
import threading
from pathlib import Path
from dataclasses import dataclass, field
from typing import Any
from enum import Enum


@dataclass
class FeatureFlag:
    """
    A feature flag definition.

    Supports:
    - Default value (used when flag is disabled/unknown)
    - Description for documentation
    - Rollout rules (percentage, users)
    """

    name: str
    default_value: bool = False
    description: str = ""
    gate: dict[str, Any] = field(default_factory=dict)  # rollout rule

    def is_enabled_for_user(self, user_id: str | None = None) -> bool:
        """
        Check if flag is enabled for a user.

        Args:
            user_id: User identifier for targeting

        Returns:
            True if flag should be enabled
        """
        if not self.gate:
            return self.default_value

        # Check user targeting first
        if "users" in self.gate:
            users = self.gate["users"]
            if user_id and user_id in users:
                return True
            # Default for users not in list: use default_value
            return self.default_value

        # Check percentage rollout
        if "pct" in self.gate:
            pct = self.gate["pct"]
            if user_id:
                # Hash user_id to get deterministic percentage
                hash_val = int(
                    hashlib.md5(f"{self.name}:{user_id}".encode()).hexdigest(), 16
                )
                bucket = (hash_val % 100) + 1  # 1-100
                return bucket <= pct
            else:
                # No user_id: use time-based percentage (changes over time)
                # This is intentional for canary deployments
                bucket = (int(time.time()) % 100) + 1
                return bucket <= pct

        return self.default_value


class FeatureFlagRegistry:
    """
    GrowthBook-style feature flag registry.

    Features:
    - Built-in flags for Harvey core features
    - Percentage rollouts for gradual rollout
    - User targeting for specific users
    - Dev convenience methods
    - Persistence to JSON file

    BUILT-IN FLAGS:
    - harvey_auto_memory_extraction: Auto-extract memories after sessions
    - harvey_llm_memory_selection: Use LLM for memory selection (not vectors)
    - harvey_new_dispatcher: New async dispatcher implementation
    - harvey_mcp_oauth: MCP OAuth2/PKCE authentication
    - harvey_analytics: Telemetry and analytics collection
    - harvey_coordinator: Multi-agent coordinator
    - harvey_team_memory: Team-shared memory
    - harvey_deferred_prefetch: Deferred I/O prefetching
    - harvey_mdm_prefire: MDM settings prefiring at startup
    - harvey_keychain_auth: macOS Keychain credential storage
    """

    BUILT_IN_FLAGS: dict[str, FeatureFlag] = {
        "harvey_auto_memory_extraction": FeatureFlag(
            name="harvey_auto_memory_extraction",
            default_value=False,
            description="Auto-extract memories after sessions",
        ),
        "harvey_llm_memory_selection": FeatureFlag(
            name="harvey_llm_memory_selection",
            default_value=False,
            description="Use LLM for memory selection instead of vector search",
        ),
        "harvey_new_dispatcher": FeatureFlag(
            name="harvey_new_dispatcher",
            default_value=False,
            description="New async dispatcher with agent lifecycle",
        ),
        "harvey_mcp_oauth": FeatureFlag(
            name="harvey_mcp_oauth",
            default_value=False,
            description="MCP OAuth2/PKCE authentication",
        ),
        "harvey_analytics": FeatureFlag(
            name="harvey_analytics",
            default_value=True,  # Analytics ON by default
            description="Telemetry and analytics collection",
        ),
        "harvey_coordinator": FeatureFlag(
            name="harvey_coordinator",
            default_value=False,
            description="Multi-agent coordinator",
        ),
        "harvey_team_memory": FeatureFlag(
            name="harvey_team_memory",
            default_value=False,
            description="Team-shared memory across agents",
        ),
        "harvey_deferred_prefetch": FeatureFlag(
            name="harvey_deferred_prefetch",
            default_value=False,
            description="Deferred I/O prefetching at startup",
        ),
        "harvey_mdm_prefire": FeatureFlag(
            name="harvey_mdm_prefire",
            default_value=False,
            description="MDM settings prefiring at startup",
        ),
        "harvey_keychain_auth": FeatureFlag(
            name="harvey_keychain_auth",
            default_value=False,
            description="macOS Keychain credential storage",
        ),
        "harvey_sparse_checkout": FeatureFlag(
            name="harvey_sparse_checkout",
            default_value=False,
            description="Git sparse checkout for large repos",
        ),
        "harvey_lru_file_cache": FeatureFlag(
            name="harvey_lru_file_cache",
            default_value=True,  # LRU cache ON by default
            description="LRU cache for file reads",
        ),
    }

    def __init__(self, flags_dir: Path | None = None):
        """
        Initialize FeatureFlagRegistry.

        Args:
            flags_dir: Directory for flag override files
        """
        Harvey_home = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
        self.flags_dir = flags_dir or Path(Harvey_home) / ".harvey"
        self._flags_file = self.flags_dir / "feature_flags.json"

        self._flags: dict[str, FeatureFlag] = {}
        self._overrides: dict[str, bool | None] = {}  # None = use built-in
        self._lock = threading.RLock()

        # Initialize with built-in flags
        for name, flag in self.BUILT_IN_FLAGS.items():
            self._flags[name] = flag

        # Load overrides from file
        self._load_overrides()

    def _load_overrides(self) -> None:
        """Load flag overrides from JSON file."""
        if not self._flags_file.exists():
            return

        try:
            data = json.loads(self._flags_file.read_text())

            # Load enabled/disabled overrides
            if "overrides" in data:
                for name, override in data["overrides"].items():
                    if override is None:
                        self._overrides[name] = None  # Reset to default
                    elif isinstance(override, bool):
                        self._overrides[name] = override

            # Load gate overrides (for percentage rollouts, etc.)
            if "gates" in data:
                for name, gate in data["gates"].items():
                    if name in self._flags:
                        self._flags[name].gate = gate
                    else:
                        # Create new flag with custom gate
                        self._flags[name] = FeatureFlag(
                            name=name, default_value=False, gate=gate
                        )

        except (json.JSONDecodeError, OSError):
            pass

    def _save_overrides(self) -> None:
        """Save flag overrides to JSON file."""
        data = {"overrides": {}, "gates": {}}

        for name, override in self._overrides.items():
            if override is not None:
                data["overrides"][name] = override

        for name, flag in self._flags.items():
            if flag.gate and name not in self.BUILT_IN_FLAGS:
                data["gates"][name] = flag.gate

        self._flags_file.parent.mkdir(parents=True, exist_ok=True)

        # Atomic write
        tmp_path = self._flags_file.with_suffix(".json.tmp")
        tmp_path.write_text(json.dumps(data, indent=2))
        tmp_path.rename(self._flags_file)

    def is_enabled(self, flag_name: str, user_id: str | None = None) -> bool:
        """
        Check if a feature flag is enabled.

        Args:
            flag_name: Name of the flag
            user_id: User identifier for targeting

        Returns:
            True if flag is enabled for the user
        """
        with self._lock:
            # Check override first
            if flag_name in self._overrides:
                override = self._overrides[flag_name]
                if override is not None:
                    return override

            # Check built-in or custom flag
            if flag_name in self._flags:
                return self._flags[flag_name].is_enabled_for_user(user_id)

            # Unknown flags default to False
            return False

    def enable(self, flag_name: str) -> None:
        """
        Enable a flag (dev convenience).

        Args:
            flag_name: Name of the flag to enable
        """
        with self._lock:
            self._overrides[flag_name] = True
            self._save_overrides()

    def disable(self, flag_name: str) -> None:
        """
        Disable a flag (dev convenience).

        Args:
            flag_name: Name of the flag to disable
        """
        with self._lock:
            self._overrides[flag_name] = False
            self._save_overrides()

    def reset(self, flag_name: str) -> None:
        """
        Reset a flag to its default (built-in) value.

        Args:
            flag_name: Name of the flag to reset
        """
        with self._lock:
            if flag_name in self._overrides:
                del self._overrides[flag_name]
            self._save_overrides()

    def set_gate(self, flag_name: str, gate: dict[str, Any]) -> None:
        """
        Set a rollout gate for a flag.

        Args:
            flag_name: Name of the flag
            gate: Rollout rule (e.g., {"pct": 20} or {"users": ["sebastian"]})
        """
        with self._lock:
            if flag_name in self._flags:
                self._flags[flag_name].gate = gate
            else:
                self._flags[flag_name] = FeatureFlag(
                    name=flag_name, default_value=False, gate=gate
                )
            self._save_overrides()

    def list_flags(self) -> list[dict]:
        """
        List all flags with their current state.

        Returns:
            List of flag info dicts
        """
        with self._lock:
            result = []
            for name, flag in self._flags.items():
                override = self._overrides.get(name)
                result.append(
                    {
                        "name": name,
                        "default": flag.default_value,
                        "enabled": self.is_enabled(name),
                        "override": override,
                        "description": flag.description,
                        "gate": flag.gate if flag.gate else None,
                    }
                )
            return result

    def get_flag(self, flag_name: str) -> FeatureFlag | None:
        """Get a flag definition."""
        return self._flags.get(flag_name)


# Module-level singleton
_registry: FeatureFlagRegistry | None = None


def get_flag_registry() -> FeatureFlagRegistry:
    """Get the global FeatureFlagRegistry instance."""
    global _registry
    if _registry is None:
        _registry = FeatureFlagRegistry()
    return _registry


def is_feature_enabled(flag_name: str, user_id: str | None = None) -> bool:
    """Convenience function to check if a flag is enabled."""
    return get_flag_registry().is_enabled(flag_name, user_id)
