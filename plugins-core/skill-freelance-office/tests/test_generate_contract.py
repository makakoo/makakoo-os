"""freelance-office generate-contract — Projektvereinbarung + v-bump."""
from __future__ import annotations

import argparse

import pytest

from src.commands import onboard_client as onboard_cmd
from src.commands import generate_contract as cmd
from src.core.errors import FreelanceError


def _onboard(tmp_freelance_home, no_brain, slug="c1", rate=1400):
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug=slug, name="N", sector="", contact_email="",
        ust_id="", b2b="true", client_country="DE", day_rate=rate,
        hourly_rate=None, payment_terms_days=30,
    ))


def _ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("description", "")
    kw.setdefault("meilensteine", "[]")
    kw.setdefault("rate", None)
    return argparse.Namespace(**kw)


def test_first_contract_is_v1(tmp_freelance_home, no_brain):
    _onboard(tmp_freelance_home, no_brain)
    r = cmd.run(_ns(client="c1", project="p1", title="T", total_days=10))
    assert r["version"] == 1
    assert r["path"].endswith("/p1-v1.md")


def test_second_contract_bumps_v2(tmp_freelance_home, no_brain):
    _onboard(tmp_freelance_home, no_brain)
    cmd.run(_ns(client="c1", project="p1", title="T", total_days=10))
    r = cmd.run(_ns(client="c1", project="p1", title="T2", total_days=20))
    assert r["version"] == 2
    assert r["path"].endswith("/p1-v2.md")


def test_contract_total_net_equals_days_times_rate(tmp_freelance_home, no_brain):
    _onboard(tmp_freelance_home, no_brain, rate=1500)
    r = cmd.run(_ns(client="c1", project="p1", title="T", total_days=30))
    assert r["total_net"] == 45000.0


def test_contract_renders_milestones(tmp_freelance_home, no_brain):
    _onboard(tmp_freelance_home, no_brain)
    r = cmd.run(_ns(
        client="c1", project="p1", title="T", total_days=10,
        meilensteine='[{"name":"M1","description":"first","due_date":"2026-04-30"}]'
    ))
    path = tmp_freelance_home / "clients" / "c1" / "projects" / "p1" / "contracts" / "p1-v1.md"
    content = path.read_text()
    assert "M1" in content and "first" in content and "2026-04-30" in content


def test_contract_missing_client_raises(tmp_freelance_home, no_brain):
    with pytest.raises(FreelanceError):
        cmd.run(_ns(client="missing", project="p1", title="T", total_days=5))
