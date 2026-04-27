"""Ship Gate #12 — format fidelity on real EARNINGS.md + EXPENSES.md.

Stage a byte-for-byte copy of the hand-maintained
``finances/2026/{EARNINGS,EXPENSES}.md`` as a fixture (one-time
snapshot at sprint time). After appending one record, every section
header + every non-target-table line + every piece of prose must
still match the original byte-for-byte.

The target table + the recomputed summary block are the only lines
allowed to differ.
"""
from __future__ import annotations

import re
import shutil
from pathlib import Path

import pytest

from src.core import earnings, expenses

FIXTURES = Path(__file__).parent / "fixtures" / "golden"


def _stage(tmp_path: Path, which: str) -> Path:
    src = FIXTURES / f"{which}_before.md"
    dest_dir = tmp_path / "finances" / "2026"
    dest_dir.mkdir(parents=True)
    dest = dest_dir / f"{which}.md"
    shutil.copyfile(src, dest)
    return tmp_path


def _section_headers(text: str):
    return [ln for ln in text.splitlines() if re.match(r"^##\s+", ln)]


def test_earnings_append_preserves_every_section_header(tmp_path):
    root = _stage(tmp_path, "EARNINGS")
    before_text = (root / "finances/2026/EARNINGS.md").read_text()
    before_headers = _section_headers(before_text)
    rec = earnings.EarningRecord(
        inv_no="INV-2026-001",
        client="northbound",
        project="platform-migration",
        issued="2026-04-21",
        net=9600.00,
        ust=0.00,
        status="⏳ offen",
    )
    earnings.append_earning(2026, rec, root)
    after_text = (root / "finances/2026/EARNINGS.md").read_text()
    after_headers = _section_headers(after_text)
    assert after_headers == before_headers, (
        f"section headers drifted — before={before_headers}, after={after_headers}"
    )


def test_earnings_steuerliche_bemerkungen_untouched(tmp_path):
    root = _stage(tmp_path, "EARNINGS")
    before_text = (root / "finances/2026/EARNINGS.md").read_text()
    # Extract the "Steuerliche Bemerkungen" section (last H2, prose only)
    b_start = before_text.find("## Steuerliche Bemerkungen")
    b_block = before_text[b_start:]

    rec = earnings.EarningRecord(
        inv_no="INV-2026-001", client="x", project="y",
        issued="2026-04-21", net=1000, ust=0, status="⏳ offen",
    )
    earnings.append_earning(2026, rec, root)

    after_text = (root / "finances/2026/EARNINGS.md").read_text()
    a_start = after_text.find("## Steuerliche Bemerkungen")
    a_block = after_text[a_start:]
    assert a_block == b_block, "prose in Steuerliche Bemerkungen was mutated"


def test_expenses_append_preserves_every_section_header(tmp_path):
    root = _stage(tmp_path, "EXPENSES")
    before_text = (root / "finances/2026/EXPENSES.md").read_text()
    before_headers = _section_headers(before_text)
    rec = expenses.ExpenseRecord(
        date="2026-04-21", description="lexoffice annual",
        category="software", net=149.00, ust=28.31,
    )
    expenses.append_expense(2026, rec, root)
    after_text = (root / "finances/2026/EXPENSES.md").read_text()
    after_headers = _section_headers(after_text)
    assert after_headers == before_headers


