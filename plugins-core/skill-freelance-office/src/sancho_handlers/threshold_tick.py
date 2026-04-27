"""SANCHO handler — alert on Kleinunternehmer / Monotributo
threshold crossings.

For every registered office we ask its country module via
``get_regime(country).check_threshold(settings, ytd_net)`` what the
current level is and compare to the last-fired level stored per
``(office.id, year)``. We fire Telegram + a Brain journal line on:

- First transition ``green`` / ``n/a`` → ``yellow`` per office per year.
- First transition ``yellow`` → ``red`` per office per year.

State lives at
``$MAKAKOO_HOME/state/skill-freelance-office/threshold_notifications.json``
and is keyed by office id to guarantee rename isolation (pi gap #4):
renaming ``de-main`` to ``de-alt`` gives the new id a fresh
notification history.
"""
from __future__ import annotations

from datetime import date
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from ..core import brain, earnings, settings
from ..core.tax import get_regime
from . import _state
from ._telegram import telegram_send
from .overdue_tick import _iter_offices

STATE_FILE = "threshold_notifications.json"

_JOURNAL_FN: Callable[[str], Any] = brain.append_journal_line


def set_journal_fn(fn: Callable[[str], Any]) -> None:
    global _JOURNAL_FN
    _JOURNAL_FN = fn


def _year_of(today: date) -> str:
    return str(today.year)


def tick(
    ctx: Optional[Dict[str, Any]] = None,
    *,
    today: Optional[date] = None,
) -> Dict[str, Any]:
    today = today or date.today()
    year = _year_of(today)
    state = _state.load(STATE_FILE)

    fired: List[Dict[str, Any]] = []
    for office in _iter_offices():
        try:
            s = settings.load_settings_at(office.path)
        except Exception:
            continue
        ytd = earnings.ytd_total(int(year), root=office.path)
        try:
            regime = get_regime(office.country)
        except Exception:
            continue
        status = regime.check_threshold(s, ytd)

        office_state = state.setdefault(office.id, {})
        year_state = office_state.setdefault(year, {"last_level": "green"})

        prev = year_state.get("last_level", "green")

        def _record(level: str, field: str) -> None:
            year_state["last_level"] = level
            year_state[field] = str(today)
            line = (
                f"[[{office.id}]] crossed {level.upper()} at {ytd:.2f} / "
                f"{status.limit} ({status.pct_used}%). [[freelance-office]]"
            )
            try:
                _JOURNAL_FN(line)
            except Exception:
                pass
            try:
                telegram_send(line)
            except Exception:
                pass
            fired.append({
                "office_id": office.id,
                "year": year,
                "level": level,
                "ytd_net": ytd,
                "pct_used": status.pct_used,
                "limit": status.limit,
            })

        if status.level == "yellow" and prev in ("green", "n/a"):
            _record("yellow", "fired_yellow_at")
        elif status.level == "red" and prev in ("green", "yellow", "n/a"):
            _record("red", "fired_red_at")
        else:
            # Same or lower level — just remember the current.
            year_state["last_level"] = status.level

    _state.save(STATE_FILE, state)
    return {
        "handler": "freelance_threshold_tick",
        "today": str(today),
        "fired": fired,
    }
