"""freelance-office kleinunternehmer-check — §19 UStG YTD vs €22.000."""
from __future__ import annotations

import argparse

from src.commands import onboard_client as onboard_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import kleinunternehmer_check as cmd


def _flip_ku(home, value=True):
    sp = home / "_meta" / "SETTINGS.yaml"
    text = sp.read_text()
    if value:
        text = text.replace("kleinunternehmer: false", "kleinunternehmer: true")
    else:
        text = text.replace("kleinunternehmer: true", "kleinunternehmer: false")
    sp.write_text(text)


def test_not_applicable_when_regular_vat(tmp_freelance_home, no_brain):
    r = cmd.run(argparse.Namespace(json=False, dry_run=False))
    assert r["exit_code"] == 0
    assert r["applicable"] is False


def test_green_under_80pct(tmp_freelance_home, no_brain):
    _flip_ku(tmp_freelance_home, True)
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="c1", name="N", sector="", contact_email="",
        ust_id="", b2b="true", client_country="DE", day_rate=1200,
        hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        title="T", description="", meilensteine="[]", total_days=10, rate=None,
    ))
    invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        amount_net=5000, days=None, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    r = cmd.run(argparse.Namespace(json=False, dry_run=False))
    assert r["status"] == "green"
    assert r["exit_code"] == 0
    assert r["pct_used"] < 80


def test_red_exit_2_at_or_above_limit(tmp_freelance_home, no_brain):
    _flip_ku(tmp_freelance_home, True)
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="c1", name="N", sector="", contact_email="",
        ust_id="", b2b="true", client_country="DE", day_rate=1400,
        hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))
    invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        amount_net=23000, days=None, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    r = cmd.run(argparse.Namespace(json=False, dry_run=False))
    assert r["status"] == "red"
    assert r["exit_code"] == 2
