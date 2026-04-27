"""SANCHO handler — nag about overdue invoices.

Every 24 hours we walk every registered office, scan the current
year's EARNINGS.md for rows whose status is still open past their
payment-terms + grace window, and:

1. Log one Brain journal line per overdue invoice per day
   (idempotent: ``state/overdue_notifications.json`` remembers the
   last-notified date per inv_no).
2. Fire a Telegram ping when the net amount exceeds
   ``SETTINGS.finance.overdue_ping_floor`` (default €500).

Hazards the pi review surfaced:

- **Race** (pi blocker #2): between reading the status cell and
  firing Telegram, a concurrent ``mark-paid`` in another terminal
  could flip the row. Mitigation: read-then-confirm — re-read the
  status immediately before ``telegram_send``. If the row is now
  paid, skip the ping.
- **Orphan purge** (pi corruption-risk #3): an invoice whose file
  was deleted (or rebuilt under a new number) leaves a stale
  notification entry that nags forever. Mitigation: every tick
  drops entries whose inv_no no longer appears in any year's
  EARNINGS.md.
"""
from __future__ import annotations

from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, List, Optional

import os

from ..core import brain, earnings, paths, settings
from ..core.registry import OfficeEntry, OfficeRegistry
from . import _state
from ._telegram import telegram_send

STATE_FILE = "overdue_notifications.json"

_JOURNAL_FN: Callable[[str], Any] = brain.append_journal_line


def set_journal_fn(fn: Callable[[str], Any]) -> None:
    """Tests override this to capture journal lines."""
    global _JOURNAL_FN
    _JOURNAL_FN = fn


def _iter_offices() -> List[OfficeEntry]:
    """Return every freelance office to scan — the registry, or
    (when the registry is empty) the single home resolved via
    :func:`paths.resolve_office_root`. Mirrors the fallback chain
    the rest of the plugin uses so SANCHO handlers see the same
    offices the CLI does."""
    registry = OfficeRegistry.load()
    if registry.offices:
        return list(registry.offices.values())
    # Registry empty → v0.1 single-home mode.
    fallback = os.environ.get("FREELANCE_OFFICE_HOME")
    root = Path(fallback).expanduser() if fallback else Path.home() / "freelance-office"
    if not root.is_dir():
        return []
    return [OfficeEntry(id="default", path=root, country="DE", added="")]


def _parse_date(s: str) -> Optional[date]:
    try:
        return datetime.strptime(s, "%Y-%m-%d").date()
    except (TypeError, ValueError):
        return None


def _status_is_open(status: str) -> bool:
    s = (status or "").lower()
    if not s:
        return True
    # "teilweise bezahlt" is still open for overdue purposes — the
    # client owes the balance. Only ``✅ bezahlt`` (without
    # "teilweise") exits the overdue funnel.
    if "teilweise" in s:
        return True
    return "bezahlt" not in s and "pagad" not in s and "paid" not in s


def _known_invoice_ids(office_root: Path) -> Iterable[str]:
    """Yield every inv_no across every finances/<year>/EARNINGS.md
    under ``office_root`` — used by the orphan-purge pass."""
    finances = office_root / "finances"
    if not finances.is_dir():
        return
    for year_dir in finances.iterdir():
        try:
            yr = int(year_dir.name)
        except ValueError:
            continue
        for rec in earnings.iter_rows(yr, office_root):
            yield rec["inv_no"]


def _find_row(office_root: Path, inv_no: str) -> Optional[Dict[str, Any]]:
    """Locate the EARNINGS row for ``inv_no`` via its year prefix."""
    try:
        year = int(inv_no.split("-")[1])
    except (IndexError, ValueError):
        return None
    for rec in earnings.iter_rows(year, office_root):
        if rec["inv_no"] == inv_no:
            return rec
    return None


def tick(
    ctx: Optional[Dict[str, Any]] = None,
    *,
    today: Optional[date] = None,
) -> Dict[str, Any]:
    """Run one overdue pass. Returns a summary dict for SANCHO
    logging. ``today`` is injectable so tests can pin the date."""
    today = today or date.today()

    state = _state.load(STATE_FILE)

    # Step 1 — orphan purge. Every entry whose inv_no is no longer
    # present in any registered office's EARNINGS gets dropped.
    offices = _iter_offices()
    live_ids: set = set()
    for office in offices:
        live_ids.update(_known_invoice_ids(office.path))
    orphans = [inv_no for inv_no in list(state.keys()) if inv_no not in live_ids]
    for inv_no in orphans:
        state.pop(inv_no, None)

    # Step 2 — scan for overdue invoices.
    fired: List[str] = []
    pinged: List[str] = []
    for office in offices:
        try:
            s = settings.load_settings_at(office.path)
        except Exception:
            continue
        grace = int(getattr(s.finance, "overdue_grace_days", 7))
        floor = float(getattr(s.finance, "overdue_ping_floor", 500.0))
        terms = int(getattr(s.finance, "payment_terms_days", 30))
        for year_dir in sorted((office.path / "finances").glob("*")):
            try:
                yr = int(year_dir.name)
            except ValueError:
                continue
            for rec in earnings.iter_rows(yr, office.path):
                if not _status_is_open(rec["status"]):
                    continue
                issued = _parse_date(rec["date"])
                if issued is None:
                    continue
                grace_end = issued + timedelta(days=terms + grace)
                if today <= grace_end:
                    continue
                # One fire per invoice per day.
                last = state.get(rec["inv_no"], {}).get("last_notified")
                if last == str(today):
                    continue
                days_overdue = (today - grace_end).days
                line = (
                    f"[[{rec['inv_no']}]] ist {days_overdue} Tage überfällig — "
                    f"€{rec['net']:.2f} offen. [[freelance-office]]"
                )
                try:
                    _JOURNAL_FN(line)
                except Exception:
                    pass
                fired.append(rec["inv_no"])

                # Read-then-confirm (pi blocker #2): re-read this
                # invoice's status right before the Telegram send. A
                # concurrent ``mark-paid`` in another terminal
                # cancels the ping.
                if rec["net"] >= floor:
                    fresh = _find_row(office.path, rec["inv_no"])
                    if fresh is not None and _status_is_open(fresh["status"]):
                        try:
                            telegram_send(line)
                        except Exception:
                            pass
                        pinged.append(rec["inv_no"])
                state[rec["inv_no"]] = {
                    "last_notified": str(today),
                    "office_id": office.id,
                    "amount": rec["net"],
                }

    _state.save(STATE_FILE, state)
    return {
        "handler": "freelance_invoice_overdue_tick",
        "today": str(today),
        "fired": fired,
        "pinged": pinged,
        "orphans_purged": orphans,
    }
