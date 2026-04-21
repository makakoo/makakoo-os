"""freelance-office track-expense — section-scoped append."""
from __future__ import annotations

import argparse

import pytest

from src.commands import track_expense as cmd
from src.core.errors import FreelanceError


def _ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("receipt_ref", "_________")
    kw.setdefault("ust", 0.0)
    kw.setdefault("date", "2026-04-21")
    return argparse.Namespace(**kw)


def test_track_software_expense(tmp_freelance_home, no_brain):
    r = cmd.run(_ns(category="software", amount_net=149, description="lexoffice"))
    assert r["status"] == "ok"
    assert r["ytd_by_category"]["software"] == 149.0


def test_track_equipment_over_800_adds_afa_warning(tmp_freelance_home, no_brain):
    r = cmd.run(_ns(category="equipment", amount_net=1200, description="Laptop"))
    assert any("AfA" in w for w in r["warnings"])


def test_track_unknown_category_rejected(tmp_freelance_home, no_brain):
    # argparse should reject via choices; directly invoking with a bad category hits expenses.append_expense
    with pytest.raises(FreelanceError):
        cmd.run(_ns(category="bogus", amount_net=10, description="x"))


def test_track_bad_date_raises(tmp_freelance_home, no_brain):
    with pytest.raises(FreelanceError):
        cmd.run(_ns(category="software", amount_net=10, description="x", date="not-a-date"))


def test_track_dry_run_no_mutation(tmp_freelance_home, no_brain):
    expenses_file = tmp_freelance_home / "finances" / "2026" / "EXPENSES.md"
    before = expenses_file.read_bytes()
    r = cmd.run(_ns(category="software", amount_net=10, description="x", dry_run=True))
    assert r["dry_run"] is True
    assert expenses_file.read_bytes() == before
