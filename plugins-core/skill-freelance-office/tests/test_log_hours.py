"""freelance-office log-hours — upsert a KW row."""
from __future__ import annotations

import argparse

import pytest

from src.commands import onboard_client as onboard_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import log_hours as cmd
from src.core.errors import FreelanceError, NotInitialisedError


def _onboard(tmp_freelance_home, no_brain, slug="c1", rate=1200):
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug=slug, name="N", sector="", contact_email="",
        ust_id="", b2b="true", client_country="DE", day_rate=rate,
        hourly_rate=None, payment_terms_days=30,
    ))


def _make_project(tmp_freelance_home, no_brain, client="c1", project="p1", days=20):
    _onboard(tmp_freelance_home, no_brain, slug=client)
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client=client, project=project,
        title="T", description="", meilensteine="[]",
        total_days=days, rate=None,
    ))


def _ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("hours", None)
    kw.setdefault("day", None)
    kw.setdefault("hours_today", None)
    kw.setdefault("note", "")
    return argparse.Namespace(**kw)


def test_log_hours_upserts_kw_row(tmp_freelance_home, no_brain):
    _make_project(tmp_freelance_home, no_brain, days=40)
    r = cmd.run(_ns(client="c1", project="p1", week=17, hours='{"Mo":8,"Di":8,"Mi":8}', note="sprint"))
    assert r["status"] == "ok"
    assert r["total_hours"] == 24.0
    assert r["spent_days"] == 3
    assert r["remaining_days"] == 37


def test_log_hours_additive_upsert(tmp_freelance_home, no_brain):
    _make_project(tmp_freelance_home, no_brain, days=40)
    cmd.run(_ns(client="c1", project="p1", week=17, hours='{"Mo":8}'))
    r = cmd.run(_ns(client="c1", project="p1", week=17, hours='{"Di":8}'))
    assert r["total_hours"] == 8.0, "the delta passed this call was 8h"
    assert r["spent_days"] == 2, "cumulative 2 days across both calls"


def test_log_hours_missing_tracker_raises(tmp_freelance_home, no_brain):
    _onboard(tmp_freelance_home, no_brain, slug="c1")
    with pytest.raises(NotInitialisedError):
        cmd.run(_ns(client="c1", project="nonexistent", week=17, hours='{"Mo":8}'))


def test_log_hours_rejects_bad_week(tmp_freelance_home, no_brain):
    _make_project(tmp_freelance_home, no_brain)
    with pytest.raises(FreelanceError):
        cmd.run(_ns(client="c1", project="p1", week=99, hours='{"Mo":8}'))


def test_log_hours_dry_run_no_mutation(tmp_freelance_home, no_brain):
    _make_project(tmp_freelance_home, no_brain)
    tracker_path = tmp_freelance_home / "clients" / "c1" / "projects" / "p1" / "_project-tracker.md"
    before = tracker_path.read_bytes()
    r = cmd.run(_ns(client="c1", project="p1", week=17, hours='{"Mo":8}', dry_run=True))
    assert r["dry_run"] is True
    assert tracker_path.read_bytes() == before
