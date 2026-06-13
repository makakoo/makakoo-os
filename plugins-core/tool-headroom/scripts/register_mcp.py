#!/usr/bin/env python3
"""Register Headroom MCP across Makakoo-supported CLI hosts.

Upstream `headroom mcp install` currently registers Claude Code only. Makakoo
infects more MCP-capable hosts, so the plugin owns a small registrar that writes
the native config shape for each host.
"""

from __future__ import annotations

import argparse
import json
import os
import re
from pathlib import Path


DESCRIPTION = (
    "Headroom context compression MCP — exposes headroom_compress, "
    "headroom_retrieve, and headroom_stats for bulky tool output."
)


def server_json(command: str, args: list[str], proxy_url: str) -> dict:
    return {
        "command": command,
        "args": args,
        "description": DESCRIPTION,
        "env": {"HEADROOM_PROXY_URL": proxy_url},
    }


def load_json(path: Path) -> dict:
    if not path.exists():
        return {}
    try:
        data = json.loads(path.read_text())
    except Exception:
        return {}
    return data if isinstance(data, dict) else {}


def write_json(path: Path, data: dict) -> bool:
    before = path.read_text() if path.exists() else None
    path.parent.mkdir(parents=True, exist_ok=True)
    body = json.dumps(data, indent=2, sort_keys=False) + "\n"
    if before == body:
        return False
    path.write_text(body)
    return True


def upsert_mcpservers_json(path: Path, command: str, args: list[str], proxy_url: str) -> str:
    data = load_json(path)
    servers = data.setdefault("mcpServers", {})
    servers["headroom"] = server_json(command, args, proxy_url)
    return "updated" if write_json(path, data) else "unchanged"


def upsert_opencode(path: Path, command: str, args: list[str], proxy_url: str) -> str:
    data = load_json(path)
    servers = data.setdefault("mcp", {})
    servers["headroom"] = {
        "type": "local",
        "enabled": True,
        "command": [command, *args],
        "description": DESCRIPTION,
        "environment": {"HEADROOM_PROXY_URL": proxy_url},
    }
    return "updated" if write_json(path, data) else "unchanged"


def toml_string(value: str) -> str:
    return json.dumps(value)


def toml_array(values: list[str]) -> str:
    return "[" + ", ".join(toml_string(v) for v in values) + "]"


def remove_codex_server(text: str) -> str:
    # Drop [mcp_servers.headroom] and all nested [mcp_servers.headroom.*]
    # sections until the next unrelated TOML section.
    lines = text.splitlines()
    out: list[str] = []
    skip = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            name = stripped.strip("[]")
            if name == "mcp_servers.headroom" or name.startswith("mcp_servers.headroom."):
                skip = True
                continue
            skip = False
        if not skip:
            out.append(line)
    return "\n".join(out).rstrip() + "\n"


def upsert_codex(path: Path, command: str, args: list[str], proxy_url: str) -> str:
    before = path.read_text() if path.exists() else ""
    body = remove_codex_server(before)
    block = f"""
[mcp_servers.headroom]
command = {toml_string(command)}
args = {toml_array(args)}
description = {toml_string(DESCRIPTION)}

[mcp_servers.headroom.env]
HEADROOM_PROXY_URL = {toml_string(proxy_url)}
"""
    after = body.rstrip() + "\n" + block.lstrip()
    path.parent.mkdir(parents=True, exist_ok=True)
    if before == after:
        return "unchanged"
    path.write_text(after)
    return "updated"


def remove_vibe_headroom(text: str) -> str:
    pattern = re.compile(
        r"\n?\[\[mcp_servers\]\]\n"
        r"(?:(?!\n\[\[mcp_servers\]\]|\n\[\[providers\]\]|\n\[\[models\]\]).)*"
        r'name\s*=\s*"headroom"'
        r"(?:(?!\n\[\[mcp_servers\]\]|\n\[\[providers\]\]|\n\[\[models\]\]).)*",
        re.S,
    )
    return pattern.sub("\n", text).rstrip() + "\n"


def upsert_vibe(path: Path, command: str, args: list[str], proxy_url: str) -> str:
    before = path.read_text() if path.exists() else ""
    body = remove_vibe_headroom(before)
    block = f"""
[[mcp_servers]]
transport = "stdio"
name = "headroom"
command = {toml_string(command)}
args = {toml_array(args)}
prompt = {toml_string(DESCRIPTION)}

[mcp_servers.env]
HEADROOM_PROXY_URL = {toml_string(proxy_url)}
"""
    after = body.rstrip() + "\n\n" + block.lstrip()
    path.parent.mkdir(parents=True, exist_ok=True)
    if before == after:
        return "unchanged"
    path.write_text(after)
    return "updated"


def detected(path: Path) -> bool:
    return path.exists() or path.parent.exists()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", default=str(Path.home()))
    parser.add_argument("--command", required=True)
    parser.add_argument("--proxy-url", default="http://127.0.0.1:8787")
    parser.add_argument("--agent", action="append", default=[])
    parser.add_argument("--dry-run", action="store_true")
    ns = parser.parse_args()

    home = Path(ns.home).expanduser()
    command = str(Path(ns.command).expanduser())
    if Path(command).name == "headroom-mcp-stdio":
        args: list[str] = []
    else:
        args = ["mcp", "serve", "--proxy-url", ns.proxy_url]

    agents = set(ns.agent or [])
    all_agents = not agents

    targets = {
        "claude": lambda: upsert_mcpservers_json(home / ".claude.json", command, args, ns.proxy_url),
        "gemini": lambda: upsert_mcpservers_json(
            home / ".gemini" / "settings.json", command, args, ns.proxy_url
        ),
        "codex": lambda: upsert_codex(home / ".codex" / "config.toml", command, args, ns.proxy_url),
        "opencode": lambda: upsert_opencode(
            home / ".config" / "opencode" / "opencode.json", command, args, ns.proxy_url
        ),
        "vibe": lambda: upsert_vibe(home / ".vibe" / "config.toml", command, args, ns.proxy_url),
        "qwen": lambda: upsert_mcpservers_json(
            home / ".qwen" / "settings.json", command, args, ns.proxy_url
        ),
        "cursor": lambda: upsert_mcpservers_json(
            home / ".cursor" / "mcp.json", command, args, ns.proxy_url
        ),
    }

    paths = {
        "claude": home / ".claude.json",
        "gemini": home / ".gemini" / "settings.json",
        "codex": home / ".codex" / "config.toml",
        "opencode": home / ".config" / "opencode" / "opencode.json",
        "vibe": home / ".vibe" / "config.toml",
        "qwen": home / ".qwen" / "settings.json",
        "cursor": home / ".cursor" / "mcp.json",
    }

    for name, fn in targets.items():
        if not all_agents and name not in agents:
            continue
        if all_agents and not detected(paths[name]):
            print(f"{name}: skipped (not detected)")
            continue
        if ns.dry_run:
            print(f"{name}: would-update {paths[name]}")
            continue
        try:
            status = fn()
            print(f"{name}: {status} {paths[name]}")
        except Exception as exc:
            print(f"{name}: error {exc}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
