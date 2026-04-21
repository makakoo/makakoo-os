"""Section-scoped mutation of ``finances/<YYYY>/EXPENSES.md``.

Seven deductible categories — each is an H2 section with one table.
Append lands in the right category. Jahreszusammenfassung is
recomputed in-place. Surrounding prose stays byte-identical.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Tuple

from . import paths
from .errors import FreelanceError

CATEGORY_HEADERS = {
    "equipment":    "## 🖥 Equipment (AfA — Abschreibung über 3 Jahre)",
    "software":     "## 💻 Software & Lizenzen",
    "fortbildung":  "## 📚 Fortbildung",
    "homeoffice":   "## 🏠 Homeoffice-Pauschale",
    "telefon":      "## 📱 Telefon & Internet (anteilig, geschäftlich)",
    "fahrt":        "## 🚗 Fahrtkosten",
    "arbeitsmittel": "## ☕ Arbeitsmittel (einmalig < 800 €)",
}

CATEGORY_LABEL = {
    "equipment":    "Equipment",
    "software":     "Software",
    "fortbildung":  "Fortbildung",
    "homeoffice":   "Homeoffice-Pauschale",
    "telefon":      "Telefon & Internet",
    "fahrt":        "Fahrtkosten",
    "arbeitsmittel": "Arbeitsmittel",
}

# 0-based index of the "Netto/Betrag" column (after the leading | is stripped).
CATEGORY_NET_COL = {
    "equipment":    3,   # | Datum | Beschreibung | Kategorie | Netto (€) | USt (€) | Beleg |
    "software":     3,
    "fortbildung":  3,
    "homeoffice":   2,   # | Monat | Arbeitstage | Betrag (€) |
    "telefon":      3,   # | Datum | Beschreibung | Anteil | Netto (€) |
    "fahrt":        3,   # | Datum | Strecke | km | Betrag (€) |
    "arbeitsmittel": 2,  # | Datum | Beschreibung | Netto (€) |
}

# DE: "Jahreszusammenfassung"; AR + ES: "Resumen Anual".
JAHRES_RE = re.compile(
    r"^## 💰 (?:Jahreszusammenfassung|Resumen Anual) \[.*?\]\s*$",
    re.MULTILINE,
)
NEXT_H2_RE = re.compile(r"^## ", re.MULTILINE)


@dataclass
class ExpenseRecord:
    date: str  # YYYY-MM-DD
    description: str
    category: str
    net: float
    ust: float = 0.0
    beleg: str = "_________"


def expenses_path(year: int, root: Optional[Path] = None) -> Path:
    base = paths.finances_dir(year) if root is None else Path(root) / "finances" / str(year)
    return base / "EXPENSES.md"


def append_expense(year: int, rec: ExpenseRecord, root: Optional[Path] = None) -> Tuple[Path, Dict[str, float]]:
    """Append ``rec`` to the category table, recompute Jahreszusammenfassung.
    Returns ``(path, {category: ytd_sum, "__total__": overall_ytd})``."""
    if rec.category not in CATEGORY_HEADERS:
        raise FreelanceError(
            f"unknown category {rec.category!r}; accepted: {sorted(CATEGORY_HEADERS)}"
        )
    path = expenses_path(year, root)
    if not path.is_file():
        raise FreelanceError(f"EXPENSES.md not found: {path}")
    text = path.read_text(encoding="utf-8")
    text = _append_row(text, rec)
    text = _recompute_jahres(text)
    path.write_text(text, encoding="utf-8")
    return path, ytd_by_category(year, root)


def ytd_by_category(year: int, root: Optional[Path] = None) -> Dict[str, float]:
    path = expenses_path(year, root)
    if not path.is_file():
        return {"__total__": 0.0}
    text = path.read_text(encoding="utf-8")
    out: Dict[str, float] = {k: 0.0 for k in CATEGORY_HEADERS}
    for cat, total in _scan_categories(text):
        out[cat] = round(total, 2)
    out["__total__"] = round(sum(out.values()), 2)
    return out


def _scan_categories(text: str):
    for cat, header in CATEGORY_HEADERS.items():
        start, end = _section_by_literal(text, header)
        if start < 0:
            continue
        body = text[start:end]
        net_col = CATEGORY_NET_COL[cat]
        total = 0.0
        for line in body.splitlines():
            if not line.startswith("|"):
                continue
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if not cells or cells[0].lower().startswith(("monat", "datum", "---")):
                continue
            if "YYYY" in cells[0] or "**" in cells[0] or cells[0] == "":
                continue
            if "summe" in cells[0].lower() or "gesamt" in cells[0].lower():
                continue
            if len(cells) <= net_col:
                continue
            money_cell = cells[net_col]
            if money_cell == "" or money_cell.startswith("_"):
                continue
            try:
                total += _parse_money(money_cell)
            except ValueError:
                continue
        yield cat, total


def _append_row(text: str, rec: ExpenseRecord) -> str:
    header = CATEGORY_HEADERS[rec.category]
    start, end = _section_by_literal(text, header)
    if start < 0:
        raise FreelanceError(f"category section not found: {header!r}")
    body = text[start:end]
    trailing_nl = _trailing_nl(body)
    lines = body.splitlines()
    # locate table header row
    hdr_idx = None
    for i, ln in enumerate(lines):
        if ln.startswith("| Datum") or ln.startswith("| Monat"):
            hdr_idx = i
            break
    if hdr_idx is None:
        raise FreelanceError(f"table header not found in section {header!r}")
    # new row
    brutto = rec.net + rec.ust
    if rec.category == "homeoffice":
        new_row = f"| {rec.date} | {rec.description} | {_fmt_money(rec.net)} |"
    elif rec.category == "telefon":
        new_row = f"| {rec.date} | {rec.description} | 100% | {_fmt_money(rec.net)} |"
    elif rec.category == "fahrt":
        new_row = f"| {rec.date} | {rec.description} | 0 | {_fmt_money(rec.net)} |"
    elif rec.category == "arbeitsmittel":
        new_row = f"| {rec.date} | {rec.description} | {_fmt_money(rec.net)} |"
    else:
        cat_label = CATEGORY_LABEL[rec.category]
        new_row = (
            f"| {rec.date} | {rec.description} | {cat_label} "
            f"| {_fmt_money(rec.net)} | {_fmt_money(rec.ust)} | {rec.beleg} |"
        )
    # find insert point: drop placeholder rows (those with YYYY-MM-DD), keep sorted
    insert_at = hdr_idx + 2
    drop_indices: List[int] = []
    for i in range(hdr_idx + 2, len(lines)):
        ln = lines[i]
        if not ln.startswith("|"):
            break
        if _is_placeholder(ln):
            drop_indices.append(i)
            continue
        cells = [c.strip() for c in ln.strip().strip("|").split("|")]
        if cells and cells[0] and cells[0] <= rec.date:
            insert_at = i + 1
    # drop in reverse so indices stay valid
    for idx in reversed(drop_indices):
        del lines[idx]
    insert_at = min(insert_at, len(lines))
    lines.insert(insert_at, new_row)
    new_body = "\n".join(lines) + ("\n" * trailing_nl)
    return text[:start] + new_body + text[end:]


def _is_placeholder(line: str) -> bool:
    return "YYYY-MM-DD" in line


def _recompute_jahres(text: str) -> str:
    m = JAHRES_RE.search(text)
    if not m:
        return text
    nxt = NEXT_H2_RE.search(text, m.end())
    end = nxt.start() if nxt else len(text)
    body = text[m.start():end]
    trailing_nl = _trailing_nl(body)
    lines = body.splitlines()
    hdr_idx = None
    for i, ln in enumerate(lines):
        if ln.startswith("| Kategorie") or ln.startswith("|Kategorie"):
            hdr_idx = i
            break
    if hdr_idx is None:
        return text
    by_cat = ytd_by_category_from_text(text)
    new_rows: List[str] = [lines[hdr_idx], lines[hdr_idx + 1]]
    for cat, label in CATEGORY_LABEL.items():
        v = by_cat.get(cat, 0.0)
        new_rows.append(f"| {label} | {_fmt_money(v)} |")
    total = sum(by_cat.get(c, 0.0) for c in CATEGORY_LABEL)
    new_rows.append(f"| **Gesamt** | **{_fmt_money(total)}** |")
    # keep original tail (below the existing summary table)
    orig_table_end = hdr_idx + 2 + len(CATEGORY_LABEL) + 1
    if orig_table_end > len(lines):
        orig_table_end = len(lines)
    tail = lines[orig_table_end:]
    new_body = "\n".join(lines[:hdr_idx] + new_rows + tail) + ("\n" * trailing_nl)
    return text[: m.start()] + new_body + text[end:]


def ytd_by_category_from_text(text: str) -> Dict[str, float]:
    out: Dict[str, float] = {c: 0.0 for c in CATEGORY_HEADERS}
    for cat, total in _scan_categories(text):
        out[cat] = round(total, 2)
    return out


def _trailing_nl(s: str) -> int:
    n = 0
    i = len(s) - 1
    while i >= 0 and s[i] == "\n":
        n += 1
        i -= 1
    return n


def _section_by_literal(text: str, header_literal: str) -> Tuple[int, int]:
    idx = text.find(header_literal)
    if idx == -1:
        return -1, -1
    nxt = NEXT_H2_RE.search(text, idx + len(header_literal))
    end = nxt.start() if nxt else len(text)
    return idx, end


def _fmt_money(v: float) -> str:
    return f"{v:,.2f}".replace(",", "X").replace(".", ",").replace("X", ".")


def _parse_money(s: str) -> float:
    s = s.strip().replace("€", "").replace(" ", "").replace("*", "").replace("_", "")
    if s in ("", "—", "0", "0,00"):
        return float(s.replace(",", ".")) if s else 0.0
    if "," in s and s.count(",") == 1 and s[-3:-2] == ",":
        s = s.replace(".", "").replace(",", ".")
    return float(s)
