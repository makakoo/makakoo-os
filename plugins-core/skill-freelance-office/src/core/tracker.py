"""Parse + update ``_project-tracker.md`` line-by-line.

The file is markdown with one embedded YAML block (Projekt-Steckbrief),
a Stundenerfassung table keyed by calendar week, and a Rechnungen
table keyed by invoice number. We mutate *in place* — surrounding
prose and formatting stay byte-identical.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Tuple

from .errors import FreelanceError

DAYS_ORDER = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"]

_STECKBRIEF_RE = re.compile(r"(## 📋 Projekt-Steckbrief\s*\n+```yaml\n)(.*?)(\n```)", re.DOTALL)
_STUNDEN_HEADER_RE = re.compile(r"^## ⏱️ Stundenerfassung\s*$", re.MULTILINE)
_RECH_HEADER_RE = re.compile(r"^## 💰 Rechnungen\s*$", re.MULTILINE)
_NEXT_HEADER_RE = re.compile(r"^## ", re.MULTILINE)
_KW_ROW_RE = re.compile(r"^\|\s*(\d{1,2})\s*\|")
_INV_ROW_RE = re.compile(r"^\|\s*(\d+)\s*\|\s*(INV-\d{4}-\d+)\s*\|")


@dataclass
class Tracker:
    path: Path
    text: str
    agreed_days: int = 0
    spent_days: int = 0
    remaining_days: int = 0

    @classmethod
    def load(cls, path: Path) -> "Tracker":
        if not path.is_file():
            raise FreelanceError(f"tracker not found: {path}")
        text = path.read_text(encoding="utf-8")
        t = cls(path=path, text=text)
        t._parse_steckbrief()
        return t

    def _parse_steckbrief(self) -> None:
        m = _STECKBRIEF_RE.search(self.text)
        if not m:
            return
        for line in m.group(2).splitlines():
            line = line.strip()
            if line.startswith("agreed_days:"):
                self.agreed_days = _to_int(line.split(":", 1)[1])
            elif line.startswith("spent_days:"):
                self.spent_days = _to_int(line.split(":", 1)[1])
            elif line.startswith("remaining_days:"):
                self.remaining_days = _to_int(line.split(":", 1)[1])

    def update_hours(self, kw: int, hours_per_day: Dict[str, float], note: str = "") -> None:
        """Upsert a calendar-week row. ``hours_per_day`` keys in DAYS_ORDER."""
        section_start, section_end = _find_section(self.text, _STUNDEN_HEADER_RE)
        body = self.text[section_start:section_end]
        trailing_nl = _count_trailing_nl(body)
        lines = body.splitlines()
        header_idx = _find_table_header_idx(lines)
        if header_idx is None:
            raise FreelanceError("Stundenerfassung table header not found")
        # Build new row
        total = sum(hours_per_day.get(d, 0) for d in DAYS_ORDER)
        cells = [f" {kw:02d} "]
        for d in DAYS_ORDER:
            v = hours_per_day.get(d, None)
            cells.append(_fmt_hours(v))
        cells.append(f" {_fmt_total(total)} ")
        cells.append(f" {note} ")
        new_row = "|" + "|".join(cells) + "|"
        # Search for existing KW row
        updated = False
        for i in range(header_idx + 2, len(lines)):
            mm = _KW_ROW_RE.match(lines[i])
            if mm and int(mm.group(1)) == kw:
                # Merge: additive for hours_per_day supplied
                existing = _parse_kw_row(lines[i])
                for d in DAYS_ORDER:
                    if d in hours_per_day:
                        existing[d] = hours_per_day[d]
                existing["_note"] = note or existing.get("_note", "")
                lines[i] = _render_kw_row(kw, existing)
                updated = True
                break
        if not updated:
            # Append before the next blank line / section end
            insert_at = len(lines)
            for i in range(len(lines) - 1, header_idx, -1):
                if lines[i].strip() == "":
                    insert_at = i
                    continue
                if lines[i].startswith("|"):
                    insert_at = i + 1
                    break
            lines.insert(insert_at, new_row)
        new_body = "\n".join(lines) + ("\n" * trailing_nl)
        self.text = self.text[:section_start] + new_body + self.text[section_end:]
        self._recompute_days()

    def _recompute_days(self) -> None:
        """Rebuild spent_days + remaining_days from Stundenerfassung."""
        total_hours = 0.0
        section_start, section_end = _find_section(self.text, _STUNDEN_HEADER_RE)
        body = self.text[section_start:section_end]
        # Read-only — no rewrite, so no trailing-nl preservation needed.
        for line in body.splitlines():
            if _KW_ROW_RE.match(line):
                row = _parse_kw_row(line)
                for d in DAYS_ORDER:
                    v = row.get(d)
                    if isinstance(v, (int, float)):
                        total_hours += float(v)
        spent = round(total_hours / 8, 2)
        remaining = max(0.0, float(self.agreed_days) - spent)
        self.spent_days = int(spent) if spent.is_integer() else spent  # type: ignore[assignment]
        self.remaining_days = int(remaining) if float(remaining).is_integer() else remaining  # type: ignore[assignment]
        m = _STECKBRIEF_RE.search(self.text)
        if not m:
            return
        block = m.group(2)
        block = _yaml_replace(block, "spent_days", self.spent_days)
        block = _yaml_replace(block, "remaining_days", self.remaining_days)
        self.text = self.text[: m.start(2)] + block + self.text[m.end(2) :]

    def mark_invoice_paid(self, inv_no: str, *, partial: bool = False) -> bool:
        """Flip the "Bezahlt" cell of the matching Rechnungen row
        to ``[✅]`` (full) or ``[💰]`` (partial). Returns ``True`` if
        a row was found and updated, ``False`` otherwise. Caller is
        responsible for calling :meth:`write` afterwards plus the
        two-phase read-back verify (pi corruption-risk #2)."""
        section_start, section_end = _find_section(self.text, _RECH_HEADER_RE)
        body = self.text[section_start:section_end]
        trailing_nl = _count_trailing_nl(body)
        lines = body.splitlines()
        marker = "[💰]" if partial else "[✅]"
        updated = False
        for i, ln in enumerate(lines):
            mm = _INV_ROW_RE.match(ln)
            if not mm or mm.group(2) != inv_no:
                continue
            # Walk the cells and replace the final ``[ ]`` (or any
            # prior marker) with the new marker. Splitting the row
            # preserves the other cells byte-for-byte.
            parts = ln.split("|")
            # Last non-empty segment is the Bezahlt cell (there's a
            # trailing empty segment after the final ``|``).
            idx = len(parts) - 2
            while idx >= 0 and parts[idx].strip() == "":
                idx -= 1
            if idx < 0:
                break
            # Only flip the final cell if it currently holds a
            # checkbox-shaped payload.
            current = parts[idx].strip()
            if current in ("[ ]", "[✅]", "[💰]"):
                parts[idx] = f" {marker} "
                lines[i] = "|".join(parts)
                updated = True
            break
        if updated:
            new_body = "\n".join(lines) + ("\n" * trailing_nl)
            self.text = self.text[:section_start] + new_body + self.text[section_end:]
        return updated

    def verify_invoice_paid_marker(self, inv_no: str, *, partial: bool = False) -> bool:
        """Re-read the text buffer and confirm the expected marker
        landed in the ``inv_no`` row — pi corruption-risk #2's
        read-after-write check."""
        marker = "[💰]" if partial else "[✅]"
        section_start, section_end = _find_section(self.text, _RECH_HEADER_RE)
        body = self.text[section_start:section_end]
        for ln in body.splitlines():
            mm = _INV_ROW_RE.match(ln)
            if mm and mm.group(2) == inv_no and marker in ln:
                return True
        return False

    def append_invoice(self, inv_no: str, amount_net: float, issued: str, due: str) -> None:
        section_start, section_end = _find_section(self.text, _RECH_HEADER_RE)
        body = self.text[section_start:section_end]
        trailing_nl = _count_trailing_nl(body)
        lines = body.splitlines()
        header_idx = _find_table_header_idx(lines)
        if header_idx is None:
            raise FreelanceError("Rechnungen table header not found")
        next_num = 1
        last_inv_line = header_idx + 1
        for i in range(header_idx + 2, len(lines)):
            mm = _INV_ROW_RE.match(lines[i])
            if mm:
                next_num = int(mm.group(1)) + 1
                last_inv_line = i
        new_row = f"| {next_num} | {inv_no} | €{amount_net:,.2f} | {issued} | {due} | [ ] |"
        lines.insert(last_inv_line + 1, new_row)
        new_body = "\n".join(lines) + ("\n" * trailing_nl)
        self.text = self.text[:section_start] + new_body + self.text[section_end:]

    def write(self) -> None:
        self.path.write_text(self.text, encoding="utf-8")


def _to_int(s: str) -> int:
    s = s.strip()
    m = re.match(r"(-?\d+)", s)
    return int(m.group(1)) if m else 0


def _fmt_hours(v) -> str:
    if v is None or v == "":
        return "  "
    if isinstance(v, float) and v.is_integer():
        v = int(v)
    return f" {v} "


def _fmt_total(hours: float) -> str:
    if hours == 0:
        return "0h"
    if float(hours).is_integer():
        return f"{int(hours)}h"
    return f"{hours}h"


def _find_section(text: str, header_re: re.Pattern) -> Tuple[int, int]:
    m = header_re.search(text)
    if not m:
        raise FreelanceError(f"section header not found: {header_re.pattern}")
    start = m.start()
    nxt = _NEXT_HEADER_RE.search(text, m.end())
    end = nxt.start() if nxt else len(text)
    return start, end


def _find_table_header_idx(lines: List[str]) -> Optional[int]:
    for i, ln in enumerate(lines):
        if ln.startswith("|") and ("KW" in ln or "INV-Nr." in ln or "#" in ln):
            return i
    return None


def _parse_kw_row(line: str) -> Dict[str, object]:
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    out: Dict[str, object] = {}
    if len(cells) < 10:
        return out
    try:
        out["_kw"] = int(cells[0])
    except ValueError:
        return {}
    for i, d in enumerate(DAYS_ORDER):
        c = cells[1 + i]
        if c == "":
            out[d] = None
        else:
            try:
                out[d] = float(c) if "." in c else int(c)
            except ValueError:
                out[d] = None
    out["_summe"] = cells[8]
    out["_note"] = cells[9]
    return out


def _render_kw_row(kw: int, row: Dict[str, object]) -> str:
    cells = [f" {kw:02d} "]
    total = 0.0
    for d in DAYS_ORDER:
        v = row.get(d)
        if isinstance(v, (int, float)):
            total += float(v)
        cells.append(_fmt_hours(v))
    cells.append(f" {_fmt_total(total)} ")
    cells.append(f" {row.get('_note', '')} ")
    return "|" + "|".join(cells) + "|"


def _count_trailing_nl(s: str) -> int:
    """Count how many trailing ``\\n`` characters a string has, so we can
    restore them after a ``splitlines`` / ``"\\n".join`` round-trip."""
    n = 0
    i = len(s) - 1
    while i >= 0 and s[i] == "\n":
        n += 1
        i -= 1
    return n


_YAML_KEY_RE = {}


def _yaml_replace(block: str, key: str, value) -> str:
    pat = _YAML_KEY_RE.get(key)
    if pat is None:
        pat = re.compile(rf"^(\s*){key}:\s*[^\n#]*(\s*(?:#.*)?)$", re.MULTILINE)
        _YAML_KEY_RE[key] = pat
    return pat.sub(lambda m: f"{m.group(1)}{key}: {value}{m.group(2)}", block)
