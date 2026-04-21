"""
skill-freelance-office — argparse top-level dispatcher.

10 subcommands. Each lives in `src.commands.<name>` and exposes a
`run(args) -> dict` returning a JSON envelope. Top-level handles
`--json` (dump envelope) vs human (pretty subset) and `--dry-run`
(short-circuit before any disk write with a preview).

Phase 1: every subcommand is a stub that returns
`{"status": "stub", "command": <name>, "phase": 1}`. Phase 3 replaces
these with real logic.
"""

from __future__ import annotations

import argparse
import json
import sys
from importlib import import_module
from typing import Any, Callable, Dict, List, Optional

SUBCOMMANDS: Dict[str, str] = {
    "init":                   "bootstrap the office filesystem from bundled templates",
    "doctor":                 "read-only sanity check (SETTINGS, RATES, counter, YTD)",
    "onboard-client":         "sign a new client (slug, day_rate_agreed, terms)",
    "log-hours":              "upsert a KW row on a project's _project-tracker.md",
    "generate-invoice":       "allocate INV-YYYY-NNN atomically, render, book",
    "track-expense":          "append a row to the right section of EXPENSES.md",
    "pipeline":               "read-only pipeline table (supports --json)",
    "kleinunternehmer-check": "local tax threshold check (§19 UStG DE / Monotributo AR / Autonomo ES)",
    "generate-contract":      "render Projektvereinbarung (v1 / v2 / v3 ...)",
    "dashboard":              "pipeline + this week + next invoice + threshold + todos",
    "office":                 "multi-office registry (list / add / remove / use / show)",
}

# Subcommands that should NOT auto-run the v0.1 -> v0.2 migration at entry.
# `office` needs to be reachable on a fresh machine before any migration has
# happened — migration uses the registry, which office commands manage.
_NO_MIGRATE_ON_ENTRY = frozenset({"office"})

# Subcommands that should NOT accept the --office selector (they manage the
# registry itself or don't touch any office's filesystem).
_NO_OFFICE_FLAG = frozenset({"office"})


def _lazy_run(name: str) -> Callable[[argparse.Namespace], Dict[str, Any]]:
    module_name = "src.commands." + name.replace("-", "_")

    def _run(args: argparse.Namespace) -> Dict[str, Any]:
        module = import_module(module_name)
        return module.run(args)

    return _run


def _add_common(parser: argparse.ArgumentParser, with_office: bool = True,
                is_toplevel: bool = False) -> None:
    # On the top-level parser we keep argparse defaults so `args.office`
    # exists even when flag-less. On subparsers we use SUPPRESS so that
    # a missing subparser-level `--office` does NOT overwrite a top-level
    # `--office` value. Same for --json / --dry-run so subparsers can
    # be invoked either position-wise.
    default = None if is_toplevel else argparse.SUPPRESS
    parser.add_argument("--json", action="store_true", default=(False if is_toplevel else argparse.SUPPRESS),
                        help="emit JSON envelope on stdout")
    parser.add_argument("--dry-run", action="store_true", default=(False if is_toplevel else argparse.SUPPRESS),
                        help="preview writes, do not touch disk")
    if with_office:
        parser.add_argument(
            "--office",
            default=default,
            help="target office id (defaults to registry default or the single registered office)",
        )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="freelance-office",
        description="Makakoo freelance-office skill (DE + AR + ES + US freelance accounting).",
    )
    _add_common(parser, is_toplevel=True)

    sub = parser.add_subparsers(dest="command", required=True, metavar="<subcommand>")
    for name, summary in SUBCOMMANDS.items():
        sp = sub.add_parser(name, help=summary)
        _add_common(sp, with_office=(name not in _NO_OFFICE_FLAG), is_toplevel=False)
        sp.set_defaults(func=_lazy_run(name), _cmd_name=name)
        # Each command module may expose add_arguments(parser) to register
        # its own flags. We import lazily here to keep `--help` cheap; if
        # the module is missing or raises, we fall back to a bare parser.
        try:
            module = import_module("src.commands." + name.replace("-", "_"))
            if hasattr(module, "add_arguments"):
                module.add_arguments(sp)
        except Exception:
            continue
    return parser


def main(argv: Optional[List[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    # v0.1 → v0.2 migration runs on every entry EXCEPT office-registry
    # commands (which manage the registry themselves). Idempotent +
    # lock-protected — safe to call on every invocation.
    if args._cmd_name not in _NO_MIGRATE_ON_ENTRY:
        try:
            from src.core.migration import ensure_v02
            ensure_v02()
        except Exception as e:
            # Migration failure is non-fatal — print to stderr and continue.
            print(f"[freelance-office] migration warning: {e}", file=sys.stderr)

    envelope = args.func(args)
    envelope.setdefault("schema_version", 1)
    envelope.setdefault("command", args._cmd_name)

    if getattr(args, "json", False):
        json.dump(envelope, sys.stdout, indent=2, ensure_ascii=False, default=str)
        sys.stdout.write("\n")
    else:
        status = envelope.get("status", "ok")
        message = envelope.get("message", "")
        print(f"freelance-office {envelope.get('command')}: {status}")
        if message:
            print(message)

    return int(envelope.get("exit_code", 0))


if __name__ == "__main__":
    sys.exit(main())
