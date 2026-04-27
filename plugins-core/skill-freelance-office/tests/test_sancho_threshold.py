"""Phase 4 — ``freelance_threshold_tick`` SANCHO handler.

- First transition green → yellow per office per year fires once.
- Same yellow level the next day does NOT re-fire.
- Transition yellow → red fires a SECOND time.
- Per-office state isolation (DE at yellow + AR at green must NOT
  fire AR).
- Office-rename state isolation (pi gap #4): renaming ``de-main``
  to ``de-alt`` gives the new id a fresh notification history so
  it still fires at 80%.
"""
from __future__ import annotations

import argparse
from datetime import date
from pathlib import Path

import pytest

from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import init as init_cmd
from src.commands import onboard_client as onboard_cmd
from src.core.registry import OfficeRegistry
from src.sancho_handlers import _state as state_mod
from src.sancho_handlers import _telegram as tg
from src.sancho_handlers import threshold_tick


@pytest.fixture
def _isolated_state(monkeypatch, tmp_path):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path / "makakoo"))


@pytest.fixture
def _capture_journal(monkeypatch):
    lines = []
    threshold_tick.set_journal_fn(lambda l: lines.append(l))
    yield lines
    from src.core import brain
    threshold_tick.set_journal_fn(brain.append_journal_line)


@pytest.fixture
def _capture_tg():
    sent = []
    tg.set_sender(lambda text, **kw: sent.append(text))
    yield sent
    tg.set_sender(None)


def _setup_kleinunternehmer(home: Path):
    sp = home / "_meta" / "SETTINGS.yaml"
    sp.write_text(
        sp.read_text(encoding="utf-8").replace(
            "kleinunternehmer: false",
            "kleinunternehmer: true",
        ),
        encoding="utf-8",
    )


def _emit_earnings(office_root: Path, amount: float, *, year: int = 2026):
    """Directly mutate EARNINGS.md for ``office_root`` to a ytd_net
    equal to ``amount``. Faster than running generate-invoice N times."""
    from src.core import earnings
    path = earnings.earnings_path(year, office_root)
    text = path.read_text(encoding="utf-8")
    # Locate the Summe row and inject a single manual invoice row
    # above it with the desired net. Use the same format as template
    # rows (9 cols).
    lines = text.splitlines()
    summe_idx = next(i for i, ln in enumerate(lines) if "**Summe**" in ln or "**Suma**" in ln or "**Total**" in ln)
    amount_str = f"{amount:,.2f}".replace(",", "X").replace(".", ",").replace("X", ".")
    row = f"| 1 | INV-{year}-001 | test-client | test-project | {year}-01-10 | {amount_str} | 0,00 | {amount_str} | ⏳ offen |"
    lines.insert(summe_idx, row)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


@pytest.fixture
def _single_de_kleinunternehmer(tmp_freelance_home, no_brain):
    _setup_kleinunternehmer(tmp_freelance_home)
    return tmp_freelance_home


def test_threshold_fires_first_green_to_yellow_per_year(
    _single_de_kleinunternehmer, _isolated_state, _capture_journal, _capture_tg
):
    # 80% of 22000 = 17600. Push ytd past that.
    _emit_earnings(_single_de_kleinunternehmer, 18000.0)
    out = threshold_tick.tick(today=date(2026, 6, 1))
    assert any(f["level"] == "yellow" for f in out["fired"]), out
    assert _capture_tg, "telegram ping should have fired"
    assert any("YELLOW" in line for line in _capture_journal)


def test_threshold_no_refire_when_staying_at_yellow(
    _single_de_kleinunternehmer, _isolated_state, _capture_journal, _capture_tg
):
    _emit_earnings(_single_de_kleinunternehmer, 18000.0)
    threshold_tick.tick(today=date(2026, 6, 1))
    _capture_journal.clear()
    _capture_tg.clear()
    threshold_tick.tick(today=date(2026, 6, 2))
    assert _capture_journal == []
    assert _capture_tg == []


