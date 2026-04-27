#!/usr/bin/env python3
"""
Shell State Persistence — Persistent cwd and env vars across bash invocations.

Inspired by claurst's ShellState pattern. When agents spawn bash commands,
the working directory and environment variables are lost between invocations.
This module wraps commands to capture and restore shell state.

The trick: append a sentinel + env dump after the user's command.
Parse the sentinel block to extract (cwd, env_delta).
Next invocation starts with the restored state.

Usage:
    from core.shell.shell_state import ShellStateManager

    mgr = ShellStateManager(session_id="abc123")

    # First command — sets up state
    wrapped = mgr.wrap_command("cd /tmp && export FOO=bar")
    output, exit_code = run_bash(wrapped)
    clean_output = mgr.parse_output(output)

    # Second command — state is restored
    wrapped = mgr.wrap_command("echo $FOO && pwd")
    # This will cd /tmp, set FOO=bar, THEN run echo/pwd
"""

import json
import logging
import os
import re
import threading
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Optional

log = logging.getLogger("harvey.shell")

SENTINEL = "__HARVEY_SHELL_STATE_7f3a__"


@dataclass
class ShellState:
    """Captured shell state from a command execution."""
    cwd: str = ""
    env_vars: Dict[str, str] = field(default_factory=dict)
    last_exit_code: int = 0


class ShellStateManager:
    """
    Manages persistent shell state across bash invocations.

    Thread-safe: uses lock for state access.
    Session-scoped: each session_id gets independent state.
    """

    # Global registry of all session states
    _registry: Dict[str, "ShellStateManager"] = {}
    _registry_lock = threading.Lock()

    def __init__(self, session_id: str = "default", initial_cwd: str = None):
        self.session_id = session_id
        self.state = ShellState(
            cwd=initial_cwd or os.getcwd(),
        )
        self._lock = threading.Lock()

    @classmethod
    def for_session(cls, session_id: str) -> "ShellStateManager":
        """Get or create a ShellStateManager for a session."""
        with cls._registry_lock:
            if session_id not in cls._registry:
                cls._registry[session_id] = cls(session_id=session_id)
            return cls._registry[session_id]

    def wrap_command(self, command: str) -> str:
        """
        Wrap a command to capture shell state after execution.

        Prepends: cd to current cwd, export saved env vars
        Appends: sentinel + pwd + env dump
        """
        parts = []

        # Restore previous state
        with self._lock:
            if self.state.cwd:
                parts.append(f'cd {_shell_quote(self.state.cwd)} 2>/dev/null')
            for key, val in self.state.env_vars.items():
                parts.append(f'export {key}={_shell_quote(val)}')

        # User's command
        parts.append(command)

        # Capture state after command
        parts.append(f'__harvey_ec=$?')
        parts.append(f'echo ""')
        parts.append(f'echo "{SENTINEL}"')
        parts.append(f'echo "CWD=$(pwd)"')
        parts.append(f'echo "EXIT=$__harvey_ec"')
        # Dump env vars that differ from system defaults
        parts.append(f'env | sort')
        parts.append(f'echo "{SENTINEL}_END"')
        parts.append(f'exit $__harvey_ec')

        return " && ".join(parts[:len(parts)-4]) + "\n" + "\n".join(parts[len(parts)-4:])

    def parse_output(self, raw_output: str) -> str:
        """
        Parse command output, extract shell state, return clean output.

        Strips the sentinel block from output.
        Updates internal state with captured cwd/env.
        Returns only the user-visible output.
        """
        sentinel_start = raw_output.find(SENTINEL)
        if sentinel_start == -1:
            return raw_output

        # Clean output = everything before sentinel
        clean = raw_output[:sentinel_start].rstrip()

        # Parse state block
        sentinel_end = raw_output.find(f"{SENTINEL}_END")
        if sentinel_end == -1:
            return clean

        state_block = raw_output[sentinel_start + len(SENTINEL):sentinel_end].strip()

        with self._lock:
            for line in state_block.split("\n"):
                line = line.strip()
                if line.startswith("CWD="):
                    self.state.cwd = line[4:]
                elif line.startswith("EXIT="):
                    try:
                        self.state.last_exit_code = int(line[5:])
                    except ValueError:
                        pass
                elif "=" in line and not line.startswith("_"):
                    # Env var — only track non-default ones
                    key, _, val = line.partition("=")
                    if key in _TRACKED_ENV_VARS or key.startswith("HARVEY_"):
                        self.state.env_vars[key] = val

        return clean

    def get_cwd(self) -> str:
        """Get current working directory."""
        with self._lock:
            return self.state.cwd

    def get_env(self, key: str) -> Optional[str]:
        """Get an environment variable."""
        with self._lock:
            return self.state.env_vars.get(key)

    def set_env(self, key: str, value: str):
        """Manually set an environment variable."""
        with self._lock:
            self.state.env_vars[key] = value

    def to_dict(self) -> dict:
        """Serialize state for persistence."""
        with self._lock:
            return {
                "session_id": self.session_id,
                "cwd": self.state.cwd,
                "env_vars": dict(self.state.env_vars),
                "last_exit_code": self.state.last_exit_code,
            }


# Environment variables worth tracking across invocations
_TRACKED_ENV_VARS = {
    "PATH", "PYTHONPATH", "NODE_PATH", "GOPATH",
    "VIRTUAL_ENV", "CONDA_DEFAULT_ENV",
    "AWS_PROFILE", "AWS_REGION",
    "DOCKER_HOST", "KUBECONFIG",
    "GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL",
    "HARVEY_HOME", "SWITCHAI_KEY", "LLM_BASE_URL",
}


def _shell_quote(s: str) -> str:
    """Quote a string for shell safety."""
    if not s:
        return "''"
    # Use single quotes, escape embedded single quotes
    return "'" + s.replace("'", "'\\''") + "'"
