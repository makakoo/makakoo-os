"""freelance-office init — bootstrap ~/freelance-office/."""
from __future__ import annotations

import argparse
import os
from pathlib import Path

import pytest

from src.commands import init as init_cmd


def _ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("dba", "")
    kw.setdefault("email", "")
    return argparse.Namespace(**kw)


def test_init_on_clean_home_creates_tree(tmp_path, monkeypatch):
    home = tmp_path / "fo"
    monkeypatch.setenv("FREELANCE_OFFICE_HOME", str(home))
    result = init_cmd.run(_ns())
    assert result["exit_code"] == 0
    for rel in (
        "_meta/SETTINGS.yaml",
        "_meta/RATES.yaml",
        "templates/INVOICE.md.j2",
        "templates/PROJECT_VEREINBARUNG.md.j2",
        "clients/_template/meta.yaml",
        "clients/_template/projects/_project-tracker.md",
    ):
        assert (home / rel).is_file(), f"missing {rel}"


def test_init_is_idempotent(tmp_path, monkeypatch):
    home = tmp_path / "fo"
    monkeypatch.setenv("FREELANCE_OFFICE_HOME", str(home))
    init_cmd.run(_ns())
    # snapshot mtimes
    files = {p: p.stat().st_mtime_ns for p in home.rglob("*") if p.is_file()}
    # re-run
    r2 = init_cmd.run(_ns())
    assert r2["exit_code"] == 0
    assert all(r2["created"] == [] or isinstance(c, str) and "[dir]" in c or "[file]" in c for c in r2.get("created", []))
    for p, mtime in files.items():
        assert p.stat().st_mtime_ns == mtime, f"{p} was modified"


def test_init_dry_run_does_not_create_files(tmp_path, monkeypatch):
    home = tmp_path / "fo"
    monkeypatch.setenv("FREELANCE_OFFICE_HOME", str(home))
    result = init_cmd.run(_ns(dry_run=True))
    assert result["status"] == "preview"
    assert result["dry_run"] is True
    assert not home.exists() or not any(home.rglob("*.yaml"))
