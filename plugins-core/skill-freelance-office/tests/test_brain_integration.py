"""Brain integration — mock makakoo_client.Client and verify every
write-side subcommand emits one outliner line with [[freelance-office]]
tag and the expected wikilink format.

The ``no_brain`` fixture in conftest.py already monkey-patches
``core.brain.append_journal_line`` to capture lines. These tests
inspect the captured buffer.
"""
from __future__ import annotations

import argparse

from src.commands import onboard_client as onboard_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import log_hours as log_cmd
from src.commands import track_expense as expense_cmd


def _setup_project(tmp_freelance_home, no_brain, rate=1200):
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="c1", name="N", sector="", contact_email="",
        ust_id="", b2b="true", client_country="DE", day_rate=rate,
        hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))


def test_onboard_writes_brain_line(tmp_freelance_home, no_brain):
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="acme", name="ACME", sector="", contact_email="",
        ust_id="", b2b="true", client_country="DE", day_rate=1200,
        hourly_rate=None, payment_terms_days=30,
    ))
    assert len(no_brain) == 1
    assert "[[acme]]" in no_brain[0]
    assert "[[freelance-office]]" in no_brain[0]


def test_log_hours_writes_brain_line(tmp_freelance_home, no_brain):
    _setup_project(tmp_freelance_home, no_brain)
    log_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        week=17, hours='{"Mo":8}', day=None, hours_today=None, note="",
    ))
    last = no_brain[-1]
    assert "[[c1/p1]]" in last
    assert "KW17" in last
    assert "[[freelance-office]]" in last


def test_generate_invoice_writes_brain_line(tmp_freelance_home, no_brain):
    _setup_project(tmp_freelance_home, no_brain)
    invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        amount_net=1000, days=None, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    last = no_brain[-1]
    assert "[[INV-2026-001]]" in last
    assert "[[c1]]" in last
    assert "[[freelance-office]]" in last


def test_contract_writes_brain_line(tmp_freelance_home, no_brain):
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="c1", name="N", sector="", contact_email="",
        ust_id="", b2b="true", client_country="DE", day_rate=1200,
        hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="c1", project="p1",
        title="T", description="", meilensteine="[]", total_days=10, rate=None,
    ))
    last = no_brain[-1]
    assert "contract v1" in last
    assert "[[c1/p1]]" in last


def test_track_expense_writes_brain_line(tmp_freelance_home, no_brain):
    expense_cmd.run(argparse.Namespace(
        json=False, dry_run=False, date="2026-04-21",
        amount_net=50, ust=0, category="software", description="GitHub Copilot",
        receipt_ref="_________",
    ))
    last = no_brain[-1]
    assert "Expense logged" in last
    assert "software" in last
    assert "[[freelance-office]]" in last


def test_dry_run_does_not_touch_brain(tmp_freelance_home, no_brain):
    _setup_project(tmp_freelance_home, no_brain)
    before = len(no_brain)
    invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=True, client="c1", project="p1",
        amount_net=500, days=None, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    assert len(no_brain) == before, "dry-run must not hit brain"
