#!/usr/bin/env python3
"""`makakoo brain ...` CLI surface for multi-source brain management.

Subcommands:
  list                        — show every registered source + default
  add <name> <type> <path>    — register a new source (type: logseq|obsidian|plain)
  remove <name>               — unregister (refuses to remove default)
  set-default <name>          — switch which source is the write default
  sync [--name NAME]          — walk source(s) and print per-source doc counts (dry; true ingest lives in the SANCHO task)
  init                        — interactive picker (first-run wizard)

The CLI is a thin wrapper over `config.py` + `brain_source.py`. Kept in Python
for fast iteration; a future Rust-side `makakoo brain` subcommand can shell to
this or reimplement once the design stabilizes.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import config as cfg  # type: ignore
from brain_source import build_source  # type: ignore


def cmd_list(args) -> int:
    registry = cfg.load_registry()
    if args.json:
        print(json.dumps({
            "default": registry.default_name,
            "sources": [
                {"name": s.name, "root": str(s.root), "writable": s.writable,
                 "type": s.__class__.__name__}
                for s in registry.sources
            ],
        }, indent=2))
        return 0
    print(f"Brain sources ({len(registry.sources)} total, default: {registry.default_name}):\n")
    for s in registry.sources:
        flag = "writable" if s.writable else "read-only"
        kind = s.__class__.__name__.replace("Source", "").lower()
        star = " (default)" if s.name == registry.default_name else ""
        print(f"  [{kind:9}] {s.name}{star}")
        print(f"             {s.root}  ({flag})")
    return 0


def cmd_add(args) -> int:
    entry = {
        "name": args.name,
        "type": args.type,
        "path": args.path,
        "writable": not args.read_only,
    }
    path = cfg.add_source(entry)
    print(f"Added source {args.name!r} ({args.type}) → {path}")
    return 0


def cmd_remove(args) -> int:
    try:
        path = cfg.remove_source(args.name)
        print(f"Removed source {args.name!r}. Config at {path}")
        return 0
    except (KeyError, ValueError, FileNotFoundError) as e:
        print(f"Error: {e}", file=sys.stderr)
        return 1


def cmd_set_default(args) -> int:
    try:
        path = cfg.set_default(args.name)
        print(f"Default source → {args.name!r}. Config at {path}")
        return 0
    except (KeyError, FileNotFoundError) as e:
        print(f"Error: {e}", file=sys.stderr)
        return 1


def cmd_sync(args) -> int:
    """Dry walk: count docs in each source, show mtime range. No DB writes here."""
    registry = cfg.load_registry()
    targets = [registry.get(args.name)] if args.name else registry.sources
    for s in targets:
        count = 0
        oldest = newest = None
        for doc in s.iter_docs():
            count += 1
            oldest = min(oldest, doc.mtime) if oldest else doc.mtime
            newest = max(newest, doc.mtime) if newest else doc.mtime
        from datetime import datetime
        oldest_s = datetime.fromtimestamp(oldest).isoformat() if oldest else "n/a"
        newest_s = datetime.fromtimestamp(newest).isoformat() if newest else "n/a"
        print(f"{s.name}: {count} docs, range {oldest_s} → {newest_s}")
    return 0


def cmd_init(args) -> int:
    """Interactive picker — register additional sources beyond the default Logseq."""
    try:
        from picker import run_interactive  # type: ignore
    except ImportError:
        print("picker module not available", file=sys.stderr)
        return 2
    return run_interactive(non_interactive=args.non_interactive)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="makakoo brain", description="Multi-source brain management")
    sub = p.add_subparsers(dest="cmd", required=True)

    p_list = sub.add_parser("list", help="list registered sources")
    p_list.add_argument("--json", action="store_true")
    p_list.set_defaults(fn=cmd_list)

    p_add = sub.add_parser("add", help="register a new source")
    p_add.add_argument("name")
    p_add.add_argument("type", choices=["logseq", "obsidian", "plain"])
    p_add.add_argument("path")
    p_add.add_argument("--read-only", action="store_true")
    p_add.set_defaults(fn=cmd_add)

    p_rm = sub.add_parser("remove", help="unregister a source")
    p_rm.add_argument("name")
    p_rm.set_defaults(fn=cmd_remove)

    p_sd = sub.add_parser("set-default", help="change the write-default source")
    p_sd.add_argument("name")
    p_sd.set_defaults(fn=cmd_set_default)

    p_sync = sub.add_parser("sync", help="walk sources, count docs (dry)")
    p_sync.add_argument("--name", help="restrict to one source")
    p_sync.set_defaults(fn=cmd_sync)

    p_init = sub.add_parser("init", help="interactive first-run picker")
    p_init.add_argument("--non-interactive", action="store_true",
                        help="skip prompts, apply safe defaults (Logseq only)")
    p_init.set_defaults(fn=cmd_init)

    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
