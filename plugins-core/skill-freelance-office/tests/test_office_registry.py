"""Office registry — CRUD + sidecar-lock race."""
from __future__ import annotations

import multiprocessing as mp
from pathlib import Path

import pytest

from src.core.registry import (
    DuplicateOfficeError,
    OfficeRegistry,
    UnknownOfficeError,
)


@pytest.fixture
def reg_path(tmp_path, monkeypatch):
    p = tmp_path / "freelance_offices.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(p))
    return p


def test_empty_registry_loads_cleanly(reg_path):
    r = OfficeRegistry.load(reg_path)
    assert len(r) == 0
    assert r.default is None
    assert r.list_ids() == []


def test_add_sets_default_when_first(reg_path):
    r = OfficeRegistry.load(reg_path)
    r.add("de-main", Path("/tmp/fo-de"), "DE")
    r2 = OfficeRegistry.load(reg_path)
    assert r2.default == "de-main"
    assert "de-main" in r2.offices
    assert r2.offices["de-main"].country == "DE"


def test_add_duplicate_raises(reg_path):
    r = OfficeRegistry.load(reg_path)
    r.add("de-main", Path("/tmp/fo-de"), "DE")
    r = OfficeRegistry.load(reg_path)
    with pytest.raises(DuplicateOfficeError):
        r.add("de-main", Path("/tmp/fo-x"), "DE")


def test_remove_unknown_raises(reg_path):
    r = OfficeRegistry.load(reg_path)
    r.add("de-main", Path("/tmp/fo-de"), "DE")
    with pytest.raises(UnknownOfficeError) as exc:
        r.remove("does-not-exist")
    # Error message must name registered offices
    assert "de-main" in str(exc.value)


def test_remove_reassigns_default(reg_path):
    r = OfficeRegistry.load(reg_path)
    r.add("de-main", Path("/tmp/fo-de"), "DE")
    r.add("ar-main", Path("/tmp/fo-ar"), "AR")
    r.remove("de-main")
    r2 = OfficeRegistry.load(reg_path)
    assert r2.default == "ar-main"


def test_set_default_requires_known_office(reg_path):
    r = OfficeRegistry.load(reg_path)
    r.add("de-main", Path("/tmp/fo-de"), "DE")
    with pytest.raises(UnknownOfficeError):
        r.set_default("nonexistent")


def _race_worker(reg_path_str, office_id, path_str):
    from src.core.registry import OfficeRegistry, DuplicateOfficeError
    r = OfficeRegistry.load(Path(reg_path_str))
    try:
        r.add(office_id, Path(path_str), "DE")
        return ("ok", office_id)
    except DuplicateOfficeError:
        return ("dup", office_id)


def test_concurrent_add_same_id_race_safe(reg_path):
    """Two processes attempting to add the same id simultaneously:
    one wins, the other sees DuplicateOfficeError. No corruption."""
    ctx = mp.get_context("fork")
    with ctx.Pool(2) as pool:
        results = pool.starmap(
            _race_worker,
            [(str(reg_path), "de-main", "/tmp/fo-de")] * 2,
        )
    outcomes = sorted(r[0] for r in results)
    assert outcomes == ["dup", "ok"], f"expected exactly one winner, got {outcomes}"
    # Exactly one entry survives
    r = OfficeRegistry.load(reg_path)
    assert len(r.offices) == 1
    assert r.offices["de-main"].country == "DE"
