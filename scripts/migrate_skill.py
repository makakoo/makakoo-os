#!/usr/bin/env python3
"""Migrate a harvey-os skill into a self-contained plugins-core plugin.

Two modes:

1. **Manifest-only (default)** — emits `plugin.toml` pointing at
   `harvey-os/skills/<cat>/<name>/<entry>`. Only works on machines that
   have the harvey-os submodule checked out (Sebastian's daily driver).

2. **Copy source (`--copy-src`)** — additionally `cp -R`'s the entire
   skill dir into `plugins-core/skill-<cat>-<name>/src/`. The emitted
   `plugin.toml` uses a relative `run = "python3 -u src/<entry>"`. The
   resulting plugin is self-contained: publicly distributable, no
   harvey-os dependency. This is the v0.1 launch shape.

Usage:
    # Manifest-only:
    python3 scripts/migrate_skill.py <cat> <name>

    # Self-contained (v0.1 public shape):
    python3 scripts/migrate_skill.py --copy-src <cat> <name> [<cat> <name>...]

    # Force re-migration over an existing plugin dir:
    python3 scripts/migrate_skill.py --copy-src --force research arxiv

Safe to re-run — refuses to overwrite an existing plugin dir unless
`--force` is passed. `--copy-src` without `--force` also refuses if
`<plugin>/src/` already exists.
"""
from __future__ import annotations

import argparse
import re
import shutil
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


def migrate_one(
    category: str,
    skill_name: str,
    force: bool,
    copy_src: bool,
) -> tuple[str, str]:
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
    src_subdir = dest / "src"

    if plugin_toml.exists() and not force:
        return ("exists", str(plugin_toml))

    dest.mkdir(parents=True, exist_ok=True)

    # --copy-src: mirror the harvey-os skill dir into plugins-core/<name>/src/
    # Self-contained plugin = publicly distributable with no harvey-os dependency.
    if copy_src:
        if src_subdir.exists():
            if not force:
                return ("src-exists", str(src_subdir))
            shutil.rmtree(src_subdir)
        shutil.copytree(src, src_subdir)

    # Pick the run command based on mode.
    if copy_src:
        if entry:
            entry_cmd = f"python3 -u src/{entry}"
        else:
            # Fallback: `-m` style against the copied src tree. Requires
            # a PYTHONPATH set by the CLI's build_skill_env.
            entry_cmd = f"python3 -u -m src.__main__"
    else:
        if entry:
            entry_cmd = (
                f"python3 -u harvey-os/skills/{category}/{skill_name}/{entry}"
            )
        else:
            entry_cmd = (
                f"python3 -u -m harvey_os.skills.{category}.{skill_name}"
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
# Capability grants default empty — conservative for batch migration.
# Add brain/llm/net/state grants manually per skill's real needs.
grants = []

[state]
dir = "$MAKAKOO_HOME/state/{plugin_name}"
retention = "keep"
'''
    )
    mode = "copy-src" if copy_src else "ref"
    return (f"wrote-{mode}", str(plugin_toml))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pairs", nargs="+", help="category skill_name pairs")
    ap.add_argument("--force", action="store_true",
                    help="Overwrite existing plugin dir + src/ subdir")
    ap.add_argument("--copy-src", action="store_true",
                    help="Copy harvey-os source into plugins-core/<name>/src/ "
                         "so the plugin is publicly distributable")
    args = ap.parse_args()

    if len(args.pairs) % 2 != 0:
        print("pairs must come in category+name pairs", file=sys.stderr)
        return 1

    for i in range(0, len(args.pairs), 2):
        cat, name = args.pairs[i], args.pairs[i + 1]
        status, detail = migrate_one(cat, name, args.force, args.copy_src)
        print(f"  {status:<16} {cat}/{name}  → {detail}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
