#!/usr/bin/env python3
"""Migrate a ~/MAKAKOO/agents/<name>/ dir into a self-contained plugins-core/agent-<name>/.

Analogous to migrate_skill.py --copy-src but for agents:

- Reads agent.yaml for name, description, runtime.command, runtime.entrypoint, requires[]
- Copies the full agent dir into plugins-core/agent-<name>/src/
- Emits plugin.toml with kind="agent", [entrypoint].run wired to the yaml's runtime fields

Usage:
    python3 scripts/migrate_agent.py <name> [<name>...]

By default refuses to overwrite an existing plugins-core/agent-<name>/. Pass --force to nuke + re-migrate.
"""
from __future__ import annotations

import argparse
import re
import shutil
import sys
from pathlib import Path

try:
    import yaml  # PyYAML
except ImportError:
    print("error: PyYAML not installed — run `pip3 install pyyaml`", file=sys.stderr)
    sys.exit(2)


REPO_ROOT = Path(__file__).resolve().parent.parent
MAKAKOO_AGENTS = Path.home() / "MAKAKOO" / "agents"
PLUGINS_CORE = REPO_ROOT / "plugins-core"


TOML_STRING_UNSAFE = re.compile(r'[\\"\n\r]')


def toml_escape(s: str) -> str:
    # Replace backslashes + double quotes + newlines with safe equivalents for basic TOML strings.
    return (
        s.replace("\\", "\\\\")
        .replace('"', "'")
        .replace("\n", " ")
        .replace("\r", " ")
    )


def migrate(name: str, force: bool = False) -> tuple[str, str]:
    """Return (status, detail). status is 'ok' | 'skip' | 'error'."""
    src_dir = MAKAKOO_AGENTS / name
    if not src_dir.is_dir():
        return ("error", f"{src_dir} is not a directory")

    yaml_path = src_dir / "agent.yaml"
    if not yaml_path.is_file():
        return ("error", f"{yaml_path} missing")

    with yaml_path.open() as f:
        meta = yaml.safe_load(f) or {}

    agent_name = meta.get("name") or name
    # Normalize name to plugin naming regex ^[a-z][a-z0-9-]{1,62}$
    plugin_name = "agent-" + re.sub(r"[_\s]+", "-", agent_name.lower())
    plugin_name = re.sub(r"[^a-z0-9-]", "", plugin_name)

    description = meta.get("description", f"{agent_name} agent")
    version = str(meta.get("version", "0.1.0"))
    # clap expects SemVer — pad "1.0" to "1.0.0"
    if re.match(r"^\d+\.\d+$", version):
        version = f"{version}.0"
    if not re.match(r"^\d+\.\d+\.\d+", version):
        version = "0.1.0"

    runtime = meta.get("runtime") or {}
    command = runtime.get("command", "python3")
    entrypoint = runtime.get("entrypoint", "")
    args = runtime.get("args") or []

    requires = meta.get("requires") or []

    target_dir = PLUGINS_CORE / plugin_name
    target_src = target_dir / "src"

    if target_dir.exists():
        if not force:
            return ("skip", f"{target_dir} already exists (use --force to overwrite)")
        shutil.rmtree(target_dir)

    target_dir.mkdir(parents=True)

    # Copy entire agent dir → target/src/, excluding pycache.
    def ignore_cache(dirname, contents):
        return [c for c in contents if c in ("__pycache__",) or c.endswith(".pyc")]

    shutil.copytree(src_dir, target_src, ignore=ignore_cache)

    # Build [entrypoint].run — "python3 -u src/query.py arg1 arg2" style.
    # Strip absolute path from `command` (e.g. /usr/local/bin/python3.11 → python3).
    if command.startswith("/"):
        command = Path(command).name

    # Determine plugin language from the command.
    if command in ("python3", "python", "python3.11", "python3.12"):
        language = "python"
    elif command in ("bash", "sh", "zsh"):
        language = "shell"
    elif command in ("node", "deno", "bun"):
        language = "node"
    else:
        language = "binary"

    run_parts = [command]
    if language == "python":
        run_parts.append("-u")
    if entrypoint:
        run_parts.append(f"src/{entrypoint}")
    if args:
        run_parts.extend(str(a) for a in args)
    run = " ".join(run_parts)

    # Conservative capability grants — agents get state/plugin by default;
    # add llm/chat if any requires looks LLM-ish, brain/read+write if the
    # description mentions Brain.
    grants: list[str] = []
    requires_str = " ".join(requires).lower()
    desc_lower = description.lower()
    if any(k in requires_str for k in ("gemini", "switchai", "openai", "anthropic")):
        grants.append('"llm/chat"')
    if "brain" in desc_lower or "superbrain" in desc_lower or "journal" in desc_lower:
        grants.extend(['"brain/read"', '"brain/write"'])
    if "qdrant" in requires_str or "embed" in desc_lower:
        grants.append('"llm/embed"')

    grants_line = ", ".join(grants) if grants else ""

    plugin_toml = target_dir / "plugin.toml"
    # Agents need start + stop + health entrypoints (kind=agent is a
    # long-running daemon contract). Best-effort defaults:
    # - start: the run we just built
    # - stop:  pkill -f '<entrypoint>' — matches the process by script name
    # - health: `true` — always-healthy until the agent ships its own probe
    entry_for_stop = entrypoint or "agent.py"
    stop_cmd = f"pkill -f {entry_for_stop}"
    health_cmd = "true"

    body = f'''[plugin]
name = "{plugin_name}"
version = "{version}"
kind = "agent"
language = "{language}"
summary = "{toml_escape(description)}"
authors = ["Makakoo OS contributors"]
license = "MIT"

[source]
path = "plugins-core/{plugin_name}"

[abi]
agent = "^1.0"

[depends]
python = ">=3.9"

[entrypoint]
start = "{run}"
stop = "{stop_cmd}"
health = "{health_cmd}"

[capabilities]
grants = [{grants_line}]

[state]
dir = "$MAKAKOO_HOME/state/{plugin_name}"
retention = "keep"
'''
    plugin_toml.write_text(body)

    return ("ok", f"{target_dir} (entry: {run})")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("names", nargs="+", help="Agent name(s) under ~/MAKAKOO/agents/ to migrate")
    ap.add_argument("--force", action="store_true", help="Overwrite existing plugin dir")
    args = ap.parse_args()

    stats = {"ok": 0, "skip": 0, "error": 0}
    for name in args.names:
        status, detail = migrate(name, force=args.force)
        stats[status] += 1
        prefix = {"ok": "✓", "skip": "–", "error": "✗"}[status]
        print(f"  {prefix} {name}: {detail}")

    print(
        f"\nBatch: {stats['ok']} migrated, {stats['skip']} skipped, {stats['error']} failed"
    )
    return 0 if stats["error"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
