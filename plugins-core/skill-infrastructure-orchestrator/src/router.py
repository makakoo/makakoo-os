"""
Router for model selection based on task characteristics.
Loads routing rules from config and applies them to select model + endpoint.
"""

import json
import os
from pathlib import Path
from typing import Optional


class Router:
    """
    Routes tasks to appropriate models based on routing rules.
    Default: minimax:M2 via localhost:18080
    """

    DEFAULT_MODEL = "minimax:M2"
    DEFAULT_ENDPOINT = "http://localhost:18080"

    def __init__(
        self,
        config_path: str = None,
    ):
        if config_path is None:
            config_path = str(Path(__file__).parent / "config" / "routing_rules.json")
        self.config_path = Path(config_path)
        self.rules: list[dict] = []
        self.default: dict = {}
        self._load_config()

    def _load_config(self) -> None:
        """Load routing rules from config file."""
        if self.config_path.exists():
            try:
                data = json.loads(self.config_path.read_text())
                self.rules = data.get("rules", [])
                self.default = data.get("default", {})
            except json.JSONDecodeError:
                self._use_defaults()
        else:
            self._use_defaults()

    def _use_defaults(self) -> None:
        """Fall back to hardcoded defaults."""
        self.default = {
            "model": self.DEFAULT_MODEL,
            "endpoint": self.DEFAULT_ENDPOINT,
        }
        self.rules = [
            {
                "match": {"complexity": "low"},
                "model": "minimax:M2",
                "endpoint": "http://localhost:18080",
            },
            {
                "match": {"complexity": "high"},
                "model": "claude-sonnet-4-20250514",
                "endpoint": "anthropic",
            },
        ]

    def route(self, task: dict) -> tuple[str, str]:
        """
        Route a task to the appropriate model and endpoint.
        Returns (model, endpoint) tuple.

        Matches rules in order; first match wins.
        """
        for rule in self.rules:
            match_spec = rule.get("match", {})
            if self._matches(task, match_spec):
                return rule.get("model", self.DEFAULT_MODEL), rule.get(
                    "endpoint", self.DEFAULT_ENDPOINT
                )

        return self.default.get("model", self.DEFAULT_MODEL), self.default.get(
            "endpoint", self.DEFAULT_ENDPOINT
        )

    def _matches(self, task: dict, spec: dict) -> bool:
        """
        Check if task matches a rule specification.
        Supports exact match and wildcard (*) for any value.
        """
        for key, expected in spec.items():
            if key == "complexity":
                # Check payload.complexity or top-level complexity
                task_complexity = (
                    task.get("payload", {}).get("complexity")
                    or task.get("complexity")
                )
                if task_complexity != expected:
                    return False
            elif key == "agent_type":
                if task.get("agent_type") != expected:
                    return False
            elif key == "description_contains":
                if expected not in task.get("description", ""):
                    return False
            else:
                # Exact match on task key
                if task.get(key) != expected:
                    return False
        return True

    def reload(self) -> None:
        """Reload config from disk."""
        self._load_config()
