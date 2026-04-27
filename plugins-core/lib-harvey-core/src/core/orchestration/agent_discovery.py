#!/usr/bin/env python3
"""
Harvey Agent Discovery — Scans the environment for available AI agents
v0.1.0

Scans for:
- AI CLI agents (claude, gemini, codex, goose, opencode, etc.)
- MCP servers configured
- Remote AI services configured
- Skills installed

Usage:
    harvey agents discover          # Discover all available agents
    harvey agents scan --full       # Full scan with capability detection
    harvey agents capabilities <id>  # Show capabilities of specific agent
"""

import argparse
import json
import os
import shutil
import subprocess
from pathlib import Path
from datetime import datetime

HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", Path.home() / "harvey"))
WORKSPACE = HARVEY_HOME / "workspace"
AGENTS_DIR = WORKSPACE / "agents"

# Known AI agent executables and their capability signatures
KNOWN_AGENTS = {
    "claude-code": {
        "command": "claude",
        "search_names": ["claude", "claude-code"],
        "capabilities": ["code", "research", "write", "analysis", "debug", "review", "plan"],
        "type": "local",
        "protocol": "cli",
    },
    "gemini-cli": {
        "command": "gemini",
        "search_names": ["gemini", "gemini-cli"],
        "capabilities": ["research", "general", "writing", "analysis"],
        "type": "local",
        "protocol": "cli",
    },
    "codex": {
        "command": "codex",
        "search_names": ["codex", "openai-codex"],
        "capabilities": ["code", "implementation", "automation"],
        "type": "local",
        "protocol": "cli",
    },
    "goose": {
        "command": "goose",
        "search_names": ["goose"],
        "capabilities": ["code", "research", "implementation", "automation"],
        "type": "local",
        "protocol": "cli",
    },
    "opencode": {
        "command": "opencode",
        "search_names": ["opencode"],
        "capabilities": ["code", "general", "analysis"],
        "type": "local",
        "protocol": "cli",
    },
    "kimi-cli": {
        "command": "kimi",
        "search_names": ["kimi", "kimi-cli"],
        "capabilities": ["research", "writing", "general"],
        "type": "local",
        "protocol": "cli",
    },
    "mistral-vibe": {
        "command": "mistral-vibe",
        "search_names": ["mistral-vibe", "mistral_vibe"],
        "capabilities": ["code", "general"],
        "type": "local",
        "protocol": "cli",
    },
    "hermes-agent": {
        "command": "hermes",
        "search_names": ["hermes", "hermes-agent"],
        "capabilities": ["general", "research", "self-improvement", "learning"],
        "type": "local",
        "protocol": "cli",
    },
}


def find_executable(name: str) -> str | None:
    """Find executable in PATH."""
    path = shutil.which(name)
    if path:
        return path

    # Try common variations
    for search in [f"{name}", f"{name}.py", f"npx {name}"]:
        path = shutil.which(search.split()[0])
        if path:
            return path

    return None