def test_threshold_fires_second_time_on_yellow_to_red(
    _single_de_kleinunternehmer, _isolated_state, _capture_journal, _capture_tg
):
    _emit_earnings(_single_de_kleinunternehmer, 18000.0)
    threshold_tick.tick(today=date(2026, 6, 1))
    # Clear captures and bump past 100%.
    _capture_journal.clear()
    _capture_tg.clear()
    # Remove the previous row and inject one past 22000.
    from src.core import earnings
    path = earnings.earnings_path(2026, _single_de_kleinunternehmer)
    text = path.read_text(encoding="utf-8")
    path.write_text(text.replace("18.000,00", "24.000,00"), encoding="utf-8")

    out = threshold_tick.tick(today=date(2026, 6, 20))
    assert any(f["level"] == "red" for f in out["fired"]), out
    assert _capture_tg


def test_threshold_per_office_state_isolation(
    tmp_path, monkeypatch, no_brain, _isolated_state, _capture_journal, _capture_tg
):
    """DE at yellow + AR at green ⇒ AR must NOT fire (Monotributo
    has its own thresholds and its own green baseline)."""
    reg_path = tmp_path / "reg.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(reg_path))

    home_de = tmp_path / "fo-de"
    home_ar = tmp_path / "fo-ar"
    reg = OfficeRegistry.load(reg_path)
    reg.add("de-main", home_de, "DE", default=True)
    reg.add("ar-main", home_ar, "AR")
    for oid in ("de-main", "ar-main"):
        init_cmd.run(argparse.Namespace(json=False, dry_run=False, dba="", email="", office=oid))

    _setup_kleinunternehmer(home_de)
    # Push DE past 80%, leave AR at zero.
    _emit_earnings(home_de, 18000.0)

    out = threshold_tick.tick(today=date(2026, 6, 1))
    fired_offices = {f["office_id"] for f in out["fired"]}
    assert "de-main" in fired_offices
    assert "ar-main" not in fired_offices


def test_threshold_office_rename_isolation(
    tmp_path, monkeypatch, no_brain, _isolated_state, _capture_journal, _capture_tg
):
    """pi gap #4: renaming ``de-main`` to ``de-alt`` must give the
    new id a fresh fired-history. Otherwise the old id's state would
    mask a legitimate yellow crossover on the new id."""
    reg_path = tmp_path / "reg.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(reg_path))

    home = tmp_path / "fo-de"
    reg = OfficeRegistry.load(reg_path)
    reg.add("de-main", home, "DE", default=True)
    init_cmd.run(argparse.Namespace(json=False, dry_run=False, dba="", email="", office="de-main"))
    _setup_kleinunternehmer(home)
    _emit_earnings(home, 18000.0)

    # 1st tick under de-main → yellow fires.
    threshold_tick.tick(today=date(2026, 6, 1))
    before_state = state_mod.load("threshold_notifications.json")
    assert "de-main" in before_state
    assert "fired_yellow_at" in before_state["de-main"]["2026"]

    _capture_journal.clear()
    _capture_tg.clear()

    # Simulate a rename: drop de-main, add de-alt at the same path.
    reg = OfficeRegistry.load(reg_path)
    reg.remove("de-main")
    reg.add("de-alt", home, "DE", default=True)

    out = threshold_tick.tick(today=date(2026, 6, 2))
    # The new id fires independently because it has no prior state.
    fired_ids = {f["office_id"] for f in out["fired"]}
    assert "de-alt" in fired_ids, out
    after_state = state_mod.load("threshold_notifications.json")
    # Old id remains in state (dangling but harmless — v0.4 can prune
    # it), but new id has its own year slot with fired_yellow_at.
    assert "de-alt" in after_state
    assert "fired_yellow_at" in after_state["de-alt"]["2026"]
