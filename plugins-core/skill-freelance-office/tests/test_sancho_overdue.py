"""Phase 3 — ``freelance_invoice_overdue_tick`` SANCHO handler.

Covers the happy-path + every pi callout:

- Fires when an open invoice is past payment_terms + grace.
- Stays silent during the grace window.
- Does not fire twice on the same day for the same invoice.
- Telegram ping respects the configurable floor.
- State round-trip via ``overdue_notifications.json``.
- Read-then-confirm cancels the Telegram ping when ``mark-paid``
  slips in between the scan and the send (pi blocker #2).
- Orphan purge drops state entries for deleted invoices (pi
  corruption-risk #3).
"""
from __future__ import annotations

import argparse
from datetime import date
from pathlib import Path

import pytest

from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import mark_paid as mark_paid_cmd
from src.commands import onboard_client as onboard_cmd
from src.sancho_handlers import _state as state_mod
from src.sancho_handlers import _telegram as tg
from src.sancho_handlers import overdue_tick


@pytest.fixture
def _isolated_state(monkeypatch, tmp_path):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path / "makakoo"))


@pytest.fixture
def _captured_telegram(monkeypatch):
    captured = []

    def _fake(text, **kw):
        captured.append(text)

    tg.set_sender(_fake)
    yield captured
    tg.set_sender(None)


@pytest.fixture
def _captured_journal(monkeypatch):
    captured = []
    overdue_tick.set_journal_fn(lambda line: captured.append(line))
    yield captured
    # Restore to the real brain module-level handle.
    from src.core import brain
    overdue_tick.set_journal_fn(brain.append_journal_line)


def _scaffold(tmp_freelance_home, no_brain, *, issued: str = "2026-03-01",
              amount_net: float = 1000.0, terms: int = 30):
    sp = tmp_freelance_home / "_meta" / "SETTINGS.yaml"
    sp.write_text(
        sp.read_text(encoding="utf-8").replace(
            f"payment_terms_days: {30}",
            f"payment_terms_days: {terms}",
        ),
        encoding="utf-8",
    )
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="acme", name="Acme GmbH", sector="",
        contact_email="", ust_id="", b2b="true", client_country="DE",
        day_rate=1200, hourly_rate=None, payment_terms_days=terms,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="acme", project="p1",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))
    r = invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, amount_net=amount_net, days=None,
        description="x", leistungszeitraum="", invoice_number=None,
        issued=issued, client="acme", project="p1",
        pdf=False, force=False,
    ))
    return r["invoice_number"]


def test_overdue_fires_past_payment_terms_plus_grace(
    tmp_freelance_home, no_brain, _isolated_state, _captured_journal, _captured_telegram
):
    inv = _scaffold(tmp_freelance_home, no_brain, issued="2026-03-01", amount_net=1000.0)
    # today = 2026-04-15: 45 days after issued. With terms=30 + grace=7
    # the grace_end is 2026-04-07 → 8 days overdue.
    out = overdue_tick.tick(today=date(2026, 4, 15))
    assert inv in out["fired"]
    assert any(inv in line and "überfällig" in line for line in _captured_journal)


def test_overdue_silent_inside_grace_window(
    tmp_freelance_home, no_brain, _isolated_state, _captured_journal, _captured_telegram
):
    inv = _scaffold(tmp_freelance_home, no_brain, issued="2026-03-01", amount_net=1000.0)
    # terms=30 + grace=7 = 37 → due-plus-grace 2026-04-07. Today = 2026-04-05
    # is still inside the grace window.
    out = overdue_tick.tick(today=date(2026, 4, 5))
    assert out["fired"] == []
    assert _captured_journal == []


def test_overdue_does_not_fire_twice_on_same_day(
    tmp_freelance_home, no_brain, _isolated_state, _captured_journal, _captured_telegram
):
    inv = _scaffold(tmp_freelance_home, no_brain, issued="2026-03-01", amount_net=1000.0)
    overdue_tick.tick(today=date(2026, 4, 15))
    _captured_journal.clear()
    overdue_tick.tick(today=date(2026, 4, 15))
    assert _captured_journal == []


