"""freelance-office track-expense — append a row to the right EXPENSES.md section."""
from __future__ import annotations

from datetime import date, datetime
from typing import Any, Dict

from ..core import brain, expenses, paths
from ..core.errors import FreelanceError

CATEGORIES = list(expenses.CATEGORY_HEADERS.keys())


def add_arguments(parser):
    parser.add_argument("--date", default=str(date.today()), help="YYYY-MM-DD (default: today)")
    parser.add_argument("--amount-net", type=float, required=True, help="EUR net")
    parser.add_argument("--ust", type=float, default=0.0, help="VAT amount in EUR (default 0)")
    parser.add_argument("--category", choices=CATEGORIES, required=True)
    parser.add_argument("--description", required=True)
    parser.add_argument("--receipt-ref", default="_________")


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    try:
        d = datetime.strptime(args.date, "%Y-%m-%d").date()
    except ValueError as e:
        raise FreelanceError(f"--date must be YYYY-MM-DD: {e}") from e

    year = d.year
    rec = expenses.ExpenseRecord(
        date=args.date,
        description=args.description,
        category=args.category,
        net=round(float(args.amount_net), 2),
        ust=round(float(args.ust), 2),
        beleg=args.receipt_ref,
    )
    dry = bool(getattr(args, "dry_run", False))

    warnings = []
    if args.category == "equipment" and rec.net >= 800:
        warnings.append("equipment ≥ €800 → AfA (3-Jahres-Abschreibung) applies; add note in EÜR.")

    if dry:
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "would_append": {
                "year": year, "date": args.date, "category": args.category,
                "net": rec.net, "ust": rec.ust, "description": args.description,
            },
            "warnings": warnings,
            "message": f"dry-run: would log €{rec.net} [{args.category}] on {args.date}",
        }

    path, by_cat = expenses.append_expense(year, rec, home)
    cat_ytd = by_cat.get(args.category, 0.0)
    if args.category == "homeoffice" and cat_ytd > 600:
        warnings.append(f"homeoffice YTD €{cat_ytd:.2f} > €600 — over the annual cap.")

    journal_line = (
        f"Expense logged: €{rec.net} {args.category} ({args.description}). "
        f"YTD {args.category}: €{cat_ytd}. [[freelance-office]]"
    )
    try:
        journaled = brain.append_journal_line(journal_line)
    except FreelanceError as e:
        warnings.append(f"brain journal failed: {e}")
        journaled = None

    return {
        "status": "ok",
        "exit_code": 0,
        "path": str(path),
        "category": args.category,
        "net": rec.net,
        "ust": rec.ust,
        "ytd_by_category": by_cat,
        "journal": journaled,
        "warnings": warnings,
        "message": f"Logged €{rec.net} [{args.category}]. YTD {args.category}: €{cat_ytd}.",
    }
