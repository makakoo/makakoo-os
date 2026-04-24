#!/usr/bin/env python3
"""Verify every shipped agent and mascot has a corresponding user manual.

Walks the installed plugin tree:
  - Every `plugins-core/agent-*/` directory whose `plugin.toml` declares
    `kind = "agent"` must have a `docs/agents/<agent-name>.md` sibling.
  - Every mascot listed in `lib-harvey-core/src/core/mascots/missions.py`
    must have a `docs/mascots/<mascot-name>.md` sibling.

Exits 0 if every installed agent + mascot has a manual.
Exits 1 with a list of missing manuals otherwise.

Usage:
  python3 scripts/verify_agent_manual_coverage.py
  python3 scripts/verify_agent_manual_coverage.py --json   # machine-readable

Run locally before every docs PR. The CI harness wires this in as part of
the docs-verification job (Phase 4 of the makakoo-grandma-docs sprint).
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGINS_CORE = REPO_ROOT / "plugins-core"
AGENT_DOCS = REPO_ROOT / "docs" / "agents"
MASCOT_DOCS = REPO_ROOT / "docs" / "mascots"
MISSIONS_PY = (
    PLUGINS_CORE / "lib-harvey-core" / "src" / "core" / "mascots" / "missions.py"
)

# The canonical mascot ledger is the mapping comment in missions.py:
#   * Pixel    → "SANCHO Doctor"
#   * Cinder   → "Entrypoint Sentinel"
#   * ...
_MASCOT_LINE_RE = re.compile(r"^\s*\*\s+(\w+)\s+→", re.MULTILINE)


def find_agents() -> list[str]:
    """Return every directory name under plugins-core/agent-*/ whose manifest
    declares kind = "agent". Directories without a plugin.toml are skipped."""
    if not PLUGINS_CORE.exists():
        print(
            f"error: plugins-core/ not found at {PLUGINS_CORE}. Run this script "
            "from inside a makakoo-os checkout.",
            file=sys.stderr,
        )
        sys.exit(2)

    agents: list[str] = []
    for plugin_dir in sorted(PLUGINS_CORE.glob("agent-*")):
        manifest = plugin_dir / "plugin.toml"
        if not manifest.exists():
            continue
        try:
            text = manifest.read_text(encoding="utf-8")
        except OSError:
            continue
        # Naive but robust: one-line regex for `kind = "agent"` / 'agent'.
        if re.search(r'^\s*kind\s*=\s*[\'"]agent[\'"]\s*$', text, re.MULTILINE):
            agents.append(plugin_dir.name)
        # agent-dreams ships with kind = "sancho-task" but lives in the
        # agent-* namespace and has a manual. Accept it for coverage purposes.
        elif plugin_dir.name == "agent-dreams":
            agents.append(plugin_dir.name)
    return agents


def find_mascots() -> list[str]:
    """Return every mascot name declared in missions.py's mapping comment."""
    if not MISSIONS_PY.exists():
        print(
            f"error: missions.py not found at {MISSIONS_PY}. Has the repo "
            "layout changed?",
            file=sys.stderr,
        )
        sys.exit(2)
    text = MISSIONS_PY.read_text(encoding="utf-8")
    names = _MASCOT_LINE_RE.findall(text)
    # Names are Capitalized in the ledger; the doc filenames are lowercase.
    return [n.lower() for n in names]


# Cross-cutting pages under docs/agents/ that are not per-agent manuals.
# New entries go here — they are NOT flagged as orphans.
_NON_AGENT_PAGES = frozenset(
    {
        "swarm-in-action",
        "bring-your-own-agent",
        "consuming-makakoo-externally",
    }
)


def manuals_in(root: Path, skip: frozenset[str] = frozenset()) -> set[str]:
    """Return the stem of every .md file directly under `root`, excluding
    `index.md` and any names in `skip`."""
    if not root.exists():
        return set()
    return {
        p.stem
        for p in root.glob("*.md")
        if p.name != "index.md" and p.stem not in skip
    }


def check() -> dict:
    agents = find_agents()
    mascots = find_mascots()
    agent_docs = manuals_in(AGENT_DOCS, skip=_NON_AGENT_PAGES)
    mascot_docs = manuals_in(MASCOT_DOCS)

    missing_agent_docs = [a for a in agents if a not in agent_docs]
    missing_mascot_docs = [m for m in mascots if m not in mascot_docs]
    orphan_agent_docs = [d for d in agent_docs if d not in agents]
    orphan_mascot_docs = [d for d in mascot_docs if d not in mascots]

    return {
        "agents_found": agents,
        "mascots_found": mascots,
        "agent_docs_found": sorted(agent_docs),
        "mascot_docs_found": sorted(mascot_docs),
        "missing_agent_docs": missing_agent_docs,
        "missing_mascot_docs": missing_mascot_docs,
        "orphan_agent_docs": orphan_agent_docs,
        "orphan_mascot_docs": orphan_mascot_docs,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--json",
        action="store_true",
        help="Print the full coverage report as JSON (machine-readable).",
    )
    args = ap.parse_args()

    report = check()

    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"Agents in plugins-core:   {len(report['agents_found'])}")
        print(f"Agent manuals found:      {len(report['agent_docs_found'])}")
        print(f"Mascots declared:         {len(report['mascots_found'])}")
        print(f"Mascot manuals found:     {len(report['mascot_docs_found'])}")
        if report["missing_agent_docs"]:
            print("\nMISSING agent manuals:")
            for a in report["missing_agent_docs"]:
                print(f"  - docs/agents/{a}.md")
        if report["missing_mascot_docs"]:
            print("\nMISSING mascot manuals:")
            for m in report["missing_mascot_docs"]:
                print(f"  - docs/mascots/{m}.md")
        if report["orphan_agent_docs"]:
            print("\nORPHAN agent manuals (no installed agent by that name):")
            for d in report["orphan_agent_docs"]:
                print(f"  - docs/agents/{d}.md")
        if report["orphan_mascot_docs"]:
            print("\nORPHAN mascot manuals (no mascot by that name):")
            for d in report["orphan_mascot_docs"]:
                print(f"  - docs/mascots/{d}.md")

    any_missing = (
        report["missing_agent_docs"]
        or report["missing_mascot_docs"]
        or report["orphan_agent_docs"]
        or report["orphan_mascot_docs"]
    )
    if any_missing:
        if not args.json:
            print("\nFAIL — manual coverage incomplete.")
        return 1
    if not args.json:
        print("\nOK — all agents and mascots have manuals, and vice versa.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