def test_overdue_telegram_floor_respected(
    tmp_freelance_home, no_brain, _isolated_state, _captured_journal, _captured_telegram
):
    """An invoice below the configured ping floor logs to Brain but
    does NOT page."""
    sp = tmp_freelance_home / "_meta" / "SETTINGS.yaml"
    sp.write_text(
        sp.read_text(encoding="utf-8").replace(
            "payment_terms_days: 30",
            "payment_terms_days: 30\n  overdue_ping_floor: 5000.0",
        ),
        encoding="utf-8",
    )
    _scaffold(tmp_freelance_home, no_brain, issued="2026-03-01", amount_net=1000.0)
    overdue_tick.tick(today=date(2026, 4, 15))
    assert _captured_journal  # journal ping fired
    assert _captured_telegram == []  # telegram floor = 5000 > 1000


def test_overdue_state_roundtrip(
    tmp_freelance_home, no_brain, _isolated_state, _captured_journal, _captured_telegram
):
    inv = _scaffold(tmp_freelance_home, no_brain, issued="2026-03-01", amount_net=1000.0)
    overdue_tick.tick(today=date(2026, 4, 15))
    reloaded = state_mod.load("overdue_notifications.json")
    assert inv in reloaded
    assert reloaded[inv]["last_notified"] == "2026-04-15"


def test_overdue_read_then_confirm_cancels_ping_on_concurrent_mark_paid(
    tmp_freelance_home, no_brain, _isolated_state, _captured_journal, monkeypatch
):
    """pi blocker #2: between the initial scan and the Telegram
    send, simulate a ``mark-paid`` in another terminal. The ping
    must be cancelled."""
    inv = _scaffold(tmp_freelance_home, no_brain, issued="2026-03-01", amount_net=1000.0)

    sent = []

    def _fake_tg(text, **kw):
        sent.append(text)

    tg.set_sender(_fake_tg)

    # Patch _find_row so it returns a paid status — simulating a
    # concurrent mark-paid landing between the scan and the send.
    original_find_row = overdue_tick._find_row

    def _race_find_row(office_root, inv_no):
        if inv_no == inv:
            return {"inv_no": inv_no, "status": "✅ bezahlt", "net": 1000.0,
                    "date": "2026-03-01", "client": "acme", "project": "p1",
                    "ust": 0, "brutto": 1000.0}
        return original_find_row(office_root, inv_no)

    monkeypatch.setattr(overdue_tick, "_find_row", _race_find_row)
    try:
        overdue_tick.tick(today=date(2026, 4, 15))
    finally:
        tg.set_sender(None)
    assert sent == [], sent


def test_overdue_orphan_purge(
    tmp_freelance_home, no_brain, _isolated_state, _captured_journal, _captured_telegram
):
    """pi corruption-risk #3: stale state entries for invoices that
    no longer exist must be dropped."""
    inv = _scaffold(tmp_freelance_home, no_brain, issued="2026-03-01", amount_net=1000.0)
    # Pre-seed a fake orphan alongside the real entry.
    state_mod.save(
        "overdue_notifications.json",
        {
            inv: {"last_notified": "2026-04-14", "office_id": "default",
                  "amount": 1000.0},
            "INV-2099-DEAD": {"last_notified": "2026-04-14",
                              "office_id": "default", "amount": 42000.0},
        },
    )
    out = overdue_tick.tick(today=date(2026, 4, 15))
    assert "INV-2099-DEAD" in out["orphans_purged"]
    reloaded = state_mod.load("overdue_notifications.json")
    assert "INV-2099-DEAD" not in reloaded
    # Real invoice stays + is bumped to today.
    assert inv in reloaded
    assert reloaded[inv]["last_notified"] == "2026-04-15"
