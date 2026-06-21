"""Tests for Makakoo OS auto-update SANCHO tick."""
from __future__ import annotations

import importlib.util
from pathlib import Path

HERE = Path(__file__).resolve().parent
SRC = HERE.parent / "src" / "tick.py"
spec = importlib.util.spec_from_file_location("tick", SRC)
tick = importlib.util.module_from_spec(spec)
assert spec and spec.loader
spec.loader.exec_module(tick)


def test_missing_config_defaults_manual_until_setup_writes_choice(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.delenv("MAKAKOO_UPDATE_MODE", raising=False)
    assert tick._read_mode(tmp_path) == "manual"


def test_config_auto_mode(tmp_path, monkeypatch):
    monkeypatch.delenv("MAKAKOO_UPDATE_MODE", raising=False)
    cfg = tmp_path / "config"
    cfg.mkdir()
    (cfg / "updates.toml").write_text('mode = "auto" # enable scheduled updates\n')
    assert tick._read_mode(tmp_path) == "auto"


def test_config_manual_mode(tmp_path, monkeypatch):
    monkeypatch.delenv("MAKAKOO_UPDATE_MODE", raising=False)
    cfg = tmp_path / "config"
    cfg.mkdir()
    (cfg / "updates.toml").write_text('mode = "manual"\n')
    assert tick._read_mode(tmp_path) == "manual"


def test_malformed_config_defaults_manual(tmp_path, monkeypatch):
    monkeypatch.delenv("MAKAKOO_UPDATE_MODE", raising=False)
    cfg = tmp_path / "config"
    cfg.mkdir()
    (cfg / "updates.toml").write_text('mode = "bogus"\n')
    assert tick._read_mode(tmp_path) == "manual"


def test_env_override_wins(tmp_path, monkeypatch):
    cfg = tmp_path / "config"
    cfg.mkdir()
    (cfg / "updates.toml").write_text('mode = "manual"\n')
    monkeypatch.setenv("MAKAKOO_UPDATE_MODE", "auto")
    assert tick._read_mode(tmp_path) == "auto"


def test_extract_delta():
    output = """
# version delta:
  before: makakoo 0.1.30 (abc)
  after:  makakoo 0.1.31 (def)
"""
    assert tick._extract_delta(output) == ("makakoo 0.1.30 (abc)", "makakoo 0.1.31 (def)")


def test_extract_delta_requires_version_delta_marker():
    output = """
package output:
  before: unrelated
  after: unrelated-new
"""
    assert tick._extract_delta(output) is None


def test_extract_delta_ignores_later_unrelated_before_after_lines():
    output = """
# version delta:
  before: makakoo 0.1.30 (abc)
  after:  makakoo 0.1.31 (def)
hook output:
  before: unrelated
  after: unrelated-new
"""
    assert tick._extract_delta(output) == ("makakoo 0.1.30 (abc)", "makakoo 0.1.31 (def)")


def test_invalid_timeout_falls_back(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_UPDATE_TIMEOUT", "bogus")
    assert tick._update_timeout() == 1200
    monkeypatch.setenv("MAKAKOO_UPDATE_TIMEOUT", "-1")
    assert tick._update_timeout() == 60


def test_single_flight_lock_blocks_nested_tick(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    with tick._single_flight_lock() as first:
        assert first is True
        with tick._single_flight_lock() as second:
            assert second is False


def test_spawn_failure_is_journaled(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.setenv("MAKAKOO_BIN", str(tmp_path / "missing-makakoo"))
    monkeypatch.delenv("MAKAKOO_UPDATE_MODE", raising=False)
    cfg = tmp_path / "config"
    cfg.mkdir()
    (cfg / "updates.toml").write_text('mode = "auto"\n')

    assert tick._main_locked() == 4
    journals = list((tmp_path / "data" / "Brain" / "journals").glob("*.md"))
    assert journals
    assert "could not start" in journals[0].read_text()
