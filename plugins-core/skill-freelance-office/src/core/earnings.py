"""Section-scoped mutation of ``finances/<YYYY>/EARNINGS.md``.

Format-fidelity contract: every header, every non-target table row,
and every piece of prose stays byte-identical. Only the target table
body + the recomputed ``Summe`` row + the ``Quartalszusammenfassung``
block move.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from typing import Dict, List, Optional, Tuple

from . import paths
from . import md_table
from .errors import FreelanceError

# Canonical column count for the Einnahmen-Übersicht table — any row
# with a different count is routed to the malformed-rows sentinel so
# that a pipe character inside a client name can never silently shift
# ``_parse_money(cells[5])`` onto a non-money cell. See
# :mod:`src.core.md_table` for rationale.
EARN_COLS = 9

# Section headers vary by country: DE uses "Einnahmen-Übersicht" and
# "Quartalszusammenfassung"; AR + ES use "Resumen de Ingresos" and
# "Resumen Trimestral". Regexes match all three locales.
EARN_HEADER_RE = re.compile(
    r"^## 💰 (?:Einnahmen-Übersicht|Resumen de Ingresos) \[.*?\]\s*$",
    re.MULTILINE,
)
QUART_HEADER_RE = re.compile(
    r"^## 📊 (?:Quartalszusammenfassung|Resumen Trimestral)\s*$",
    re.MULTILINE,
)
ZAHL_HEADER_RE = re.compile(
    r"^## 📅 (?:Zahlungseingänge|Cobros Recibidos|Pagos Recibidos)\s*$",
    re.MULTILINE,
)
NEXT_H2_RE = re.compile(r"^## ", re.MULTILINE)
# "Summe" (DE) vs "Suma" / "Total" (AR / ES)
SUMME_ROW_RE = re.compile(r"^\|.*\*\*(?:Summe|Suma|Total)\*\*.*\|$", re.MULTILINE)
DATA_ROW_RE = re.compile(r"^\|\s*(\d+)\s*\|\s*(INV-\d{4}-\d+|[^|]*)\s*\|")


@dataclass
class EarningRecord:
    inv_no: str
    client: str
    project: str
    issued: str  # YYYY-MM-DD
    net: float
    ust: float = 0.0
    status: str = "⏳ offen"

    @property
    def brutto(self) -> float:
        return round(self.net + self.ust, 2)


@dataclass
class PaidStatus:
    """Return value of :func:`mark_row_paid`.

    Fields:
        status: new cell text ("✅ bezahlt" / "💰 teilweise" / prior).
        amount_paid: the amount applied by this call (after rounding).
        accumulated: cumulative amount paid so far (this call included).
        net: invoice net.
        balance: remaining open balance (``net - accumulated``).
        fully_paid: ``True`` once ``accumulated >= net``.
        already_paid: ``True`` if the invoice was already fully paid
            and this call is a no-op / would-over-pay.
    """
    status: str
    amount_paid: float
    accumulated: float
    net: float
    balance: float
    fully_paid: bool
    already_paid: bool = False


def earnings_path(year: int, root: Optional[Path] = None) -> Path:
    base = paths.finances_dir(year) if root is None else Path(root) / "finances" / str(year)
    return base / "EARNINGS.md"


def append_earning(year: int, rec: EarningRecord, root: Optional[Path] = None) -> Tuple[Path, float]:
    """Append ``rec`` to the Einnahmen-Übersicht table, recompute
    the Summe row and the Quartalszusammenfassung block. Returns
    ``(path, ytd_total_net_after)``."""
    path = earnings_path(year, root)
    if not path.is_file():
        raise FreelanceError(f"EARNINGS.md not found: {path}")
    text = path.read_text(encoding="utf-8")
    text = _append_row(text, rec)
    text = _recompute_summe(text)
    text = _recompute_quartale(text)
    path.write_text(text, encoding="utf-8")
    return path, ytd_total(year, root)


def ytd_total(year: int, root: Optional[Path] = None) -> float:
    path = earnings_path(year, root)
    if not path.is_file():
        return 0.0
    text = path.read_text(encoding="utf-8")
    total = 0.0
    for r in _iter_rows(text, source=path):
        total += r["net"]
    return round(total, 2)


def _iter_rows(text: str, *, source: Optional[Path] = None):
    start, end = _section_bounds(text, EARN_HEADER_RE)
    body = text[start:end]
    for line in body.splitlines():
        if not DATA_ROW_RE.match(line):
            continue
        cells = md_table.parse_row(line, EARN_COLS)
        if cells is None:
            md_table.log_malformed_row(
                "earnings",
                line,
                source=source,
                expected_cols=EARN_COLS,
                reason="cell count mismatch — pipe inside a cell?",
            )
            continue
        inv_no = cells[1]
        if inv_no.startswith("INV-") and "YYYY" in inv_no:
            continue
        try:
            net = _parse_money(cells[5])
        except ValueError:
            continue
        yield {
            "inv_no": inv_no,
            "client": cells[2],
            "project": cells[3],
            "date": cells[4],
            "net": net,
            "ust": _parse_money_safe(cells[6]),
            "brutto": _parse_money_safe(cells[7]),
            "status": cells[8],
        }


def _append_row(text: str, rec: EarningRecord) -> str:
    start, end = _section_bounds(text, EARN_HEADER_RE)
    body = text[start:end]
    trailing_nl = _trailing_nl(body)
    lines = body.splitlines()

    # find table header idx
    header_idx = None
    for i, ln in enumerate(lines):
        if ln.startswith("| #") and ("Rechnungs-Nr." in ln or "Factura" in ln):
            header_idx = i
            break
    if header_idx is None:
        raise FreelanceError("earnings table header not found (looked for Rechnungs-Nr. / Factura)")

    # collect existing data rows (between header +2 and next non-data line)
    data_end = header_idx + 2
    next_num = 1
    is_placeholder_only = True
    for i in range(header_idx + 2, len(lines)):
        ln = lines[i]
        if SUMME_ROW_RE.match(ln):
            data_end = i
            break
        m = DATA_ROW_RE.match(ln)
        if not m:
            data_end = i
            break
        cells = md_table.parse_row(ln, EARN_COLS)
        if cells is None:
            # malformed row (already logged by the _iter_rows pass):
            # don't let it shift next_num, just skip and move on.
            data_end = i + 1
            continue
        inv_no = cells[1]
        if not ("YYYY" in inv_no):
            is_placeholder_only = False
            try:
                next_num = int(cells[0]) + 1
            except ValueError:
                pass
        data_end = i + 1

    new_row = (
        f"| {next_num} | {rec.inv_no} | {rec.client} | {rec.project} | {rec.issued} "
        f"| {_fmt_money(rec.net)} | {_fmt_money(rec.ust)} | {_fmt_money(rec.brutto)} | {rec.status} |"
    )
    if is_placeholder_only:
        # remove every placeholder row (still keep the Summe and any blank after)
        new_lines = lines[: header_idx + 2] + [new_row]
        # preserve the Summe row + anything after data_end
        for ln in lines[data_end:]:
            new_lines.append(ln)
        lines = new_lines
    else:
        lines.insert(data_end, new_row)

    new_body = "\n".join(lines) + ("\n" * trailing_nl)
    return text[:start] + new_body + text[end:]


def _recompute_summe(text: str) -> str:
    start, end = _section_bounds(text, EARN_HEADER_RE)
    body = text[start:end]
    trailing_nl = _trailing_nl(body)
    rows = []
    for line in body.splitlines():
        if not DATA_ROW_RE.match(line):
            continue
        cells = md_table.parse_row(line, EARN_COLS)
        if cells is None:
            # parser hardening: malformed rows are logged once during
            # _iter_rows — no double-log here.
            continue
        if "YYYY" in cells[1]:
            continue
        rows.append((
            _parse_money_safe(cells[5]),
            _parse_money_safe(cells[6]),
            _parse_money_safe(cells[7]),
        ))
    sn = sum(n for n, _, _ in rows)
    su = sum(u for _, u, _ in rows)
    sb = sum(b for _, _, b in rows)
    # Preserve the locale's summary label: DE=Summe / AR=Suma / ES=Total.
    existing_label = _extract_summary_label(body) or "Summe"
    new_summe = (
        f"| | | | | **{existing_label}** | **{_fmt_money(sn)}** | "
        f"**{_fmt_money(su)}** | **{_fmt_money(sb)}** | |"
    )
    new_body, n_subs = SUMME_ROW_RE.subn(new_summe, body, count=1)
    if n_subs == 0:
        lines = body.splitlines()
        for i, ln in enumerate(lines):
            if ln.startswith("| #") and ("Rechnungs-Nr." in ln or "Factura" in ln):
                insert_at = i + 2
                while insert_at < len(lines) and DATA_ROW_RE.match(lines[insert_at]):
                    insert_at += 1
                lines.insert(insert_at, new_summe)
                new_body = "\n".join(lines) + ("\n" * trailing_nl)
                break
    # Restore trailing newlines that .subn() / splitlines may have stripped.
    if not new_body.endswith("\n" * trailing_nl):
        new_body = new_body.rstrip("\n") + ("\n" * trailing_nl)
    return text[:start] + new_body + text[end:]


def _recompute_quartale(text: str) -> str:
    quart_start = QUART_HEADER_RE.search(text)
    if not quart_start:
        return text
    nxt = NEXT_H2_RE.search(text, quart_start.end())
    qend = nxt.start() if nxt else len(text)
    body = text[quart_start.start():qend]
    trailing_nl = _trailing_nl(body)
    lines = body.splitlines()
    header_idx = None
    for i, ln in enumerate(lines):
        if ln.startswith("| Quartal"):
            header_idx = i
            break
    if header_idx is None:
        return text
    q = {1: 0.0, 2: 0.0, 3: 0.0, 4: 0.0}
    qu = {1: 0.0, 2: 0.0, 3: 0.0, 4: 0.0}
    for r in _iter_rows(text):
        try:
            y, m, _ = r["date"].split("-")
            idx = (int(m) - 1) // 3 + 1
            q[idx] += r["net"]
            qu[idx] += r["ust"]
        except (ValueError, IndexError):
            continue
    labels = {1: "Q1 (Jan–Mrz)", 2: "Q2 (Apr–Jun)", 3: "Q3 (Jul–Sep)", 4: "Q4 (Okt–Dez)"}
    new_rows: List[str] = [
        lines[header_idx],
        lines[header_idx + 1],
    ]
    for i in (1, 2, 3, 4):
        new_rows.append(f"| {labels[i]} | {_fmt_euro(q[i])} | {_fmt_euro(qu[i])} | |")
    total_n = sum(q.values())
    total_u = sum(qu.values())
    # DE="Gesamt", AR/ES="Total" — preserve existing label if detectable.
    total_label = _extract_total_label(body) or "Gesamt"
    new_rows.append(f"| **{total_label}** | **{_fmt_euro(total_n)}** | **{_fmt_euro(total_u)}** | |")
    # keep everything after the 6 original table lines
    orig_end = header_idx + 2 + 5
    tail = lines[orig_end:]
    new_body = "\n".join(lines[:header_idx] + new_rows + tail) + ("\n" * trailing_nl)
    return text[:quart_start.start()] + new_body + text[qend:]


def _extract_summary_label(body: str) -> str:
    """Detect the existing ``**<label>**`` used in the Summe/Suma/Total row."""
    m = re.search(r"\*\*(Summe|Suma|Total)\*\*", body)
    return m.group(1) if m else ""


def _extract_total_label(body: str) -> str:
    """Detect the existing ``**<label>**`` used in the Quartale/Trimestral
    totals row — DE: 'Gesamt', AR/ES: 'Total'."""
    m = re.search(r"\|\s*\*\*(Gesamt|Total)\*\*", body)
    return m.group(1) if m else ""


def _trailing_nl(s: str) -> int:
    n = 0
    i = len(s) - 1
    while i >= 0 and s[i] == "\n":
        n += 1
        i -= 1
    return n


def _section_bounds(text: str, header_re: re.Pattern) -> Tuple[int, int]:
    m = header_re.search(text)
    if not m:
        raise FreelanceError(f"section header not found: {header_re.pattern!r}")
    start = m.start()
    nxt = NEXT_H2_RE.search(text, m.end())
    end = nxt.start() if nxt else len(text)
    return start, end


def _fmt_money(v: float) -> str:
    return f"{v:,.2f}".replace(",", "X").replace(".", ",").replace("X", ".")


def _fmt_euro(v: float) -> str:
    return _fmt_money(v) + " €"


def _parse_money(s: str) -> float:
    s = s.strip().replace("€", "").replace(" ", "").replace("*", "")
    if s == "" or s == "—":
        raise ValueError("empty money")
    # accept both "1.234,56" and "1234.56"
    if "," in s and s.count(",") == 1 and s[-3:-2] == ",":
        s = s.replace(".", "").replace(",", ".")
    return float(s)


def _parse_money_safe(s: str) -> float:
    try:
        return _parse_money(s)
    except ValueError:
        return 0.0


# ── mark-paid support (v0.3 Phase 2) ─────────────────────────────────


STATUS_OPEN = "⏳ offen"
STATUS_FULL = "✅ bezahlt"
STATUS_PARTIAL = "💰 teilweise"


def iter_rows(year: int, root: Optional[Path] = None):
    """Public alias over the hardened ``_iter_rows`` for consumers
    outside this module (SANCHO handlers, mark-paid, dashboard)."""
    path = earnings_path(year, root)
    if not path.is_file():
        return iter(())
    text = path.read_text(encoding="utf-8")
    return _iter_rows(text, source=path)


def payments_total(year: int, inv_no: str, root: Optional[Path] = None) -> float:
    """Sum of Zahlungseingänge / Cobros Recibidos rows booked
    against ``inv_no``. Zero when the section is empty."""
    path = earnings_path(year, root)
    if not path.is_file():
        return 0.0
    text = path.read_text(encoding="utf-8")
    m = ZAHL_HEADER_RE.search(text)
    if not m:
        return 0.0
    nxt = NEXT_H2_RE.search(text, m.end())
    body = text[m.end(): nxt.start() if nxt else len(text)]
    total = 0.0
    for line in body.splitlines():
        if not line.startswith("|"):
            continue
        cells = md_table.parse_row(line, 5)
        if cells is None:
            continue
        if cells[0] != inv_no:
            continue
        try:
            total += _parse_money(cells[1])
        except ValueError:
            continue
    return round(total, 2)


def _replace_status_in_row(line: str, new_status: str) -> str:
    """Replace the Status cell (cell index 8) in a 9-col data row.

    Preserves surrounding whitespace. When the row is malformed,
    returns the line unchanged.
    """
    cells = md_table.parse_row(line, EARN_COLS)
    if cells is None:
        return line
    # Re-serialise with a leading space (matches the format produced
    # by ``_append_row`` and the template) so the write stays
    # byte-faithful to the surrounding table.
    cells[8] = new_status
    return "| " + " | ".join(cells) + " |"


def _append_zahlung_row(
    text: str,
    inv_no: str,
    amount: float,
    paid_date: str,
    bank_entry_date: str,
    status: str,
) -> str:
    """Insert a new row into the Zahlungseingänge / Cobros Recibidos
    table by positional slice.

    Byte-faithful to surrounding lines: we do NOT round-trip the
    section through ``splitlines + join`` (that pattern silently
    drifted trailing newlines). Instead, we locate the last table
    row via a single string scan and splice the new row in place.
    """
    m = ZAHL_HEADER_RE.search(text)
    if not m:
        raise FreelanceError("Zahlungseingänge / Cobros Recibidos section not found")
    nxt = NEXT_H2_RE.search(text, m.end())
    section_end = nxt.start() if nxt else len(text)
    section_start = m.start()

    # Locate the table header within the section.
    hdr_pos = -1
    for marker in ("| Rechnungs-Nr.", "|Rechnungs-Nr.", "| Factura", "|Factura"):
        p = text.find(marker, section_start, section_end)
        if p != -1:
            hdr_pos = p
            break
    if hdr_pos == -1:
        raise FreelanceError("Zahlungseingänge table header not found")

    # Skip the separator line, then walk forward as long as we see
    # ``|``-prefixed data rows at the start of each line.
    # First, advance past the header line.
    after_header = text.find("\n", hdr_pos) + 1
    # Advance past the separator.
    after_sep = text.find("\n", after_header) + 1
    cursor = after_sep
    # Walk data rows.
    while cursor < section_end:
        # Find end of line at cursor.
        nl = text.find("\n", cursor)
        if nl == -1 or nl > section_end:
            break
        line = text[cursor:nl]
        if not line.startswith("|"):
            break
        cursor = nl + 1

    new_row = (
        f"| {inv_no} | {_fmt_money(amount)} | {paid_date} | "
        f"{bank_entry_date} | {status} |\n"
    )
    return text[:cursor] + new_row + text[cursor:]


def _rewrite_status_cell_in_place(text: str, inv_no: str, new_status: str) -> str:
    """Replace the status cell in the Einnahmen-Übersicht row for
    ``inv_no`` by positional slice.

    Preserves every surrounding line byte-for-byte (no section
    round-trip through ``splitlines``)."""
    start, end = _section_bounds(text, EARN_HEADER_RE)
    body = text[start:end]
    cursor = 0
    while cursor < len(body):
        nl = body.find("\n", cursor)
        line_end = nl if nl != -1 else len(body)
        line = body[cursor:line_end]
        if DATA_ROW_RE.match(line):
            cells = md_table.parse_row(line, EARN_COLS)
            if cells is not None and cells[1] == inv_no:
                cells[8] = new_status
                replacement = "| " + " | ".join(cells) + " |"
                new_body = body[:cursor] + replacement + body[line_end:]
                return text[:start] + new_body + text[end:]
        if nl == -1:
            break
        cursor = nl + 1
    raise FreelanceError(
        f"could not rewrite status cell for [[{inv_no}]] — row not found"
    )


def mark_row_paid(
    year: int,
    inv_no: str,
    paid_date: str,
    *,
    amount: Optional[float] = None,
    bank_entry_date: Optional[str] = None,
    root: Optional[Path] = None,
) -> PaidStatus:
    """Flip the EARNINGS status cell for ``inv_no`` and append a
    Zahlungseingänge / Cobros Recibidos row.

    ``amount=None`` means "pay the full outstanding balance" — no
    partial accounting. ``amount`` less than the outstanding balance
    records a partial payment and sets status to ``💰 teilweise``.
    When the cumulative paid amount reaches or exceeds ``net``, the
    status auto-flips to ``✅ bezahlt``.
    """
    path = earnings_path(year, root)
    if not path.is_file():
        raise FreelanceError(f"EARNINGS.md not found: {path}")
    text = path.read_text(encoding="utf-8")

    # Find the target row (via the hardened iterator so a malformed
    # row never shifts our view).
    target = None
    for rec in _iter_rows(text, source=path):
        if rec["inv_no"] == inv_no:
            target = rec
            break
    if target is None:
        raise FreelanceError(
            f"Invoice [[{inv_no}]] has no EARNINGS.md row. "
            "Run `freelance-office reconcile` to backfill, or add the row manually."
        )

    net = float(target["net"])
    already = payments_total(year, inv_no, root)
    prior_status = target["status"]
    if _status_is_full(prior_status) or already >= net - 0.005:
        # Already fully paid — idempotent no-op for the zero-amount
        # call, explicit over-payment error when an amount is passed.
        if amount is not None:
            raise FreelanceError(
                f"Invoice [[{inv_no}]] is already paid in full ({_fmt_money(already)}); "
                f"cannot apply additional €{amount:.2f}."
            )
        return PaidStatus(
            status=STATUS_FULL,
            amount_paid=0.0,
            accumulated=already,
            net=net,
            balance=0.0,
            fully_paid=True,
            already_paid=True,
        )

    if amount is None:
        applied = round(net - already, 2)
    else:
        applied = round(float(amount), 2)
        if applied <= 0:
            raise FreelanceError(f"--amount must be positive; got {amount}")
        if already + applied > net + 0.005:
            raise FreelanceError(
                f"Applying €{applied:.2f} would over-pay [[{inv_no}]] "
                f"(net €{net:.2f}, already paid €{already:.2f})."
            )

    new_accum = round(already + applied, 2)
    fully_paid = new_accum >= net - 0.005
    new_status = STATUS_FULL if fully_paid else STATUS_PARTIAL

    # Rewrite only the row's status cell in place (no section
    # round-trip — preserves every surrounding line byte-for-byte).
    text = _rewrite_status_cell_in_place(text, inv_no, new_status)

    # Append the Zahlungseingänge row.
    text = _append_zahlung_row(
        text,
        inv_no=inv_no,
        amount=applied,
        paid_date=paid_date,
        bank_entry_date=bank_entry_date or paid_date,
        status=new_status,
    )

    path.write_text(text, encoding="utf-8")
    return PaidStatus(
        status=new_status,
        amount_paid=applied,
        accumulated=new_accum,
        net=net,
        balance=max(0.0, round(net - new_accum, 2)),
        fully_paid=fully_paid,
    )


def _status_is_full(status: str) -> bool:
    s = (status or "").lower()
    return "bezahlt" in s and "teilweise" not in s
