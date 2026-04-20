#!/usr/bin/env python3
"""Generate a plugins-core/skill-<cat>-<name>/plugin.toml from an existing
harvey-os/skills/<cat>/<name>/SKILL.md.

Usage:
    python3 scripts/migrate_skill.py <category> <skill_name> [<category> <skill_name> ...]

Reads the SKILL.md frontmatter for description + optional entry/grants,
emits a minimal-but-valid plugin.toml pointing at the existing Python
code in harvey-os/skills/. This is the Phase H.4 batch-migration path:
the manifest is the migration, not a rewrite.

Safe to re-run — refuses to overwrite an existing plugin dir unless
`--force` is passed.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
HARVEY_OS_SKILLS = Path.home() / "MAKAKOO" / "harvey-os" / "skills"
PLUGINS_CORE = REPO_ROOT / "plugins-core"


def parse_frontmatter(text: str) -> dict[str, str]:
    m = re.match(r"^---\n(.*?)\n---", text, re.DOTALL)
    if not m:
        return {}
    out: dict[str, str] = {}
    for line in m.group(1).splitlines():
        if ":" not in line:
            continue
        k, _, v = line.partition(":")
        out[k.strip()] = v.strip().strip('"')
    return out


def find_entrypoint(skill_dir: Path) -> str:
    # Common shapes, in order of preference.
    for candidate in ("run.py", "wizard.py"):
        if (skill_dir / candidate).exists():
            return candidate
    # Look for `<name>_wizard.py` or `<name>.py`.
    for p in skill_dir.iterdir():
        if p.suffix == ".py" and p.stem.startswith(skill_dir.name):
            return p.name
    # Fallback: any top-level .py.
    for p in skill_dir.iterdir():
        if p.suffix == ".py" and p.name != "__init__.py":
            return p.name
    return ""


def migrate_one(category: str, skill_name: str, force: bool) -> tuple[str, str]:
    src = HARVEY_OS_SKILLS / category / skill_name
    if not src.is_dir():
        return ("skip", f"harvey-os/skills/{category}/{skill_name} not found")

    skill_md = src / "SKILL.md"
    fm = parse_frontmatter(skill_md.read_text() if skill_md.exists() else "")
    summary = fm.get("description") or fm.get("name") or f"{skill_name} skill"
    entry = fm.get("entry") or find_entrypoint(src)

    plugin_name = f"skill-{category}-{skill_name}"
    dest = PLUGINS_CORE / plugin_name
    plugin_toml = dest / "plugin.toml"

    if plugin_toml.exists() and not force:
        return ("exists", str(plugin_toml))

    dest.mkdir(parents=True, exist_ok=True)

    # Default capability set — no grants. Skills that need brain/llm/net
    # should hand-edit after the batch migration; v0.1 conservative.
    entry_cmd = (
        f'python3 -u harvey-os/skills/{category}/{skill_name}/{entry}'
        if entry
        else f"python3 -u -m harvey_os.skills.{category}.{skill_name}"
    )

    plugin_toml.write_text(
        f'''[plugin]
name = "{plugin_name}"
version = "0.1.0"
kind = "skill"
language = "python"
summary = "{summary.replace('"', "'")}"
authors = ["Makakoo OS contributors"]
license = "MIT"

[source]
path = "plugins-core/{plugin_name}"

[abi]
skill = "^1.0"

[depends]
python = ">=3.8"

[entrypoint]
run = "{entry_cmd}"

[capabilities]
# Capability grants default empty — conservative for Phase H.4 batch.
# Add brain/llm/net/state grants manually per skill's real needs.
grants = []

[state]
dir = "$MAKAKOO_HOME/state/{plugin_name}"
retention = "keep"
'''
    )
    return ("wrote", str(plugin_toml))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pairs", nargs="+", help="category skill_name pairs")
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()

    if len(args.pairs) % 2 != 0:
        print("pairs must come in category+name pairs", file=sys.stderr)
        return 1

    for i in range(0, len(args.pairs), 2):
        cat, name = args.pairs[i], args.pairs[i + 1]
        status, detail = migrate_one(cat, name, args.force)
        print(f"  {status:<6} {cat}/{name}  → {detail}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
