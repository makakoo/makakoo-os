"""freelance-office doctor — read-only sanity check."""
from __future__ import annotations

import argparse

import pytest

from src.commands import doctor as doctor_cmd


def _ns():
    return argparse.Namespace(json=False, dry_run=False)


def test_doctor_green_on_fresh_init(tmp_freelance_home):
    r = doctor_cmd.run(_ns())
    assert r["status"] == "ok"
    assert r["exit_code"] == 0
    check_names = [c["name"] for c in r["checks"]]
    assert "home" in check_names
    assert "SETTINGS.yaml" in check_names
    assert "RATES.yaml" in check_names
    assert "invoice_counter" in check_names


def test_doctor_red_when_home_missing(tmp_path, monkeypatch):
    monkeypatch.setenv("FREELANCE_OFFICE_HOME", str(tmp_path / "does-not-exist"))
    r = doctor_cmd.run(_ns())
    assert r["status"] == "red"
    assert r["exit_code"] == 1


def test_doctor_red_when_settings_corrupt(tmp_freelance_home):
    (tmp_freelance_home / "_meta" / "SETTINGS.yaml").write_text("not: valid: yaml: [[[\n")
    r = doctor_cmd.run(_ns())
    # SETTINGS check itself fails
    settings_check = next(c for c in r["checks"] if c["name"] == "SETTINGS.yaml")
    assert settings_check["ok"] is False
    assert r["exit_code"] == 1
