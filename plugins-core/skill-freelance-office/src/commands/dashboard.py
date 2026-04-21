"""freelance-office dashboard — union of pipeline + hours + next invoice + KU + todos."""
from __future__ import annotations

from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any, Dict, List

from ..core import client_meta, earnings, paths, settings


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    s = settings.load_settings_at(home)
    year = date.today().year

    # Revenue YTD
    earnings_ytd = earnings.ytd_total(year, home)

    # Hours logged this week (KW) — approximate by scanning all trackers
    iso_year, iso_week, _ = date.today().isocalendar()
    hours_this_week = 0.0
    clients_dir = home / "clients"
    active_clients = 0
    next_due: Dict[str, Any] = {}
    todos: List[str] = []

    if clients_dir.is_dir():
        for cdir in sorted(clients_dir.iterdir()):
            if not cdir.is_dir() or cdir.name.startswith("_"):
                continue
            active_clients += 1
            meta_path = cdir / "meta.yaml"
            if meta_path.is_file():
                try:
                    m = client_meta.ClientMeta.load(meta_path).flat()
                    if m.get("current_status") == "prospecting":
                        todos.append(f"[[{cdir.name}]] still in prospecting — sign a contract?")
                except Exception:
                    pass
            projects_dir = cdir / "projects"
            if not projects_dir.is_dir():
                continue
            for pdir in projects_dir.iterdir():
                if not pdir.is_dir() or pdir.name.startswith("_"):
                    continue
                tracker_path = pdir / "_project-tracker.md"
                if tracker_path.is_file():
                    hours_this_week += _hours_in_kw(tracker_path, iso_week)
                invoices_dir = pdir / "invoices"
                if invoices_dir.is_dir():
                    for f in invoices_dir.glob("INV-*.md"):
                        # Use earnings row for due_date if present
                        pass

    # Next invoice due: scan earnings rows for unpaid, pick the earliest issued + 30d
    for rec in earnings._iter_rows(_safe_read(earnings.earnings_path(year, home))):
        st = str(rec.get("status") or "")
        if "offen" in st.lower() or "⏳" in st:
            try:
                d = datetime.strptime(rec["date"], "%Y-%m-%d").date()
            except ValueError:
                continue
            due = d + timedelta(days=30)
            if not next_due or due < next_due["due"]:
                next_due = {"inv_no": rec["inv_no"], "net": rec["net"], "due": due}
                if (due - date.today()).days < 0:
                    todos.append(
                        f"[[{rec['inv_no']}]] overdue since {due} (€{rec['net']} net)"
                    )

    ku_progress: Dict[str, Any] = {"applicable": bool(s.tax.kleinunternehmer)}
    if s.tax.kleinunternehmer:
        pct = round(earnings_ytd / 22000.0 * 100, 2)
        ku_progress.update({"ytd_net": earnings_ytd, "limit": 22000, "pct_used": pct})
        if pct >= 80:
            todos.append(f"Kleinunternehmer at {pct}% — plan VAT regime switch")

    if active_clients == 0:
        message = (
            f"No clients yet. `freelance-office onboard-client --slug <slug> "
            "--name '...' --day-rate <EUR>` to start."
        )
    else:
        message = (
            f"{active_clients} clients. €{earnings_ytd:.2f} net YTD. "
            f"{hours_this_week:.1f}h logged KW{iso_week:02d}."
        )

    return {
        "status": "ok",
        "exit_code": 0,
        "year": year,
        "iso_week": iso_week,
        "active_clients": active_clients,
        "earnings_ytd": earnings_ytd,
        "hours_this_week": hours_this_week,
        "next_invoice_due": {**next_due, "due": str(next_due["due"])} if next_due else None,
        "kleinunternehmer": ku_progress,
        "todos": todos[:3],
        "message": message,
    }


def _safe_read(p: Path) -> str:
    if not p.is_file():
        return ""
    return p.read_text(encoding="utf-8")


def _hours_in_kw(tracker_path: Path, kw: int) -> float:
    try:
        text = tracker_path.read_text(encoding="utf-8")
    except OSError:
        return 0.0
    for line in text.splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) < 9:
            continue
        try:
            row_kw = int(cells[0])
        except ValueError:
            continue
        if row_kw != kw:
            continue
        total = 0.0
        for c in cells[1:8]:
            if c == "":
                continue
            try:
                total += float(c)
            except ValueError:
                pass
        return total
    return 0.0
