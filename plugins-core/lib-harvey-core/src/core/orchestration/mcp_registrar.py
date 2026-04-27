"""
mcp_registrar — Phase 3 of harvey:infect.

Registers the existing harvey_mcp.py MCP server with whichever host CLI was
detected in Phase 1, idempotently and reversibly. The MCP plugin
(`plugins-core/lib-harvey-core/src/core/mcp/harvey_mcp.py`) is what gives the host its Harvey
muscles; this module is the installer that wires it in.

Per-host configuration matrix (verified in v2.1 sprint):

  | Host         | Config path                          | Top key       |
  |--------------|--------------------------------------|---------------|
  | claude-code  | ~/.claude/settings.json              | mcpServers    |
  | opencode     | ~/.opencode/config.json              | mcpServers    |
  | crush        | ~/.config/crush/crush.json           | mcp           |  ← different
  | gemini-cli   | ~/.gemini/settings.json              | mcpServers    |
  | codex        | (no MCP support today)               | —             |

Crush uses `mcp` instead of `mcpServers` — verified against the
charmbracelet/crush repository's config docs.

All config files are JSON. The registrar reads, deeply merges in the
harvey-mcp entry, and writes atomically (temp file + rename). Idempotent:
re-running detects an existing harvey-mcp entry pointing at the same path
and returns `already-registered` without writing.

Reversibility: `unregister(host_info)` removes only the harvey-mcp entry,
preserving every other server. Exit rights are non-negotiable.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Optional

from .host_detector import HostInfo, HostType


SERVER_NAME = "harvey-mcp"


class RegistrationStatus(Enum):
    REGISTERED = "registered"
    ALREADY_REGISTERED = "already-registered"
    UPDATED = "updated"  # existed but pointed at a different path
    UNSUPPORTED = "unsupported"  # host has no MCP support
    ERROR = "error"


@dataclass
class RegistrationResult:
    status: RegistrationStatus
    host: str
    config_path: Optional[Path] = None
    config_key: Optional[str] = None
    server_command: Optional[list[str]] = None
    error: str = ""


# ─── Per-host config descriptor ─────────────────────────────────


@dataclass
class HostConfig:
    """Where and how to write a host's MCP config."""
    path: Path
    top_key: str  # 'mcpServers' for most, 'mcp' for crush


def _config_for(host: HostType, home: Optional[Path] = None) -> Optional[HostConfig]:
    """Return the HostConfig for a given host, or None for unsupported."""
    home = home or Path(os.path.expanduser("~"))
    table = {
        HostType.CLAUDE_CODE: HostConfig(
            path=home / ".claude" / "settings.json",
            top_key="mcpServers",
        ),
        HostType.OPENCODE: HostConfig(
            path=home / ".opencode" / "config.json",
            top_key="mcpServers",
        ),
        HostType.CRUSH: HostConfig(
            path=home / ".config" / "crush" / "crush.json",
            top_key="mcp",  # verified against charmbracelet/crush
        ),
        HostType.GEMINI_CLI: HostConfig(
            path=home / ".gemini" / "settings.json",
            top_key="mcpServers",
        ),
        HostType.CODEX: None,  # no MCP support
    }
    return table.get(host)


# ─── Public API ─────────────────────────────────────────────────


