"""freelance-office kleinunternehmer-check — per-country threshold check.

Routes through the office's country tax regime. DE's behavior is
identical to v0.1 (§19 UStG, €22.000 limit, 80% warn, 100% exit 2).
AR, ES, US (and any future country) implement their own semantics.
"""
from __future__ import annotations

import sys
from datetime import date
from typing import Any, Dict

from ..core import earnings, paths, settings
from ..core.tax import get_regime


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    s = settings.load_settings_at(home)
    year = date.today().year
    regime = get_regime(s.office.country)

    ytd = earnings.ytd_total(year, home)
    status = regime.check_threshold(s, ytd)

    # v0.1 compatibility: kleinunternehmer-check with kleinunternehmer=false
    # returns applicable=False. The "n/a" level maps to that.
    applicable = status.level != "n/a"

    # stderr warning for yellow — matches v0.1's print-to-stderr behavior
    if status.level == "yellow":
        print(status.message, file=sys.stderr)

    return {
        "status": _status_label(status.level, applicable),
        "exit_code": status.exit_code,
        "applicable": applicable,
        "ytd_net": status.ytd_net,
        "limit": status.limit,
        "pct_used": status.pct_used,
        "year": year,
        "country": s.office.country,
        "message": status.message,
    }


def _status_label(level: str, applicable: bool) -> str:
    # v0.1 returned "ok" when applicable=False; when applicable=True it
    # returned "green"/"yellow"/"red". Preserve that shape.
    if not applicable:
        return "ok"
    return level
