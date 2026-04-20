"""
Harvey OS — Settings Loader

6-tier precedence settings loading system.
Matches Claude Code's settings precedence pattern.

Precedence (lowest to highest):
1. PLUGIN_BASE - Plugin default settings
2. USER - ~/.harvey/settings.json
3. PROJECT - $PWD/.harvey/settings.json
4. LOCAL - $HARVEY_HOME/.harvey/settings.local.json
5. FLAG - --settings CLI flag
6. POLICY - MDM / enterprise policy (highest)

Path: harvey-os/core/settings/loader.py
"""

from __future__ import annotations

import os
import json
import copy
import platform
from pathlib import Path
from typing import Any, Optional, Dict, List, Callable, Union
from enum import Enum

from .schema import HarveySettings


class SettingSource(Enum):
    """
    Settings source tiers.

    Lower values = lower priority.
    Higher values override lower values.
    """

    PLUGIN_BASE = "plugin_base"  # Lowest: plugin defaults
    USER = "user"  # ~/.harvey/settings.json
    PROJECT = "project"  # $PWD/.harvey/settings.json
    LOCAL = "local"  # $HARVEY_HOME/.harvey/settings.local.json
    FLAG = "flag"  # --settings CLI flag
    POLICY = "policy"  # MDM / enterprise (highest)


SETTINGS_PRECEDENCE = [
    SettingSource.PLUGIN_BASE,
    SettingSource.USER,
    SettingSource.PROJECT,
    SettingSource.LOCAL,
    SettingSource.FLAG,
    SettingSource.POLICY,
]