def check_agent_installed(agent_id: str, agent_info: dict) -> dict:
    """Check if agent is installed and get version info."""
    result = {
        "agent_id": agent_id,
        "installed": False,
        "path": None,
        "version": None,
        "status": "NOT_FOUND",
        "last_checked": datetime.utcnow().isoformat() + "Z",
    }

    for cmd_name in agent_info.get("search_names", [agent_info["command"]]):
        path = find_executable(cmd_name)
        if path:
            result["installed"] = True
            result["path"] = path
            result["status"] = "IDLE"

            # Try to get version
            try:
                ver = subprocess.run(
                    [cmd_name, "--version"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                )
                if ver.returncode == 0:
                    result["version"] = ver.stdout.strip()[:100]
            except Exception:
                pass

            break

    return result


def check_mcp_servers() -> list:
    """Discover configured MCP servers."""
    mcp_servers = []

    # Check common MCP config locations
    config_locations = [
        Path.home() / ".config" / "mcp" / "servers.json",
        Path.home() / ".mcp" / "servers.json",
        HARVEY_HOME / "config" / "mcp.json",
    ]

    for loc in config_locations:
        if loc.exists():
            try:
                data = json.loads(loc.read_text())
                if isinstance(data, dict) and "mcpServers" in data:
                    for name, config in data["mcpServers"].items():
                        mcp_servers.append({
                            "id": f"mcp:{name}",
                            "name": name,
                            "type": "mcp",
                            "config": config,
                            "source": str(loc),
                        })
            except Exception:
                pass

    return mcp_servers


def check_remote_agents() -> list:
    """Check for configured remote AI services."""
    remote = []

    # Check environment for API keys
    api_keys = {
        "ANTHROPIC_API_KEY": ("anthropic", "Claude API"),
        "OPENAI_API_KEY": ("openai", "OpenAI API"),
        "GOOGLE_AI_API_KEY": ("google", "Google AI API"),
        "GEMINI_API_KEY": ("google", "Gemini API"),
    }

    for env_var, (provider, name) in api_keys.items():
        if os.environ.get(env_var):
            remote.append({
                "agent_id": f"remote:{provider}",
                "name": f"Remote {name}",
                "type": "remote",
                "provider": provider,
                "endpoint": f"${env_var}",
                "status": "CONFIGURED" if os.environ.get(env_var) else "NOT_CONFIGURED",
                "capabilities": ["code", "research", "general", "analysis"],
            })

    return remote


def discover_all() -> dict:
    """Run full agent discovery."""
    agents = {}
    now = datetime.utcnow().isoformat() + "Z"

    # Check known local agents
    for agent_id, info in KNOWN_AGENTS.items():
        result = check_agent_installed(agent_id, info)
        result["capabilities"] = info["capabilities"]
        result["protocol"] = info["protocol"]
        result["command"] = info["command"]
        agents[agent_id] = result

    # Check MCP servers
    mcp_servers = check_mcp_servers()
    for mcp in mcp_servers:
        agents[mcp["id"]] = mcp

    # Check remote services
    remote_agents = check_remote_agents()
    for remote in remote_agents:
        agents[remote["agent_id"]] = remote

    return {
        "discovered_at": now,
        "agents": agents,
        "summary": {
            "total": len(agents),
            "installed": sum(1 for a in agents.values() if a.get("installed")),
            "configured": sum(1 for a in agents.values() if a.get("status") == "CONFIGURED"),
            "mcp": len(mcp_servers),
        },
    }


def cmd_discover(args):
    """Discover available agents."""
    results = discover_all()

    print()
    print("╔══════════════════════════════════════════════════════════════╗")
    print("║  HARVEY AGENT DISCOVERY                                     ║")
    print("╠══════════════════════════════════════════════════════════════╣")
    print(f"║  {results['summary']['installed']} installed / {results['summary']['total']} known agents     ")

    for agent_id, info in sorted(results["agents"].items()):
        installed = "✓" if info.get("installed") else "✗"
        configured = "●" if info.get("status") == "CONFIGURED" else " "
        status = info.get("status", "?")

        print(f"║  {installed} {configured} {agent_id:20s} {status:12s}  ", end="")

        caps = info.get("capabilities", [])
        if caps:
            print(f"{', '.join(caps[:3])}", end="")
        print("  ║")

        if info.get("version"):
            print(f"║         version: {info['version'][:50]}")
        if info.get("path"):
            print(f"║         path: {info['path'][:50]}")

    print("╚══════════════════════════════════════════════════════════════╝")

    # Save to registry
    AGENTS_DIR.mkdir(parents=True, exist_ok=True)
    registry_path = AGENTS_DIR / "agent_discovery.json"
    registry_path.write_text(json.dumps(results, indent=2))
    print(f"\nSaved to {registry_path}")

    return results


def cmd_capabilities(args):
    """Show detailed capabilities of an agent."""
    registry_path = AGENTS_DIR / "agent_discovery.json"
    if not registry_path.exists():
        print("No discovery data. Run `harvey agents discover` first.")
        return

    data = json.loads(registry_path.read_text())
    agent = data.get("agents", {}).get(args.agent_id)

    if not agent:
        print(f"Agent {args.agent_id} not found.")
        return

    print(f"Agent: {args.agent_id}")
    print(f"Type: {agent.get('type')}")
    print(f"Status: {agent.get('status')}")
    print(f"Installed: {agent.get('installed', False)}")
    if agent.get("path"):
        print(f"Path: {agent['path']}")
    if agent.get("version"):
        print(f"Version: {agent['version']}")
    print(f"Capabilities: {', '.join(agent.get('capabilities', []))}")
    if agent.get("protocol"):
        print(f"Protocol: {agent.get('protocol')}")


def main():
    parser = argparse.ArgumentParser(prog="harvey agents", description="Harvey Agent Discovery")
    sub = parser.add_subparsers(dest="cmd")

    discover = sub.add_parser("discover", help="Discover available agents")
    discover.add_argument("--full", action="store_true", help="Full capability detection scan")

    cap = sub.add_parser("capabilities", help="Show agent capabilities")
    cap.add_argument("agent_id", help="Agent ID")

    scan = sub.add_parser("scan", help="Full environment scan")
    scan.add_argument("--full", action="store_true")

    args = parser.parse_args()

    if args.cmd == "discover":
        cmd_discover(args)
    elif args.cmd == "capabilities":
        cmd_capabilities(args)
    elif args.cmd == "scan":
        cmd_discover(args)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
