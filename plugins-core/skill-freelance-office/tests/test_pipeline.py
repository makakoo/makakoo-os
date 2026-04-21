"""freelance-office pipeline — read-only pipeline table."""
from __future__ import annotations

import argparse

from src.commands import onboard_client as onboard_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import pipeline as cmd


def _ns_pipeline(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("status", None)
    return argparse.Namespace(**kw)


def test_pipeline_empty_home(tmp_freelance_home, no_brain):
    r = cmd.run(_ns_pipeline())
    assert r["rows"] == []
    assert r["totals"]["invoiced_net"] == 0


def test_pipeline_with_one_invoiced_project(tmp_freelance_home, no_brain):
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
        amount_net=3000, days=None, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    r = cmd.run(_ns_pipeline())
    assert len(r["rows"]) == 1
    row = r["rows"][0]
    assert row["client"] == "c1"
    assert row["project"] == "p1"
    assert row["invoiced_net"] == 3000.0
    assert r["totals"]["invoiced_net"] == 3000.0


def test_pipeline_json_envelope_shape(tmp_freelance_home, no_brain):
    r = cmd.run(_ns_pipeline())
    for key in ("rows", "totals", "generated_at"):
        assert key in r
    for key in ("invoiced_net", "paid_net", "outstanding_net", "overdue_net"):
        assert key in r["totals"]