def test_earnings_mark_paid_preserves_every_non_target_line(tmp_path):
    """Phase 2 format-fidelity: marking a row paid must rewrite only
    the target row's status cell + append one Zahlungseingänge row.
    Every other line of the file stays byte-identical (pi
    corruption-risk #2 downstream: a mutation that rearranges
    untouched lines masks whether the Summe recompute is safe)."""
    root = _stage(tmp_path, "EARNINGS")
    rec = earnings.EarningRecord(
        inv_no="INV-2026-042", client="northbound",
        project="proj", issued="2026-04-21", net=1000, ust=0,
        status="⏳ offen",
    )
    earnings.append_earning(2026, rec, root)
    baseline = (root / "finances/2026/EARNINGS.md").read_text(encoding="utf-8").splitlines()

    # Now mark the row paid and diff line-by-line.
    earnings.mark_row_paid(
        2026, "INV-2026-042", "2026-05-01", root=root,
    )
    after = (root / "finances/2026/EARNINGS.md").read_text(encoding="utf-8").splitlines()

    assert len(after) == len(baseline) + 1, (
        f"expected exactly one new line (Zahlungseingänge append); "
        f"got {len(after) - len(baseline)}"
    )

    # The only allowed mutations are:
    #   (a) one in-place status-cell flip on the INV-2026-042 row, and
    #   (b) one newly-inserted Zahlungseingänge row.
    # Use difflib to get the structured diff and assert exactly
    # those two opcodes.
    import difflib
    sm = difflib.SequenceMatcher(a=baseline, b=after)
    opcodes = [op for op in sm.get_opcodes() if op[0] != "equal"]
    replaces = [op for op in opcodes if op[0] == "replace"]
    inserts = [op for op in opcodes if op[0] == "insert"]
    deletes = [op for op in opcodes if op[0] == "delete"]
    assert not deletes, f"unexpected deletions: {deletes}"
    assert len(replaces) == 1, f"expected exactly one replaced row, got {replaces}"
    _, a1, a2, b1, b2 = replaces[0]
    # One line replaced → one new line
    assert a2 - a1 == 1 and b2 - b1 == 1
    assert "⏳ offen" in baseline[a1]
    assert "✅ bezahlt" in after[b1]
    assert "INV-2026-042" in after[b1]
    assert len(inserts) == 1, f"expected exactly one Zahlungseingänge insert, got {inserts}"
    _, _, _, b1, b2 = inserts[0]
    inserted_lines = after[b1:b2]
    assert len(inserted_lines) == 1
    assert "INV-2026-042" in inserted_lines[0]
    assert "2026-05-01" in inserted_lines[0]


def test_earnings_partial_mark_paid_preserves_every_non_target_line(tmp_path):
    """Same byte-fidelity invariant for partial payments — status
    cell flips to ``💰 teilweise`` and one Zahlungseingänge row is
    appended; every other line stays identical."""
    root = _stage(tmp_path, "EARNINGS")
    rec = earnings.EarningRecord(
        inv_no="INV-2026-077", client="acme", project="proj",
        issued="2026-04-21", net=2000, ust=0, status="⏳ offen",
    )
    earnings.append_earning(2026, rec, root)
    baseline = (root / "finances/2026/EARNINGS.md").read_text(encoding="utf-8").splitlines()

    earnings.mark_row_paid(
        2026, "INV-2026-077", "2026-05-01", amount=500.0, root=root,
    )
    after = (root / "finances/2026/EARNINGS.md").read_text(encoding="utf-8").splitlines()

    assert len(after) == len(baseline) + 1

    import difflib
    sm = difflib.SequenceMatcher(a=baseline, b=after)
    opcodes = [op for op in sm.get_opcodes() if op[0] != "equal"]
    replaces = [op for op in opcodes if op[0] == "replace"]
    inserts = [op for op in opcodes if op[0] == "insert"]
    deletes = [op for op in opcodes if op[0] == "delete"]
    assert not deletes
    assert len(replaces) == 1
    _, a1, _, b1, _ = replaces[0]
    assert "⏳ offen" in baseline[a1]
    assert "💰 teilweise" in after[b1]
    assert len(inserts) == 1


def test_expenses_homeoffice_table_rows_unchanged_when_appending_software(tmp_path):
    """Appending to Software should NOT touch the Homeoffice-Pauschale
    monthly table (it's a pre-seeded calendar, not a transaction log)."""
    root = _stage(tmp_path, "EXPENSES")
    before = (root / "finances/2026/EXPENSES.md").read_text()
    b_start = before.find("## 🏠 Homeoffice-Pauschale")
    b_end = before.find("## 📱")
    b_block = before[b_start:b_end]

    rec = expenses.ExpenseRecord(
        date="2026-04-21", description="tool",
        category="software", net=99, ust=0,
    )
    expenses.append_expense(2026, rec, root)

    after = (root / "finances/2026/EXPENSES.md").read_text()
    a_start = after.find("## 🏠 Homeoffice-Pauschale")
    a_end = after.find("## 📱")
    a_block = after[a_start:a_end]
    assert a_block == b_block, "Homeoffice-Pauschale table was mutated by Software append"
