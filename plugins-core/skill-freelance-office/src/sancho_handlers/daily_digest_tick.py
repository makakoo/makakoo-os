"""SANCHO handler — daily 09:00 summary of overdue + threshold
activity.

Catches the case where the single per-event ping fired at 02:00
while Sebastian was asleep. Reads the two sibling state files,
diffs against the "last digest sent" marker, and fires ONE
Telegram if anything new happened in the last 24h. Zero-activity
days = zero messages.
"""
from __future__ import annotations

from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from ..core import brain
from . import _state
from ._telegram import telegram_send

STATE_FILE = "digest_state.json"

_JOURNAL_FN: Callable[[str], Any] = brain.append_journal_line


def set_journal_fn(fn: Callable[[str], Any]) -> None:
    global _JOURNAL_FN
    _JOURNAL_FN = fn


def _parse_date(s: str) -> Optional[date]:
    try:
        return datetime.strptime(s, "%Y-%m-%d").date()
    except (TypeError, ValueError):
        return None


def _overdue_activity_since(cutoff: date) -> List[Dict[str, Any]]:
    state = _state.load("overdue_notifications.json")
    out = []
    for inv_no, rec in state.items():
        last = _parse_date(rec.get("last_notified", ""))
        if last is None or last < cutoff:
            continue
        out.append({
            "inv_no": inv_no,
            "last_notified": str(last),
            "office_id": rec.get("office_id", ""),
            "amount": float(rec.get("amount", 0)),
        })
    return out


def _threshold_activity_since(cutoff: date) -> List[Dict[str, Any]]:
    state = _state.load("threshold_notifications.json")
    out = []
    for office_id, years in state.items():
        if not isinstance(years, dict):
            continue
        for year, ys in years.items():
            for marker, level in (
                ("fired_yellow_at", "yellow"),
                ("fired_red_at", "red"),
            ):
                when = _parse_date(ys.get(marker, ""))
                if when is None or when < cutoff:
                    continue
                out.append({
                    "office_id": office_id,
                    "year": year,
                    "level": level,
                    "fired_at": str(when),
                })
    return out


def tick(
    ctx: Optional[Dict[str, Any]] = None,
    *,
    today: Optional[date] = None,
) -> Dict[str, Any]:
    today = today or date.today()
    cutoff = today - timedelta(days=1)
    overdue = _overdue_activity_since(cutoff)
    thresh = _threshold_activity_since(cutoff)

    digest_state = _state.load(STATE_FILE)
    last_digest = _parse_date(digest_state.get("last_digest_sent", ""))
    summary = {
        "handler": "freelance_daily_digest_tick",
        "today": str(today),
        "overdue_events": overdue,
        "threshold_events": thresh,
        "fired": False,
        "message": None,
    }

    if not overdue and not thresh:
        return summary

    # Don't double-send on the same day — once per day max.
    if last_digest == today:
        return summary

    parts: List[str] = []
    if overdue:
        amount_sum = sum(o["amount"] for o in overdue)
        invs = ", ".join(o["inv_no"] for o in overdue)
        parts.append(
            f"{len(overdue)} overdue invoices totalling €{amount_sum:,.2f} "
            f"({invs})"
        )
    if thresh:
        by_level: Dict[str, List[str]] = {"yellow": [], "red": []}
        for t in thresh:
            by_level.setdefault(t["level"], []).append(t["office_id"])
        if by_level["yellow"]:
            parts.append(
                f"{len(by_level['yellow'])} offices crossed 80% "
                f"({', '.join(by_level['yellow'])})"
            )
        if by_level["red"]:
            parts.append(
                f"{len(by_level['red'])} offices crossed 100% "
                f"({', '.join(by_level['red'])})"
            )

    message = "[[freelance-office]] daily digest: " + "; ".join(parts) + "."
    try:
        telegram_send(message)
    except Exception:
        pass
    try:
        _JOURNAL_FN(message)
    except Exception:
        pass

    digest_state["last_digest_sent"] = str(today)
    _state.save(STATE_FILE, digest_state)
    summary["fired"] = True
    summary["message"] = message
    return summary
