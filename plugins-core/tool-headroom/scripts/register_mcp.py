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


def server_json(
    command: str,
    args: list[str],
    proxy_url: str,
    *,
    extra: dict | None = None,
) -> dict:
    entry = {
        "command": command,
        "args": args,
        "description": DESCRIPTION,
        "env": {"HEADROOM_PROXY_URL": proxy_url},
    }
    if extra:
        entry.update(extra)
    return entry


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


def upsert_mcpservers_json(
    path: Path,
    command: str,
    args: list[str],
    proxy_url: str,
    *,
    extra: dict | None = None,
) -> str:
    data = load_json(path)
    servers = data.setdefault("mcpServers", {})
    servers["headroom"] = server_json(command, args, proxy_url, extra=extra)
    return "updated" if write_json(path, data) else "unchanged"


def upsert_claude(home: Path, command: str, args: list[str], proxy_url: str) -> str:
    # alwaysLoad avoids Claude Code /compact 400 when deferred headroom tools are referenced.
    extra = {"type": "stdio", "alwaysLoad": True}
    primary = upsert_mcpservers_json(home / ".claude.json", command, args, proxy_url, extra=extra)
    secondary_path = home / ".claude" / ".claude.json"
    if secondary_path.exists():
        sec_data = load_json(secondary_path)
        if isinstance(sec_data.get("mcpServers"), dict):
            sec = upsert_mcpservers_json(secondary_path, command, args, proxy_url, extra=extra)
            if primary == "unchanged" and sec == "updated":
                return "updated"
    return primary


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


def remove_toml_mcp_server(text: str) -> str:
    # Drop [mcp_servers.headroom] and all nested [mcp_servers.headroom.*]
    # sections until the next unrelated TOML table or array-of-tables header.
    lines = text.splitlines()
    out: list[str] = []
    skip = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            name = stripped.strip("[]").strip()
            if name == "mcp_servers.headroom" or name.startswith("mcp_servers.headroom."):
                skip = True
                continue
            skip = False
        if not skip:
            out.append(line)
    return "\n".join(out).rstrip() + "\n"


def upsert_toml_mcp_server(
    path: Path,
    command: str,
    args: list[str],
    proxy_url: str,
    *,
    startup_timeout_sec: str = "120.0",
    include_description: bool = True,
    approved_tools: tuple[str, ...] = (),
) -> str:
    before = path.read_text() if path.exists() else ""
    body = remove_toml_mcp_server(before)
    desc_line = f"description = {toml_string(DESCRIPTION)}\n" if include_description else ""
    tool_blocks = "".join(
        f'\n[mcp_servers.headroom.tools.{tool}]\napproval_mode = "approve"\n'
        for tool in approved_tools
    )
    block = f"""
[mcp_servers.headroom]
command = {toml_string(command)}
args = {toml_array(args)}
{desc_line}enabled = true
startup_timeout_sec = {startup_timeout_sec}

[mcp_servers.headroom.env]
HEADROOM_PROXY_URL = {toml_string(proxy_url)}
{tool_blocks}"""
    after = body.rstrip() + "\n\n" + block.lstrip()
    path.parent.mkdir(parents=True, exist_ok=True)
    if before == after:
        return "unchanged"
    path.write_text(after)
    return "updated"


def upsert_codex(path: Path, command: str, args: list[str], proxy_url: str) -> str:
    # Codex v0.169+ warns on unrecognized `description` under [mcp_servers.*]
    # and requires per-tool approval_mode="approve" when approval_policy="never".
    return upsert_toml_mcp_server(
        path,
        command,
        args,
        proxy_url,
        startup_timeout_sec="120.0",
        include_description=False,
        approved_tools=("headroom_compress", "headroom_retrieve", "headroom_stats"),
    )


def upsert_grok(path: Path, command: str, args: list[str], proxy_url: str) -> str:
    return upsert_toml_mcp_server(
        path, command, args, proxy_url, startup_timeout_sec="120"
    )


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


def detected(name: str, path: Path, home: Path) -> bool:
    if name == "claude":
        return path.exists() or (home / ".claude").exists()
    if name == "agy":
        return path.exists() or (home / ".gemini" / "antigravity-cli").exists()
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
        "claude": lambda: upsert_claude(home, command, args, ns.proxy_url),
        "agy": lambda: upsert_mcpservers_json(
            home / ".gemini" / "config" / "mcp_config.json", command, args, ns.proxy_url
        ),
        "gemini": lambda: upsert_mcpservers_json(
            home / ".gemini" / "settings.json", command, args, ns.proxy_url
        ),
        "codex": lambda: upsert_codex(home / ".codex" / "config.toml", command, args, ns.proxy_url),
        "grok": lambda: upsert_grok(home / ".grok" / "config.toml", command, args, ns.proxy_url),
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
        "kimi": lambda: upsert_mcpservers_json(
            home / ".kimi-code" / "mcp.json",
            command,
            args,
            ns.proxy_url,
            extra={"startupTimeoutMs": 120000},
        ),
    }

    paths = {
        "claude": home / ".claude.json",
        "agy": home / ".gemini" / "config" / "mcp_config.json",
        "gemini": home / ".gemini" / "settings.json",
        "codex": home / ".codex" / "config.toml",
        "grok": home / ".grok" / "config.toml",
        "opencode": home / ".config" / "opencode" / "opencode.json",
        "vibe": home / ".vibe" / "config.toml",
        "qwen": home / ".qwen" / "settings.json",
        "cursor": home / ".cursor" / "mcp.json",
        "kimi": home / ".kimi-code" / "mcp.json",
    }

    for name, fn in targets.items():
        if not all_agents and name not in agents:
            continue
        if all_agents and not detected(name, paths[name], home):
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
