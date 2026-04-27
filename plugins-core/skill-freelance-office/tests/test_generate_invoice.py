"""freelance-office generate-invoice — rate precedence + VAT regime."""
from __future__ import annotations

import argparse

import pytest

from src.commands import onboard_client as onboard_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as cmd
from src.core import settings, paths
from src.core.errors import FreelanceError


def _setup(tmp_freelance_home, no_brain, country="DE", b2b="true", ust_id="", rate=1200, ku=False):
    if ku:
        sp = tmp_freelance_home / "_meta" / "SETTINGS.yaml"
        sp.write_text(sp.read_text().replace("kleinunternehmer: false", "kleinunternehmer: true"))
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="c1", name="N", sector="", contact_email="",
        ust_id=ust_id, b2b=b2b, client_country=country, day_rate=rate,
        hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))


def _ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("amount_net", None)
    kw.setdefault("days", None)
    kw.setdefault("description", "x")
    kw.setdefault("leistungszeitraum", "")
    kw.setdefault("invoice_number", None)
    kw.setdefault("issued", None)
    return argparse.Namespace(**kw)


def test_rate_precedence_amount_net_wins(tmp_freelance_home, no_brain):
    _setup(tmp_freelance_home, no_brain)
    r = cmd.run(_ns(client="c1", project="p1", amount_net=5000))
    assert r["net"] == 5000.0


def test_rate_precedence_days_uses_client_rate(tmp_freelance_home, no_brain):
    _setup(tmp_freelance_home, no_brain, rate=1500)
    r = cmd.run(_ns(client="c1", project="p1", days=10))
    assert r["net"] == 15000.0


def test_rate_precedence_neither_raises(tmp_freelance_home, no_brain):
    _setup(tmp_freelance_home, no_brain)
    with pytest.raises(FreelanceError):
        cmd.run(_ns(client="c1", project="p1"))


def test_kleinunternehmer_no_vat(tmp_freelance_home, no_brain):
    _setup(tmp_freelance_home, no_brain, ku=True)
    r = cmd.run(_ns(client="c1", project="p1", amount_net=1000))
    assert r["regime"]["kleinunternehmer"] is True
    assert r["ust"] == 0.0
    assert r["brutto"] == 1000.0


def test_reverse_charge_eu_b2b(tmp_freelance_home, no_brain):
    _setup(tmp_freelance_home, no_brain, country="AT", b2b="true", ust_id="ATU12345678")
    r = cmd.run(_ns(client="c1", project="p1", amount_net=1000))
    assert r["regime"]["reverse_charge"] is True
    assert r["ust"] == 0.0


def test_domestic_b2b_adds_19pct(tmp_freelance_home, no_brain):
    _setup(tmp_freelance_home, no_brain, country="DE")
    r = cmd.run(_ns(client="c1", project="p1", amount_net=1000))
    assert r["regime"]["apply_vat"] is True
    assert r["ust"] == 190.0
    assert r["brutto"] == 1190.0


def test_invoice_number_atomic_series(tmp_freelance_home, no_brain):
    _setup(tmp_freelance_home, no_brain)
    r1 = cmd.run(_ns(client="c1", project="p1", amount_net=100))
    r2 = cmd.run(_ns(client="c1", project="p1", amount_net=100))
    r3 = cmd.run(_ns(client="c1", project="p1", amount_net=100))
    assert r1["invoice_number"] == "INV-2026-001"
    assert r2["invoice_number"] == "INV-2026-002"
    assert r3["invoice_number"] == "INV-2026-003"


def test_dry_run_preview_does_not_allocate(tmp_freelance_home, no_brain):
    _setup(tmp_freelance_home, no_brain)
    r = cmd.run(_ns(client="c1", project="p1", amount_net=500, dry_run=True))
    assert r["dry_run"] is True
    # Counter not written (file may not exist yet since we dry-ran first call)
    from src.core import invoice_counter
    assert invoice_counter.peek(2026) == 0


def test_generate_invoice_envelope_carries_summary_field(tmp_freelance_home, no_brain):
    """Phase 5 / pi scope-undershoot: every generate-invoice envelope
    ships a post-generate ``summary`` line so callers don't need to
    run ``dashboard`` to see where things stand."""
    _setup(tmp_freelance_home, no_brain, ku=True)
    r = cmd.run(_ns(client="c1", project="p1", amount_net=5000))
    assert "summary" in r, r
    assert "open invoices" in r["summary"]
    # With kleinunternehmer=true the threshold is meaningful → expect
    # a percentage in the summary.
    assert "%" in r["summary"]