class SettingsLoader:
    """
    Claude Code's 6-tier settings precedence pattern.

    Each layer is merged in order (later overrides earlier).
    Arrays are concatenated and deduplicated.

    Features:
    - Deep merge with array concatenation
    - Dot-path setting access
    - File watching for change detection
    - Per-source persistence
    """

    def __init__(
        self,
        harvey_home: Optional[Union[Path, str]] = None,
        flag_settings: Optional[Dict[str, Any]] = None,
    ):
        """
        Initialize SettingsLoader.

        Args:
            harvey_home: Harvey home directory (defaults to env/HARVEY_HOME or ~/.HARVEY)
            flag_settings: Settings passed via CLI flag (highest priority)
        """
        if isinstance(harvey_home, str):
            self.harvey_home = Path(harvey_home)
        elif harvey_home is None:
            Harvey_home = os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY"))
            self.harvey_home = Path(Harvey_home)
        else:
            self.harvey_home = harvey_home

        self.flag_settings = flag_settings
        self._cache: Optional[HarveySettings] = None
        self._cache_valid = False
        self._watchers: Dict[str, Callable] = {}
        self._ensure_directories()

    def _ensure_directories(self) -> None:
        """Ensure settings directories exist."""
        self.harvey_home.mkdir(parents=True, exist_ok=True)
        harvey_dir = self.harvey_home / ".harvey"
        harvey_dir.mkdir(parents=True, exist_ok=True)

    def _settings_path(self, source: SettingSource) -> Optional[Path]:
        """
        Get path for settings source.

        Returns None for FLAG (passed in memory) and PLUGIN_BASE (no file).
        """
        harvey_dir = self.harvey_home / ".harvey"

        if source == SettingSource.USER:
            return harvey_dir / "settings.json"
        elif source == SettingSource.PROJECT:
            return Path.cwd() / ".harvey" / "settings.json"
        elif source == SettingSource.LOCAL:
            return harvey_dir / "settings.local.json"
        elif source == SettingSource.FLAG:
            return None  # In-memory
        elif source == SettingSource.POLICY:
            # MDM / enterprise
            if platform.system() == "Darwin":
                return Path("/Library/Managed Preferences/harvey.json")
            elif platform.system() == "Windows":
                return None  # Registry
            else:
                return Path("/etc/harvey/managed-settings.json")
        elif source == SettingSource.PLUGIN_BASE:
            return None  # No file - plugins provide defaults in code
        return None

    def _load_from_source(self, source: SettingSource) -> Optional[Dict[str, Any]]:
        """
        Load settings dict from a single source.

        Returns None if source doesn't exist or can't be read.
        """
        # Handle FLAG source (in-memory)
        if source == SettingSource.FLAG:
            return self.flag_settings

        path = self._settings_path(source)
        if not path or not path.exists():
            return None

        try:
            data = json.loads(path.read_text())
            return data
        except (json.JSONDecodeError, OSError):
            return None

    def load_settings(self) -> HarveySettings:
        """
        Load settings with 6-tier precedence.

        Later sources override earlier ones.
        Arrays are concatenated.

        Returns:
            Fully merged HarveySettings instance
        """
        if self._cache_valid and self._cache is not None:
            return self._cache

        # Deep merge in precedence order
        merged: Dict[str, Any] = {}

        for source in SETTINGS_PRECEDENCE:
            data = self._load_from_source(source)
            if data:
                merged = self._deep_merge(merged, data)

        # Apply plugin base defaults for any missing keys
        plugin_defaults = self._get_plugin_defaults()
        merged = self._deep_merge(plugin_defaults, merged)

        try:
            self._cache = HarveySettings(**merged)
            self._cache_valid = True
        except (TypeError, ValueError):
            # Fall back to defaults if merge fails
            self._cache = HarveySettings()
            self._cache_valid = True

        return self._cache

    def _get_plugin_defaults(self) -> Dict[str, Any]:
        """
        Get plugin base defaults.

        This is where plugins would register their default settings.
        Currently returns empty dict - plugins override via PLUGIN_BASE file.
        """
        plugin_base_path = self.harvey_home / ".harvey" / "settings.plugin_base.json"
        if plugin_base_path.exists():
            try:
                return json.loads(plugin_base_path.read_text())
            except (json.JSONDecodeError, OSError):
                pass
        return {}

    def _deep_merge(
        self, target: Dict[str, Any], source: Dict[str, Any]
    ) -> Dict[str, Any]:
        """
        Deep merge with array concatenation (Claude Code pattern).

        - Dicts are merged recursively
        - Lists are concatenated and deduplicated
        - None values don't override existing values
        - Later values override earlier values
        """
        result = copy.deepcopy(target)

        for key, value in source.items():
            if key in result:
                if isinstance(result[key], list) and isinstance(value, list):
                    # Concatenate + dedupe preserving order
                    seen = set()
                    concatenated = []
                    for item in result[key] + value:
                        if isinstance(item, str):
                            if item not in seen:
                                seen.add(item)
                                concatenated.append(item)
                        else:
                            concatenated.append(item)
                    result[key] = concatenated
                elif isinstance(result[key], dict) and isinstance(value, dict):
                    result[key] = self._deep_merge(result[key], value)
                elif value is not None:
                    result[key] = value
            else:
                if value is not None:
                    result[key] = value

        return result

    def get_effective_setting(self, key_path: str) -> Any:
        """
        Get a single setting by dot-path.

        Example: loader.get_effective_setting('model.temperature')

        Args:
            key_path: Dot-separated path to setting (e.g., 'model.temperature')

        Returns:
            Setting value or None if not found
        """
        settings = self.load_settings()
        keys = key_path.split(".")
        value = settings.model_dump()

        for k in keys:
            if isinstance(value, dict):
                value = value.get(k)
            else:
                return None

        return value

    def update_setting(self, source: SettingSource, key: str, value: Any) -> None:
        """
        Update a setting in a specific source.

        Args:
            source: Which settings file to update
            key: Dot-separated key path (e.g., 'model.temperature')
            value: New value
        """
        path = self._settings_path(source)
        if not path:
            return  # FLAG source doesn't persist

        # Load existing data
        if path.exists():
            try:
                data = json.loads(path.read_text())
            except (json.JSONDecodeError, OSError):
                data = {}
        else:
            data = {}
            path.parent.mkdir(parents=True, exist_ok=True)

        # Dot-path update
        keys = key.split(".")
        current = data
        for k in keys[:-1]:
            if k not in current:
                current[k] = {}
            elif not isinstance(current[k], dict):
                current[k] = {}
            current = current[k]
        current[keys[-1]] = value

        # Write back atomically
        tmp_path = path.with_suffix(".json.tmp")
        tmp_path.write_text(json.dumps(data, indent=2))
        tmp_path.rename(path)

        # Invalidate cache
        self._cache_valid = False

    def invalidate_cache(self) -> None:
        """Invalidate the settings cache."""
        self._cache_valid = False
        self._cache = None


# Module-level singleton
_loader: Optional[SettingsLoader] = None


def get_settings_loader() -> SettingsLoader:
    """Get the global SettingsLoader instance."""
    global _loader
    if _loader is None:
        _loader = SettingsLoader()
    return _loader


def load_settings() -> HarveySettings:
    """Convenience function to load settings."""
    return get_settings_loader().load_settings()
