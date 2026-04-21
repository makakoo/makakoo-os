"""Pytest fixtures for freelance-office tests.

`tmp_freelance_home` stages a fresh tmp directory, points
``$FREELANCE_OFFICE_HOME`` at it, runs ``freelance-office init`` via
the in-process ``init.run(args)`` helper (no subprocess), and yields
the path. Every write-side test uses this — isolation + speed.

`no_brain` monkey-patches ``core.brain.append_journal_line`` to a
no-op returning a sentinel path. Keeps tests fast + hermetic (no
capability socket needed).
"""
from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path
from typing import List

import pytest

PLUGIN_ROOT = Path(__file__).resolve().parent.parent
if str(PLUGIN_ROOT) not in sys.path:
    sys.path.insert(0, str(PLUGIN_ROOT))

from src.commands import init as init_cmd  # noqa: E402


@pytest.fixture
def tmp_freelance_home(tmp_path: Path, monkeypatch) -> Path:
    home = tmp_path / "freelance-office"
    # Isolate BOTH the office root AND the registry — every test gets its
    # own registry so cross-test state never leaks. The registry lives
    # inside the test's tmp dir.
    monkeypatch.setenv("FREELANCE_OFFICE_HOME", str(home))
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(tmp_path / "freelance_offices.json"))
    ns = argparse.Namespace(json=False, dry_run=False, dba="", email="", office=None)
    result = init_cmd.run(ns)
    assert result["exit_code"] == 0, result
    return home


@pytest.fixture
def no_brain(monkeypatch):
    calls: List[str] = []

    def _fake(line: str):
        calls.append(line)
        return "/tmp/fake-journal-path"

    from src.core import brain as brain_mod
    monkeypatch.setattr(brain_mod, "append_journal_line", _fake)
    # Also patch every commands.* module that imported the function by name.
    for mod_name in (
        "src.commands.onboard_client",
        "src.commands.log_hours",
        "src.commands.generate_invoice",
        "src.commands.generate_contract",
        "src.commands.track_expense",
    ):
        mod = sys.modules.get(mod_name)
        if mod is not None:
            monkeypatch.setattr(mod, "brain", brain_mod)
    return calls


def make_args(**kw) -> argparse.Namespace:
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    return argparse.Namespace(**kw)
