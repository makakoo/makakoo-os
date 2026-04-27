"""freelance-office log-hours — upsert a KW row in _project-tracker.md."""
from __future__ import annotations

import json
from typing import Any, Dict

from ..core import brain, paths, tracker
from ..core.errors import FreelanceError, NotInitialisedError

DAY_ALIASES = {
    "mo": "Mo", "mon": "Mo", "monday": "Mo",
    "di": "Di", "tue": "Di", "tuesday": "Di",
    "mi": "Mi", "wed": "Mi", "wednesday": "Mi",
    "do": "Do", "thu": "Do", "thursday": "Do",
    "fr": "Fr", "fri": "Fr", "friday": "Fr",
    "sa": "Sa", "sat": "Sa", "saturday": "Sa",
    "so": "So", "sun": "So", "sunday": "So",
}


def add_arguments(parser):
    parser.add_argument("--client", required=True)
    parser.add_argument("--project", required=True)
    parser.add_argument("--week", type=int, required=True, help="ISO calendar week 1–53")
    parser.add_argument("--hours", default=None, help='JSON dict, e.g. \'{"Mo":8,"Di":8}\'')
    parser.add_argument("--day", default=None, help="day shorthand (mo/di/mi/do/fr/sa/so)")
    parser.add_argument("--hours-today", type=float, default=None)
    parser.add_argument("--note", default="")


def _build_hours(args) -> Dict[str, float]:
    if args.hours:
        try:
            raw = json.loads(args.hours)
        except json.JSONDecodeError as e:
            raise FreelanceError(f"--hours must be JSON: {e}") from e
        if not isinstance(raw, dict):
            raise FreelanceError("--hours must be a JSON object")
        out: Dict[str, float] = {}
        for k, v in raw.items():
            day = DAY_ALIASES.get(str(k).lower(), str(k))
            if day not in ("Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"):
                raise FreelanceError(f"unknown day key {k!r}")
            out[day] = float(v)
        return out
    if args.day and args.hours_today is not None:
        day = DAY_ALIASES.get(args.day.lower(), args.day)
        if day not in ("Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"):
            raise FreelanceError(f"unknown --day {args.day!r}")
        return {day: float(args.hours_today)}
    raise FreelanceError("provide --hours (JSON) or --day + --hours-today")


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    hours = _build_hours(args)
    if not (1 <= args.week <= 53):
        raise FreelanceError(f"--week must be 1..53, got {args.week}")
    tracker_path = (
        home / "clients" / args.client / "projects" / args.project
        / "_project-tracker.md"
    )
    if not tracker_path.is_file():
        raise NotInitialisedError(
            f"tracker missing at {tracker_path} — run `freelance-office generate-contract` "
            f"--client {args.client} --project {args.project} first"
        )

    dry = bool(getattr(args, "dry_run", False))
    t = tracker.Tracker.load(tracker_path)
    before_text = t.text

    t.update_hours(args.week, hours, note=args.note)
    total = sum(hours.values())

    if dry:
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "would_write_kw": args.week,
            "would_add_hours": hours,
            "total_hours": total,
            "bytes_changed": abs(len(t.text) - len(before_text)),
            "message": f"dry-run: would log {total}h on KW{args.week}",
        }

    t.write()
    # reload to get recomputed spent/remaining
    t2 = tracker.Tracker.load(tracker_path)
    journal_line = (
        f"Logged {total}h on [[{args.client}/{args.project}]] KW{args.week} "
        f"(remaining: {t2.remaining_days} days). [[freelance-office]]"
    )
    warnings = []
    try:
        journaled = brain.append_journal_line(journal_line)
    except FreelanceError as e:
        warnings.append(f"brain journal failed: {e}")
        journaled = None
    return {
        "status": "ok",
        "exit_code": 0,
        "client": args.client,
        "project": args.project,
        "week": args.week,
        "logged_hours": hours,
        "total_hours": total,
        "spent_days": t2.spent_days,
        "remaining_days": t2.remaining_days,
        "journal": journaled,
        "warnings": warnings,
        "message": f"Logged {total}h for KW{args.week}. {t2.remaining_days} days remaining.",
    }
