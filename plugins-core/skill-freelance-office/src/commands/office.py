"""freelance-office office — CRUD over the office registry.

Nested subcommands:

    office list         list all registered offices
    office add          register a new office
    office remove       unregister (does not delete the filesystem)
    office use <id>     set default office
    office show [--id]  show full config for one office
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict

from ..core import registry as reg_mod
from ..core.errors import FreelanceError


def add_arguments(parser):
    sub = parser.add_subparsers(dest="office_subcommand", required=True, metavar="<action>")

    p_list = sub.add_parser("list", help="list registered offices")
    p_list.add_argument("--json", action="store_true", help="emit JSON envelope")
    p_list.add_argument("--dry-run", action="store_true")

    p_add = sub.add_parser("add", help="register a new office")
    p_add.add_argument("--id", required=True, help="short id, e.g. de-main / ar-main")
    p_add.add_argument("--path", required=True, help="absolute path to the office root")
    p_add.add_argument("--country", default="DE", help="ISO-3166-1 alpha-2 code (default DE)")
    p_add.add_argument("--default", action="store_true", help="make this the default office")
    p_add.add_argument("--json", action="store_true")
    p_add.add_argument("--dry-run", action="store_true")

    p_rm = sub.add_parser("remove", help="unregister an office")
    p_rm.add_argument("--id", required=True)
    p_rm.add_argument("--json", action="store_true")
    p_rm.add_argument("--dry-run", action="store_true")

    p_use = sub.add_parser("use", help="set the default office")
    p_use.add_argument("id", help="office id")
    p_use.add_argument("--json", action="store_true")
    p_use.add_argument("--dry-run", action="store_true")

    p_show = sub.add_parser("show", help="show full config for one office")
    p_show.add_argument("--id", default=None, help="office id (defaults to registry default)")
    p_show.add_argument("--json", action="store_true")
    p_show.add_argument("--dry-run", action="store_true")


def run(args) -> Dict[str, Any]:
    sc = getattr(args, "office_subcommand", None)
    if sc == "list":
        return _list(args)
    if sc == "add":
        return _add(args)
    if sc == "remove":
        return _remove(args)
    if sc == "use":
        return _use(args)
    if sc == "show":
        return _show(args)
    raise FreelanceError(f"unknown office subcommand: {sc!r}")


def _list(args) -> Dict[str, Any]:
    reg = reg_mod.OfficeRegistry.load()
    rows = [
        {
            "id": e.id,
            "country": e.country,
            "path": str(e.path),
            "added": e.added,
            "default": e.id == reg.default,
        }
        for e in reg.offices.values()
    ]
    return {
        "status": "ok",
        "exit_code": 0,
        "default": reg.default,
        "offices": rows,
        "count": len(rows),
        "message": (
            f"{len(rows)} offices registered, default: {reg.default or '(none)'}"
            if rows
            else "no offices registered yet. use `office add` to register one."
        ),
    }


def _add(args) -> Dict[str, Any]:
    path = Path(args.path).expanduser().resolve()
    if getattr(args, "dry_run", False):
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "would_add": {"id": args.id, "path": str(path), "country": args.country.upper(),
                          "default": bool(args.default)},
            "message": f"dry-run: would register {args.id} ({args.country}) at {path}",
        }
    reg = reg_mod.OfficeRegistry.load()
    entry = reg.add(args.id, path, args.country, default=bool(args.default))
    return {
        "status": "ok",
        "exit_code": 0,
        "id": entry.id,
        "path": str(entry.path),
        "country": entry.country,
        "default": reg.default == entry.id,
        "message": f"registered {entry.id} ({entry.country}) at {entry.path}",
    }


def _remove(args) -> Dict[str, Any]:
    if getattr(args, "dry_run", False):
        reg = reg_mod.OfficeRegistry.load()
        if args.id not in reg.offices:
            raise reg_mod.UnknownOfficeError(
                reg_mod.OfficeRegistry._unknown_msg(reg, args.id)  # noqa: SLF001
            )
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "would_remove": args.id,
            "message": f"dry-run: would unregister {args.id} (filesystem untouched)",
        }
    reg = reg_mod.OfficeRegistry.load()
    reg.remove(args.id)
    return {
        "status": "ok",
        "exit_code": 0,
        "id": args.id,
        "new_default": reg.default,
        "message": (
            f"unregistered {args.id}. new default: {reg.default or '(none — no offices left)'}"
        ),
    }


def _use(args) -> Dict[str, Any]:
    if getattr(args, "dry_run", False):
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "would_set_default": args.id,
            "message": f"dry-run: would set default to {args.id}",
        }
    reg = reg_mod.OfficeRegistry.load()
    reg.set_default(args.id)
    return {
        "status": "ok",
        "exit_code": 0,
        "default": reg.default,
        "message": f"default office is now {reg.default}",
    }


def _show(args) -> Dict[str, Any]:
    reg = reg_mod.OfficeRegistry.load()
    target = args.id or reg.default
    if not target:
        raise FreelanceError("no office specified and no default set")
    if target not in reg.offices:
        raise reg_mod.UnknownOfficeError(
            reg_mod.OfficeRegistry._unknown_msg(reg, target)  # noqa: SLF001
        )
    e = reg.offices[target]
    settings_path = e.path / "_meta" / "SETTINGS.yaml"
    has_settings = settings_path.is_file()
    return {
        "status": "ok",
        "exit_code": 0,
        "id": e.id,
        "country": e.country,
        "path": str(e.path),
        "added": e.added,
        "is_default": reg.default == e.id,
        "has_settings_yaml": has_settings,
        "message": (
            f"{e.id} ({e.country}) at {e.path}"
            f"{' [default]' if reg.default == e.id else ''}"
            f"{'' if has_settings else ' — SETTINGS.yaml missing; run init'}"
        ),
    }
