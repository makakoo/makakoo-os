"""freelance-office pipeline — live read-only pipeline table."""
from __future__ import annotations

from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any, Dict, List

from ..core import client_meta, earnings, paths


def add_arguments(parser):
    parser.add_argument("--status", default=None, help="filter by client current_status")


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    rows: List[Dict[str, Any]] = []
    totals = {"invoiced_net": 0.0, "paid_net": 0.0, "outstanding_net": 0.0, "overdue_net": 0.0}

    clients_dir = home / "clients"
    year = date.today().year
    # pre-index invoices by client from the current-year EARNINGS
    earn_by_inv: Dict[str, Dict[str, Any]] = {}
    for rec in earnings._iter_rows(_safe_read(earnings.earnings_path(year, home))):
        earn_by_inv[rec["inv_no"]] = rec

    if not clients_dir.is_dir():
        return _envelope(rows, totals, message="no clients dir")

    for cdir in sorted(clients_dir.iterdir()):
        if not cdir.is_dir() or cdir.name.startswith("_"):
            continue
        meta_path = cdir / "meta.yaml"
        if not meta_path.is_file():
            continue
        try:
            meta = client_meta.ClientMeta.load(meta_path).flat()
        except Exception:
            continue
        status = str(meta.get("current_status") or "").lower()
        if args.status and status != args.status.lower():
            continue
        rate = meta.get("day_rate_agreed")
        projects_dir = cdir / "projects"
        proj_list = []
        if projects_dir.is_dir():
            for pdir in sorted(projects_dir.iterdir()):
                if pdir.is_dir() and not pdir.name.startswith("_"):
                    proj_list.append(pdir.name)
        if not proj_list:
            rows.append({
                "client": cdir.name,
                "project": None,
                "status": status or "prospecting",
                "day_rate": rate,
                "invoiced_net": 0.0,
                "paid_net": 0.0,
                "outstanding_net": 0.0,
                "overdue_net": 0.0,
            })
            continue
        for proj in proj_list:
            invoiced = paid = outstanding = overdue = 0.0
            invoices_dir = cdir / "projects" / proj / "invoices"
            if invoices_dir.is_dir():
                for f in invoices_dir.glob("INV-*.md"):
                    inv_no = f.stem
                    rec = earn_by_inv.get(inv_no)
                    if not rec:
                        continue
                    net = rec["net"]
                    invoiced += net
                    st = str(rec.get("status") or "").lower()
                    if "bezahlt" in st or "paid" in st or "✅" in st:
                        paid += net
                    else:
                        outstanding += net
                        try:
                            d = datetime.strptime(rec["date"], "%Y-%m-%d").date()
                            if date.today() - d > timedelta(days=30):
                                overdue += net
                        except ValueError:
                            pass
            rows.append({
                "client": cdir.name,
                "project": proj,
                "status": status or "active",
                "day_rate": rate,
                "invoiced_net": round(invoiced, 2),
                "paid_net": round(paid, 2),
                "outstanding_net": round(outstanding, 2),
                "overdue_net": round(overdue, 2),
            })
            totals["invoiced_net"] += invoiced
            totals["paid_net"] += paid
            totals["outstanding_net"] += outstanding
            totals["overdue_net"] += overdue

    for k in totals:
        totals[k] = round(totals[k], 2)
    return _envelope(rows, totals)


def _safe_read(p: Path) -> str:
    if not p.is_file():
        return ""
    return p.read_text(encoding="utf-8")


def _envelope(rows, totals, message: str = "") -> Dict[str, Any]:
    return {
        "status": "ok",
        "exit_code": 0,
        "generated_at": datetime.utcnow().isoformat() + "Z",
        "rows": rows,
        "totals": totals,
        "message": message or f"{len(rows)} rows. invoiced €{totals['invoiced_net']:.2f} / outstanding €{totals['outstanding_net']:.2f} / overdue €{totals['overdue_net']:.2f}",
    }
