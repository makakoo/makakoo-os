#!/usr/bin/env python3
"""
Harvey MCP Auto-Setup — Detect all installed CLIs and configure Harvey.

One command to connect Harvey to every CLI on the machine:
    superbrain setup

Auto-detects: Claude Code, OpenCode, Gemini CLI, Codex
Configures each one to use harvey-mcp as a tool server.

Also: superbrain setup --check (verify connections)
"""

import json
import os
import shutil
import subprocess
from pathlib import Path

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
MCP_SCRIPT = os.path.join(HARVEY_HOME, "harvey-os", "core", "mcp", "harvey_mcp.py")
PYTHONPATH_DIR = os.path.join(HARVEY_HOME, "harvey-os")

# Env vars passed to every CLI registration so Harvey works for users
# whose clone lives at a non-default location. Without these the MCP
# server falls back to ~/MAKAKOO which only works on Sebastian's box.
_HARVEY_ENV = {
    "HARVEY_HOME": HARVEY_HOME,
    "PYTHONPATH": PYTHONPATH_DIR,
}


def detect_clis() -> list:
    """Detect all installed MCP-compatible CLIs."""
    clis = []

    # CLI-specific env-var passing syntax (per `<cli> mcp add --help`):
    #   Claude Code:  -e KEY=VALUE -e KEY2=VALUE2
    #   Gemini CLI:   -e KEY=VALUE -e KEY2=VALUE2
    #   Codex:        --env KEY=VALUE --env KEY2=VALUE2
    #   OpenCode:     "environment" object inside the config block
    claude_env = " ".join(f"-e {k}={v}" for k, v in _HARVEY_ENV.items())
    gemini_env = " ".join(f"-e {k}={v}" for k, v in _HARVEY_ENV.items())
    codex_env = " ".join(f"--env {k}={v}" for k, v in _HARVEY_ENV.items())

    checks = [
        {
            "name": "Claude Code",
            "binary": "claude",
            "install_cmd": f'claude mcp add harvey {claude_env} -- python3 {MCP_SCRIPT}',
            "check_cmd": "claude mcp list",
            "method": "cli",
        },
        {
            "name": "OpenCode",
            "binary": "opencode",
            "install_cmd": None,  # Config file based
            "check_cmd": "opencode mcp",
            "method": "config",
            "config_path": os.path.expanduser("~/.config/opencode/opencode.json"),
        },
        {
            "name": "Gemini CLI",
            "binary": "gemini",
            "install_cmd": f'gemini mcp add harvey -s user {gemini_env} python3 {MCP_SCRIPT}',
            "check_cmd": "gemini mcp list",
            "method": "cli",
        },
        {
            "name": "Codex",
            "binary": "codex",
            "install_cmd": f'codex mcp add harvey {codex_env} -- python3 {MCP_SCRIPT}',
            "check_cmd": "codex mcp list",
            "method": "cli",
        },
    ]

    for cli in checks:
        binary_path = shutil.which(cli["binary"])
        if binary_path:
            # Get version
            try:
                result = subprocess.run(
                    [cli["binary"], "--version"],
                    capture_output=True, text=True, timeout=5,
                )
                version = result.stdout.strip().split("\n")[0]
            except Exception:
                version = "unknown"

            clis.append({
                **cli,
                "installed": True,
                "binary_path": binary_path,
                "version": version,
            })

    return clis


