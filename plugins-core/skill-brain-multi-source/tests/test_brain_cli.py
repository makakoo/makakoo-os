"""Tests for legacy helper safety boundaries."""

from __future__ import annotations

import sys
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "src"))

import brain_cli  # noqa: E402
import config as cfg  # noqa: E402


def test_legacy_helper_refuses_unvalidated_okf(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.delenv("HARVEY_HOME", raising=False)
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    args = SimpleNamespace(
        name="catalog",
        type="okf",
        path=str(bundle),
        writable=False,
        read_only=True,
    )
    assert brain_cli.cmd_add(args) == 2
    assert "makakoo brain add" in capsys.readouterr().err
    assert not cfg.config_path().exists()


def test_legacy_helper_refuses_canonical_override(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.delenv("HARVEY_HOME", raising=False)
    args = SimpleNamespace(
        name="default",
        type="plain",
        path=str(tmp_path / "external"),
        writable=True,
        read_only=False,
    )

    assert brain_cli.cmd_add(args) == 2
    assert "cannot be replaced" in capsys.readouterr().err
    assert not cfg.config_path().exists()
