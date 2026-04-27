"""freelance-office dashboard — union view."""
from __future__ import annotations

import argparse

from src.commands import onboard_client as onboard_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import log_hours as log_cmd
from src.commands import dashboard as cmd


def test_dashboard_empty_state(tmp_freelance_home, no_brain):
    r = cmd.run(argparse.Namespace(json=False, dry_run=False))
    assert r["active_clients"] == 0
    assert r["earnings_ytd"] == 0


def test_dashboard_with_data(tmp_freelance_home, no_brain):
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
        amount_net=5000, days=None, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    from datetime import date
    _, iso_week, _ = date.today().isocalendar()
    log_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        week=iso_week, hours='{"Mo":8,"Di":8}', day=None, hours_today=None, note="",
    ))

    r = cmd.run(argparse.Namespace(json=False, dry_run=False))
    assert r["active_clients"] == 1
    assert r["earnings_ytd"] == 5000.0
    assert r["hours_this_week"] == 16.0
    assert r["next_invoice_due"] is not None
    assert r["next_invoice_due"]["inv_no"] == "INV-2026-001"
