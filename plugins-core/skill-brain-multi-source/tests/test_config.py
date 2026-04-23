"""Tests for config loader / registry."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "src"))

import config as cfg  # noqa: E402


@pytest.fixture
def tmp_home(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.delenv("HARVEY_HOME", raising=False)
    return tmp_path


def test_load_registry_returns_default_when_config_absent(tmp_home):
    registry = cfg.load_registry()
    assert "default" in registry.names()
    assert registry.default_name == "default"


def test_add_source_creates_config_if_missing(tmp_home):
    path = cfg.add_source({
        "name": "personal",
        "type": "obsidian",
        "path": str(tmp_home / "vault"),
        "writable": True,
    })
    assert path.exists()
    data = json.loads(path.read_text())
    names = {s["name"] for s in data["sources"]}
    assert "default" in names  # seeded automatically
    assert "personal" in names


def test_add_source_replaces_existing_by_name(tmp_home):
    cfg.add_source({"name": "v", "type": "obsidian", "path": "/tmp/a"})
    cfg.add_source({"name": "v", "type": "obsidian", "path": "/tmp/b"})
    data = json.loads(cfg.config_path().read_text())
    v_entries = [s for s in data["sources"] if s["name"] == "v"]
    assert len(v_entries) == 1
    assert v_entries[0]["path"] == "/tmp/b"


def test_remove_source(tmp_home):
    cfg.add_source({"name": "extra", "type": "plain", "path": "/tmp/x"})
    cfg.remove_source("extra")
    data = json.loads(cfg.config_path().read_text())
    names = {s["name"] for s in data["sources"]}
    assert "extra" not in names


def test_cannot_remove_default(tmp_home):
    cfg.add_source({"name": "default", "type": "logseq", "path": "/tmp/d"})
    with pytest.raises(ValueError, match="cannot remove default"):
        cfg.remove_source("default")


def test_remove_nonexistent_raises(tmp_home):
    cfg.add_source({"name": "real", "type": "plain", "path": "/tmp/y"})
    with pytest.raises(KeyError):
        cfg.remove_source("ghost")


def test_set_default(tmp_home):
    cfg.add_source({"name": "vault", "type": "obsidian", "path": "/tmp/v"})
    cfg.set_default("vault")
    registry = cfg.load_registry()
    assert registry.default_name == "vault"


def test_set_default_unknown_raises(tmp_home):
    with pytest.raises(KeyError):
        cfg.set_default("nonexistent")


def test_registry_get_raises_on_unknown(tmp_home):
    registry = cfg.load_registry()
    with pytest.raises(KeyError):
        registry.get("does-not-exist")


def test_registry_get_default_works(tmp_home):
    registry = cfg.load_registry()
    default = registry.get_default()
    assert default.name == registry.default_name


def test_corrupt_config_falls_back_to_default(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("{not valid json")
    registry = cfg.load_registry()
    # Falls back to default instead of crashing
    assert len(registry.sources) >= 1
