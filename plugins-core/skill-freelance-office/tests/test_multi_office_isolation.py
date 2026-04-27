"""pi gap #1 — multi-office isolation.

Two freelance-office installs coexist. Invoice counters, EARNINGS.md
files, and project trackers are strictly isolated — office A can
never see office B's data, and vice versa.
"""
from __future__ import annotations

import argparse
from pathlib import Path

import pytest

from src.commands import init as init_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import onboard_client as onboard_cmd
from src.core import invoice_counter, earnings
from src.core.registry import OfficeRegistry


@pytest.fixture
def two_offices(tmp_path, monkeypatch, no_brain):
    """Stage two independently-init'd offices + register both."""
    reg_path = tmp_path / "freelance_offices.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(reg_path))

    home_de = tmp_path / "fo-de"
    home_ar = tmp_path / "fo-ar"

    # register first so init can pick up office country
    reg = OfficeRegistry.load(reg_path)
    # Two DE offices — the test is about multi-office isolation, not AR VAT.
    # AR-specific tax behavior ships in Phase 3.
    reg.add("de-main", home_de, "DE", default=True)
    reg.add("ar-main", home_ar, "DE")

    def _init(office_id):
        ns = argparse.Namespace(
            json=False, dry_run=False, dba="", email="", office=office_id,
        )
        init_cmd.run(ns)

    _init("de-main")
    _init("ar-main")
    return home_de, home_ar


def test_invoice_counter_isolated_across_offices_same_year(two_offices):
    home_de, home_ar = two_offices
    # 3 allocations per office, same year
    de = [invoice_counter.allocate(2026, home_de) for _ in range(3)]
    ar = [invoice_counter.allocate(2026, home_ar) for _ in range(3)]
    de_nums = [n for (_, n, _) in de]
    ar_nums = [n for (_, n, _) in ar]
    assert de_nums == [1, 2, 3], f"DE counter wrong: {de_nums}"
    assert ar_nums == [1, 2, 3], f"AR counter wrong: {ar_nums}"
    # Both INV-2026-001 exist, one per office — NO collision
    de_file = home_de / "finances" / "2026" / "_invoice_counter.json"
    ar_file = home_ar / "finances" / "2026" / "_invoice_counter.json"
    assert de_file.is_file() and ar_file.is_file()
    assert de_file != ar_file


def test_earnings_isolated_across_offices(two_offices, no_brain):
    home_de, home_ar = two_offices
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", slug="dc", name="DE Client",
        sector="", contact_email="", ust_id="", b2b="true", client_country="DE",
        day_rate=1200, hourly_rate=None, payment_terms_days=30,
    ))
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", slug="ac", name="AR Client",
        sector="", contact_email="", ust_id="", b2b="true", client_country="AR",
        day_rate=1000, hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", client="dc", project="p",
        title="T", description="", meilensteine="[]", total_days=10, rate=None,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", client="ac", project="p",
        title="T", description="", meilensteine="[]", total_days=5, rate=None,
    ))
    invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", client="dc", project="p",
        amount_net=5000, days=None, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", client="ac", project="p",
        amount_net=3000, days=None, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))

    assert earnings.ytd_total(2026, home_de) == 5000.0
    assert earnings.ytd_total(2026, home_ar) == 3000.0
    de_text = (home_de / "finances" / "2026" / "EARNINGS.md").read_text()
    ar_text = (home_ar / "finances" / "2026" / "EARNINGS.md").read_text()
    assert "dc" in de_text and "ac" not in de_text
    assert "ac" in ar_text and "dc" not in ar_text


def test_tracker_isolated_across_offices(two_offices, no_brain):
    home_de, home_ar = two_offices
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", slug="nb", name="Nb",
        sector="", contact_email="", ust_id="", b2b="true", client_country="DE",
        day_rate=1200, hourly_rate=None, payment_terms_days=30,
    ))
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", slug="nb", name="Nb",
        sector="", contact_email="", ust_id="", b2b="true", client_country="AR",
        day_rate=1000, hourly_rate=None, payment_terms_days=30,
    ))
    # same client slug in both offices — must not collide
    assert (home_de / "clients" / "nb" / "meta.yaml").is_file()
    assert (home_ar / "clients" / "nb" / "meta.yaml").is_file()
    from src.core import client_meta
    de_flat = client_meta.ClientMeta.load(home_de / "clients" / "nb" / "meta.yaml").flat()
    ar_flat = client_meta.ClientMeta.load(home_ar / "clients" / "nb" / "meta.yaml").flat()
    # Both offices in this fixture are DE (see fixture note). The proper
    # AR-currency inheritance test lives in test_onboard_client.py ::
    # test_onboard_client_respects_office_settings — it uses an AR-country
    # office once Phase 3 ships.
    assert de_flat["currency"] == "EUR"
    assert ar_flat["currency"] == "EUR"