class MCPRegistrar:
    """Installs and removes the harvey-mcp server entry in host config files."""

    def __init__(
        self,
        harvey_home: Optional[str] = None,
        home: Optional[str] = None,
    ):
        self.harvey_home = Path(
            harvey_home or os.environ.get("HARVEY_HOME") or os.path.expanduser("~/MAKAKOO")
        ).resolve()
        self.home = Path(home or os.path.expanduser("~"))

    def register(self, host_info: HostInfo) -> RegistrationResult:
        """Register harvey-mcp on the given host. Idempotent + reversible."""
        cfg = _config_for(host_info.name, home=self.home)
        if cfg is None:
            return RegistrationResult(
                status=RegistrationStatus.UNSUPPORTED,
                host=host_info.name.value,
                error=f"{host_info.name.value} has no MCP support",
            )

        server_entry = self._build_server_entry()

        try:
            existing = self._read_config(cfg.path)
        except json.JSONDecodeError as e:
            return RegistrationResult(
                status=RegistrationStatus.ERROR,
                host=host_info.name.value,
                config_path=cfg.path,
                config_key=cfg.top_key,
                error=f"existing config is not valid JSON: {e}",
            )

        servers = existing.setdefault(cfg.top_key, {})
        if not isinstance(servers, dict):
            return RegistrationResult(
                status=RegistrationStatus.ERROR,
                host=host_info.name.value,
                config_path=cfg.path,
                config_key=cfg.top_key,
                error=f"{cfg.top_key!r} in config is not a dict",
            )

        prior = servers.get(SERVER_NAME)
        if prior is not None:
            if prior == server_entry:
                return RegistrationResult(
                    status=RegistrationStatus.ALREADY_REGISTERED,
                    host=host_info.name.value,
                    config_path=cfg.path,
                    config_key=cfg.top_key,
                    server_command=server_entry.get("args"),
                )
            # Existing entry differs — update it
            servers[SERVER_NAME] = server_entry
            self._atomic_write(cfg.path, existing)
            return RegistrationResult(
                status=RegistrationStatus.UPDATED,
                host=host_info.name.value,
                config_path=cfg.path,
                config_key=cfg.top_key,
                server_command=server_entry.get("args"),
            )

        servers[SERVER_NAME] = server_entry
        self._atomic_write(cfg.path, existing)
        return RegistrationResult(
            status=RegistrationStatus.REGISTERED,
            host=host_info.name.value,
            config_path=cfg.path,
            config_key=cfg.top_key,
            server_command=server_entry.get("args"),
        )

    def unregister(self, host_info: HostInfo) -> RegistrationResult:
        """Remove the harvey-mcp entry from this host's config. No-op if absent.

        Touches ONLY the harvey-mcp entry — every other server is preserved.
        """
        cfg = _config_for(host_info.name, home=self.home)
        if cfg is None:
            return RegistrationResult(
                status=RegistrationStatus.UNSUPPORTED,
                host=host_info.name.value,
            )

        if not cfg.path.exists():
            return RegistrationResult(
                status=RegistrationStatus.ALREADY_REGISTERED,  # nothing to do
                host=host_info.name.value,
                config_path=cfg.path,
                config_key=cfg.top_key,
            )

        try:
            existing = self._read_config(cfg.path)
        except json.JSONDecodeError as e:
            return RegistrationResult(
                status=RegistrationStatus.ERROR,
                host=host_info.name.value,
                config_path=cfg.path,
                error=f"existing config is not valid JSON: {e}",
            )

        servers = existing.get(cfg.top_key, {})
        if isinstance(servers, dict) and SERVER_NAME in servers:
            del servers[SERVER_NAME]
            # Drop the top key entirely if it's now empty
            if not servers:
                existing.pop(cfg.top_key, None)
            self._atomic_write(cfg.path, existing)

        return RegistrationResult(
            status=RegistrationStatus.REGISTERED,  # absence confirmed
            host=host_info.name.value,
            config_path=cfg.path,
            config_key=cfg.top_key,
        )

    def is_registered(self, host_info: HostInfo) -> bool:
        """Read-only check: is harvey-mcp present in this host's config?"""
        cfg = _config_for(host_info.name, home=self.home)
        if cfg is None or not cfg.path.exists():
            return False
        try:
            existing = self._read_config(cfg.path)
        except json.JSONDecodeError:
            return False
        servers = existing.get(cfg.top_key, {})
        return isinstance(servers, dict) and SERVER_NAME in servers

    # ─── Internals ─────────────────────────────────────────

    def _build_server_entry(self) -> dict:
        """Build the harvey-mcp server entry that all hosts get installed.

        Standard MCP server JSON format: command + args + optional env.
        Points at python3 + harvey_mcp.py with HARVEY_HOME pinned in env.
        """
        mcp_path = self.harvey_home / "plugins-core" / "lib-harvey-core" / "src" / "core" / "mcp" / "harvey_mcp.py"
        return {
            "command": sys.executable or "python3",
            "args": [str(mcp_path)],
            "env": {
                "HARVEY_HOME": str(self.harvey_home),
            },
        }

    def _read_config(self, path: Path) -> dict:
        """Read a JSON config file. Returns empty dict if file is missing.

        Raises json.JSONDecodeError on malformed JSON (caller wraps as
        RegistrationStatus.ERROR).
        """
        if not path.exists():
            return {}
        text = path.read_text(encoding="utf-8")
        if not text.strip():
            return {}
        return json.loads(text)

    def _atomic_write(self, path: Path, data: dict) -> None:
        """Write JSON atomically: write to temp + rename."""
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(path.suffix + ".tmp")
        tmp.write_text(
            json.dumps(data, indent=2, sort_keys=False) + "\n",
            encoding="utf-8",
        )
        os.replace(tmp, path)
