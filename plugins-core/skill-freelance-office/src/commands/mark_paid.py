"""freelance-office mark-paid — flip an invoice's paid status.

Two-phase verify (pi corruption-risk #2): after we touch both
EARNINGS.md and the project tracker, the tracker is re-read and
the expected checkbox marker is asserted. A dropped tracker write
leaves EARNINGS.md updated and the tracker not — the worst split
state — so we surface it loudly instead of letting it slip.

Partial payments: passing ``--amount <N<net>`` books a tranche
and flips status to ``💰 teilweise``. Subsequent calls with
``--amount`` accumulate against ``net``; when the cumulative total
reaches or exceeds ``net``, status auto-flips to ``✅ bezahlt``.
Over-paying errors cleanly.
"""
from __future__ import annotations

from datetime import date, datetime
from pathlib import Path
from typing import Any, Dict, Optional

from ..core import brain, earnings, paths, tracker
from ..core.errors import FreelanceError, NotInitialisedError


def add_arguments(parser):
    parser.add_argument("--invoice", required=True, help="INV-YYYY-NNN")
    parser.add_argument("--paid-date", default=None, help="YYYY-MM-DD (defaults to today)")
    parser.add_argument(
        "--amount",
        type=float,
        default=None,
        help="tranche amount for partial payments; omit for full balance",
    )
    parser.add_argument(
        "--bank-entry-date",
        default=None,
        help="YYYY-MM-DD; defaults to --paid-date",
    )


def _find_invoice_file(home: Path, inv_no: str) -> Optional[Path]:
    """Locate ``clients/*/projects/*/invoices/<inv>.md`` under the
    office root. Returns ``None`` when no file matches (legal case
    for an EARNINGS row added by ``reconcile`` in the future)."""
    matches = list((home / "clients").glob(f"*/projects/*/invoices/{inv_no}.md"))
    if len(matches) > 1:
        raise FreelanceError(
            f"multiple invoice files match {inv_no}: "
            + ", ".join(str(m) for m in matches)
        )
    return matches[0] if matches else None


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    inv_no = args.invoice
    if not inv_no.startswith("INV-"):
        raise FreelanceError(
            f"invoice number must start with INV-YYYY-NNN; got {inv_no!r}"
        )
    try:
        year = int(inv_no.split("-")[1])
    except (IndexError, ValueError) as e:
        raise FreelanceError(f"cannot parse year from {inv_no!r}") from e

    paid_str = args.paid_date or str(date.today())
    try:
        datetime.strptime(paid_str, "%Y-%m-%d")
    except ValueError as e:
        raise FreelanceError(f"--paid-date must be YYYY-MM-DD: {e}") from e
    bank_date = args.bank_entry_date or paid_str
    try:
        datetime.strptime(bank_date, "%Y-%m-%d")
    except ValueError as e:
        raise FreelanceError(f"--bank-entry-date must be YYYY-MM-DD: {e}") from e

    dry = bool(getattr(args, "dry_run", False))
    if dry:
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "invoice_number": inv_no,
            "paid_date": paid_str,
            "bank_entry_date": bank_date,
            "message": f"dry-run: would mark {inv_no} paid on {paid_str}",
        }

    # 1. Update EARNINGS.md + append Zahlungseingänge row.
    result = earnings.mark_row_paid(
        year,
        inv_no,
        paid_str,
        amount=args.amount,
        bank_entry_date=bank_date,
        root=home,
    )

    # Idempotent no-op path: row was already paid, no amount given.
    if result.already_paid:
        return {
            "status": "ok",
            "exit_code": 0,
            "invoice_number": inv_no,
            "amount_paid": 0.0,
            "accumulated": result.accumulated,
            "balance": 0.0,
            "paid_status": result.status,
            "bank_entry_date": bank_date,
            "journal": None,
            "message": (
                f"Invoice [[{inv_no}]] was already paid in full "
                f"(€{result.accumulated:.2f}) — no change."
            ),
        }

    # 2. Update project tracker's Bezahlt cell.
    invoice_file = _find_invoice_file(home, inv_no)
    tracker_verified = None
    if invoice_file is not None:
        tracker_path = invoice_file.parent.parent / "_project-tracker.md"
        if tracker_path.is_file():
            t = tracker.Tracker.load(tracker_path)
            updated = t.mark_invoice_paid(inv_no, partial=not result.fully_paid)
            if updated:
                t.write()
            # Two-phase verify: re-read from disk and confirm.
            t2 = tracker.Tracker.load(tracker_path)
            tracker_verified = t2.verify_invoice_paid_marker(
                inv_no, partial=not result.fully_paid
            )
            if updated and not tracker_verified:
                raise FreelanceError(
                    "tracker write dropped — EARNINGS.md updated, tracker not. "
                    "Re-run mark-paid to retry."
                )

    # 3. Brain journal.
    outstanding = max(0.0, result.balance)
    line = (
        f"Marked [[{inv_no}]] paid — €{result.amount_paid:,.2f} net on {paid_str}. "
        f"Outstanding: €{outstanding:,.2f}. [[freelance-office]]"
    )
    journaled = None
    try:
        journaled = brain.append_journal_line(line)
    except FreelanceError as e:
        # Non-fatal: ledger is updated regardless.
        journaled = None

    return {
        "status": "ok",
        "exit_code": 0,
        "invoice_number": inv_no,
        "amount_paid": result.amount_paid,
        "accumulated": result.accumulated,
        "balance": outstanding,
        "net": result.net,
        "paid_status": result.status,
        "fully_paid": result.fully_paid,
        "bank_entry_date": bank_date,
        "tracker_verified": tracker_verified,
        "journal": journaled,
        "message": (
            f"{inv_no}: applied €{result.amount_paid:,.2f} → status {result.status}. "
            f"Balance €{outstanding:,.2f}."
        ),
    }
