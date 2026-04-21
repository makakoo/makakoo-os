"""End-to-end smoke: init → onboard → contract → log → invoice → expense →
pipeline → dashboard → kleinunternehmer. Assert filesystem shape + journal
tally."""
from __future__ import annotations

import argparse

from src.commands import dashboard as dashboard_cmd
from src.commands import doctor as doctor_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import kleinunternehmer_check as ku_cmd
from src.commands import log_hours as log_cmd
from src.commands import onboard_client as onboard_cmd
from src.commands import pipeline as pipeline_cmd
from src.commands import track_expense as expense_cmd


def _ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    return argparse.Namespace(**kw)


def test_full_flow(tmp_freelance_home, no_brain):
    # doctor on a just-init'd home should be green
    r = doctor_cmd.run(_ns())
    assert r["status"] == "ok"

    onboard_cmd.run(_ns(
        slug="northbound", name="Northbound GmbH", sector="SaaS",
        contact_email="ops@nb.com", ust_id="", b2b="true", client_country="DE",
        day_rate=1400, hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(_ns(
        client="northbound", project="platform-migration",
        title="Platform Migration", description="migrate to Rust", meilensteine="[]",
        total_days=40, rate=None,
    ))
    log_cmd.run(_ns(
        client="northbound", project="platform-migration", week=17,
        hours='{"Mo":8,"Di":8,"Mi":8}', day=None, hours_today=None, note="sprint",
    ))
    r_inv = invoice_cmd.run(_ns(
        client="northbound", project="platform-migration",
        amount_net=None, days=20, description="Sprint 1",
        leistungszeitraum="2026-04", invoice_number=None, issued="2026-04-21",
    ))
    assert r_inv["invoice_number"] == "INV-2026-001"
    assert r_inv["net"] == 28000.0

    expense_cmd.run(_ns(
        date="2026-04-21", amount_net=149, ust=28.31,
        category="software", description="lexoffice", receipt_ref="_________",
    ))

    pipeline_r = pipeline_cmd.run(_ns(status=None))
    assert len(pipeline_r["rows"]) == 1
    assert pipeline_r["totals"]["invoiced_net"] == 28000.0

    dash = dashboard_cmd.run(_ns())
    assert dash["active_clients"] == 1
    assert dash["earnings_ytd"] == 28000.0

    ku = ku_cmd.run(_ns())
    assert ku["applicable"] is False  # SETTINGS default = regular VAT

    # Filesystem shape
    for rel in (
        "clients/northbound/meta.yaml",
        "clients/northbound/projects/platform-migration/_project-tracker.md",
        "clients/northbound/projects/platform-migration/contracts/platform-migration-v1.md",
        "clients/northbound/projects/platform-migration/invoices/INV-2026-001.md",
        "finances/2026/EARNINGS.md",
        "finances/2026/EXPENSES.md",
        "finances/2026/_invoice_counter.json",
    ):
        assert (tmp_freelance_home / rel).is_file(), f"missing {rel}"

    # Brain journal tally — 5 write-side subcommands, each writes one line:
    # onboard / contract / log / invoice / expense. 5 total.
    assert len(no_brain) == 5, f"expected 5 journal lines, got {len(no_brain)}"
