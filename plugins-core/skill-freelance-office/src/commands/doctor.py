"""freelance-office doctor — read-only sanity check.

Verifies: ~/freelance-office/ exists, _meta files parse, invoice
counter is consistent with on-disk files, YTD counts are sane.

Exit code: 0 when all checks pass, 1 when any red check fires.
"""
from __future__ import annotations

from datetime import date
from pathlib import Path
from typing import Any, Dict, List

from ..core import earnings, expenses, invoice_counter, paths, settings
from ..core.errors import FreelanceError


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    checks: List[Dict[str, Any]] = []
    red = False

    if not home.is_dir():
        return {
            "status": "red",
            "exit_code": 1,
            "home": str(home),
            "checks": [{"name": "home", "ok": False, "detail": f"{home} does not exist"}],
            "message": f"{home} does not exist — run `freelance-office init`.",
        }
    checks.append({"name": "home", "ok": True, "detail": str(home)})

    try:
        s = settings.load_settings_at(home)
        checks.append({
            "name": "SETTINGS.yaml",
            "ok": True,
            "detail": (
                f"office={s.office.id} ({s.office.country}) "
                f"identity.name={s.identity.name!r} kleinunternehmer={s.tax.kleinunternehmer}"
            ),
        })
    except FreelanceError as e:
        red = True
        checks.append({"name": "SETTINGS.yaml", "ok": False, "detail": str(e)})
        s = None  # type: ignore

    try:
        r = settings.load_rates_at(home)
        floor = r.floor_day_rate
        checks.append({
            "name": "RATES.yaml",
            "ok": True,
            "detail": f"day_rates={sorted(r.day_rates.items())} floor={floor}",
        })
    except FreelanceError as e:
        red = True
        checks.append({"name": "RATES.yaml", "ok": False, "detail": str(e)})

    year = date.today().year
    counter_path = invoice_counter.counter_data_path(year, home)
    on_disk_highest = invoice_counter._highest_on_disk(year, home)  # noqa: SLF001
    counter_value = invoice_counter.peek(year, home)
    counter_ok = counter_value >= on_disk_highest
    if not counter_ok:
        red = True
    checks.append({
        "name": "invoice_counter",
        "ok": counter_ok,
        "detail": (
            f"last_number={counter_value} disk_max={on_disk_highest} "
            f"file={'present' if counter_path.is_file() else 'missing-seed-from-disk'}"
        ),
    })

    active_clients: List[str] = []
    clients = paths.clients_dir_for(home)
    if clients.is_dir():
        for p in clients.iterdir():
            if p.is_dir() and not p.name.startswith("_"):
                active_clients.append(p.name)
    checks.append({"name": "active_clients", "ok": True, "detail": f"{len(active_clients)} ({active_clients})"})

    earn_ytd = earnings.ytd_total(year, home) if not red else 0.0
    exp_ytd = expenses.ytd_by_category(year, home).get("__total__", 0.0) if not red else 0.0
    checks.append({"name": "finances_ytd", "ok": True, "detail": f"earnings={earn_ytd} expenses={exp_ytd}"})

    ku_detail = ""
    if s is not None and s.tax.kleinunternehmer:
        pct = 0.0 if earn_ytd == 0 else round(earn_ytd / 22000.0 * 100, 1)
        ku_detail = f"§19 UStG: €{earn_ytd}/€22000 ({pct}%)"
    else:
        ku_detail = "regular VAT regime"
    checks.append({"name": "kleinunternehmer", "ok": True, "detail": ku_detail})

    message = "all green" if not red else "red checks present — see details above"
    return {
        "status": "red" if red else "ok",
        "exit_code": 1 if red else 0,
        "home": str(home),
        "year": year,
        "checks": checks,
        "message": message,
    }