def install_harvey_mcp(cli: dict) -> dict:
    """Install Harvey MCP server into a specific CLI.

    Idempotent: if `harvey` is already registered, remove it first then
    re-add. This is what makes setup_mcp.py safe to re-run after every
    upgrade — without it, `claude mcp add` and friends fail with
    "already exists" and the user thinks the upgrade failed.
    """
    name = cli["name"]

    if cli["method"] == "cli" and cli.get("install_cmd"):
        # Best-effort idempotency: try to remove an existing registration
        # before adding. Per-CLI quirks:
        #   - Claude Code: needs `-s local` to disambiguate from the
        #     `project` scope (the plugin manifest at .mcp.json) which
        #     we don't want to touch
        #   - Gemini CLI: also has scopes; same flag
        #   - Codex: single scope, plain remove
        # Failures here are silent — the install attempt below will
        # surface the real error if there is one.
        binary = cli["binary"]
        remove_cmd = f"{binary} mcp remove harvey"
        if binary in ("claude", "gemini"):
            remove_cmd = f"{binary} mcp remove harvey -s local" if binary == "claude" else f"{binary} mcp remove harvey -s user"
        try:
            subprocess.run(
                remove_cmd,
                shell=True, capture_output=True, text=True, timeout=10,
            )
        except Exception:
            pass

        try:
            result = subprocess.run(
                cli["install_cmd"], shell=True,
                capture_output=True, text=True, timeout=30,
            )
            if result.returncode == 0:
                return {"cli": name, "status": "installed", "output": result.stdout.strip()}
            else:
                return {"cli": name, "status": "failed", "error": result.stderr.strip()}
        except Exception as e:
            return {"cli": name, "status": "error", "error": str(e)}

    elif cli["method"] == "config":
        config_path = cli.get("config_path", "")
        if not config_path:
            return {"cli": name, "status": "skipped", "error": "No config path"}

        try:
            config = {}
            if os.path.exists(config_path):
                with open(config_path) as f:
                    config = json.load(f)

            config.setdefault("mcp", {})["harvey"] = {
                "type": "local",
                "command": ["python3", MCP_SCRIPT],
                "enabled": True,
                "environment": dict(_HARVEY_ENV),
            }

            os.makedirs(os.path.dirname(config_path), exist_ok=True)
            with open(config_path, "w") as f:
                json.dump(config, f, indent=2)

            return {"cli": name, "status": "installed", "output": f"Added to {config_path}"}
        except Exception as e:
            return {"cli": name, "status": "error", "error": str(e)}

    return {"cli": name, "status": "skipped", "error": "Unknown install method"}


def setup_all() -> list:
    """Detect all CLIs and install Harvey MCP in each."""
    clis = detect_clis()
    results = []

    if not clis:
        return [{"cli": "none", "status": "error",
                 "error": "No MCP-compatible CLIs found. Install Claude Code, OpenCode, Gemini CLI, or Codex."}]

    for cli in clis:
        result = install_harvey_mcp(cli)
        result["version"] = cli.get("version", "")
        results.append(result)

    return results


def check_connections() -> list:
    """Verify Harvey MCP is configured in each installed CLI."""
    clis = detect_clis()
    results = []

    for cli in clis:
        entry = {"cli": cli["name"], "version": cli.get("version", ""), "binary": cli["binary_path"]}

        if cli["method"] == "config":
            config_path = cli.get("config_path", "")
            if os.path.exists(config_path):
                try:
                    with open(config_path) as f:
                        config = json.load(f)
                    if "harvey" in config.get("mcp", {}):
                        entry["harvey_configured"] = True
                    else:
                        entry["harvey_configured"] = False
                except Exception:
                    entry["harvey_configured"] = False
            else:
                entry["harvey_configured"] = False
        else:
            # CLI-based check
            try:
                result = subprocess.run(
                    cli["check_cmd"], shell=True,
                    capture_output=True, text=True, timeout=10,
                )
                entry["harvey_configured"] = "harvey" in result.stdout.lower()
            except Exception:
                entry["harvey_configured"] = False

        results.append(entry)

    return results


def print_setup_results(results: list):
    """Pretty-print setup results."""
    print(f"\n{'=' * 55}")
    print(f"  Harvey MCP — Setup Results")
    print(f"{'=' * 55}")
    for r in results:
        icon = "✅" if r["status"] == "installed" else "❌"
        version = f" ({r.get('version', '')})" if r.get("version") else ""
        print(f"  {icon} {r['cli']}{version}: {r['status']}")
        if r.get("error"):
            print(f"     {r['error']}")
    print(f"{'=' * 55}")
    print(f"\n  Harvey is now available in all connected CLIs.")
    print(f"  Say 'Harvey, search the brain for X' in any CLI.\n")


def print_check_results(results: list):
    """Pretty-print connection check results."""
    print(f"\n{'=' * 55}")
    print(f"  Harvey MCP — Connection Check")
    print(f"{'=' * 55}")
    for r in results:
        icon = "✅" if r.get("harvey_configured") else "❌"
        version = f" ({r.get('version', '')})" if r.get("version") else ""
        status = "connected" if r.get("harvey_configured") else "NOT configured"
        print(f"  {icon} {r['cli']}{version}: {status}")
    print(f"{'=' * 55}\n")
