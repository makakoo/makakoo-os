#!/usr/bin/env python3
"""Review and promote staged memory drafts produced by the session observer.

Usage:
    python3 memory_review.py list                 # list pending drafts
    python3 memory_review.py show <filename>      # print draft body
    python3 memory_review.py promote <filename>   # move draft to live memory + update MEMORY.md
    python3 memory_review.py reject <filename>    # delete draft
    python3 memory_review.py cleanup              # remove promoted/rejected markers

A "draft" is a markdown file under memory/drafts/ with Brain-compatible frontmatter.
Promotion copies the file to memory/ (live), appends a one-line entry to MEMORY.md,
and leaves a .promoted marker in drafts/ so repeated runs don't re-promote.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

MEMORY_ROOT = Path.home() / ".claude" / "projects" / "-Users-sebastian-MAKAKOO" / "memory"
DRAFTS_DIR = MEMORY_ROOT / "drafts"
MEMORY_INDEX = MEMORY_ROOT / "MEMORY.md"


def _active_drafts() -> list[Path]:
    if not DRAFTS_DIR.exists():
        return []
    return sorted(
        p for p in DRAFTS_DIR.glob("*.md")
        if not (p.with_suffix(".promoted").exists() or p.with_suffix(".rejected").exists())
    )


def _parse_frontmatter(text: str) -> dict:
    m = re.match(r"^---\n(.*?)\n---\n", text, re.DOTALL)
    if not m:
        return {}
    block = m.group(1)
    out = {}
    for line in block.splitlines():
        if ":" in line:
            k, _, v = line.partition(":")
            out[k.strip()] = v.strip()
    return out


def cmd_list() -> int:
    drafts = _active_drafts()
    if not drafts:
        print("No pending memory drafts.")
        return 0
    print(f"{len(drafts)} draft(s) pending review:\n")
    for p in drafts:
        meta = _parse_frontmatter(p.read_text(errors="replace"))
        name = meta.get("name", p.stem)
        desc = meta.get("description", "").strip()[:100]
        mtype = meta.get("type", "?")
        print(f"  [{mtype:9}] {p.name}")
        print(f"             name: {name}")
        if desc:
            print(f"             desc: {desc}")
        print()
    return 0


def cmd_show(filename: str) -> int:
    path = DRAFTS_DIR / filename
    if not path.exists():
        print(f"Not found: {path}", file=sys.stderr)
        return 1
    print(path.read_text(errors="replace"))
    return 0


def _update_memory_index(name: str, filename: str, description: str) -> None:
    if not MEMORY_INDEX.exists():
        return
    entry = f"- [{name}]({filename}) — {description}\n"
    current = MEMORY_INDEX.read_text()
    if filename in current:
        return
    MEMORY_INDEX.write_text(current.rstrip() + "\n" + entry)


def cmd_promote(filename: str) -> int:
    src = DRAFTS_DIR / filename
    if not src.exists():
        print(f"Not found: {src}", file=sys.stderr)
        return 1
    meta = _parse_frontmatter(src.read_text(errors="replace"))
    name = meta.get("name") or src.stem
    description = meta.get("description", "")

    dest_filename = f"{meta.get('type', 'memory')}_{name}.md"
    dest_filename = re.sub(r"[^a-zA-Z0-9_.-]", "_", dest_filename)
    dest = MEMORY_ROOT / dest_filename

    dest.write_text(src.read_text())
    src.with_suffix(".promoted").write_text(f"promoted -> {dest_filename}\n")
    _update_memory_index(name, dest_filename, description)
    print(f"Promoted: {src.name} -> {dest_filename}")
    print(f"Index updated: {MEMORY_INDEX}")
    return 0


def cmd_reject(filename: str) -> int:
    src = DRAFTS_DIR / filename
    if not src.exists():
        print(f"Not found: {src}", file=sys.stderr)
        return 1
    src.with_suffix(".rejected").write_text("rejected\n")
    src.unlink()
    print(f"Rejected: {filename}")
    return 0


def cmd_cleanup() -> int:
    removed = 0
    for marker in DRAFTS_DIR.glob("*.promoted"):
        marker.unlink()
        removed += 1
    for marker in DRAFTS_DIR.glob("*.rejected"):
        marker.unlink()
        removed += 1
    print(f"Removed {removed} marker file(s).")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Review staged memory drafts.")
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    p_show = sub.add_parser("show")
    p_show.add_argument("filename")
    p_prom = sub.add_parser("promote")
    p_prom.add_argument("filename")
    p_rej = sub.add_parser("reject")
    p_rej.add_argument("filename")
    sub.add_parser("cleanup")

    args = parser.parse_args()
    if args.cmd == "list":
        return cmd_list()
    if args.cmd == "show":
        return cmd_show(args.filename)
    if args.cmd == "promote":
        return cmd_promote(args.filename)
    if args.cmd == "reject":
        return cmd_reject(args.filename)
    if args.cmd == "cleanup":
        return cmd_cleanup()
    return 2


if __name__ == "__main__":
    sys.exit(main())
