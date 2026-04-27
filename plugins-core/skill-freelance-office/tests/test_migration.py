"""v0.1 -> v0.2 migration — auto-upgrade, idempotency, concurrent seed."""
from __future__ import annotations

import multiprocessing as mp
from pathlib import Path

import pytest

from src.core.migration import ensure_v02
from src.core.registry import OfficeRegistry


@pytest.fixture
def v01_home(tmp_path, monkeypatch):
    """Stage a v0.1-shaped filesystem (SETTINGS.yaml WITHOUT office: block)."""
    home = tmp_path / "freelance-office"
    (home / "_meta").mkdir(parents=True)
    (home / "_meta" / "SETTINGS.yaml").write_text(
        "# v0.1 settings file\n"
        "identity:\n"
        '  name: "Test Name"\n'
        "tax:\n"
        "  kleinunternehmer: false\n",
        encoding="utf-8",
    )
    reg_path = tmp_path / "freelance_offices.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(reg_path))
    monkeypatch.setenv("FREELANCE_OFFICE_HOME", str(home))
    return home, reg_path


def test_first_run_seeds_registry_and_injects_office_block(v01_home):
    home, reg_path = v01_home
    before = (home / "_meta" / "SETTINGS.yaml").read_text()

    summary = ensure_v02()

    assert summary["seeded_registry"] is True
    assert summary["noop"] is False
    # registry has exactly one entry pointing at the v0.1 home
    reg = OfficeRegistry.load(reg_path)
    assert len(reg) == 1
    assert reg.default == "default"
    assert reg.offices["default"].country == "DE"
    assert reg.offices["default"].path == home.resolve()
    # SETTINGS.yaml has the office: block injected exactly once
    after = (home / "_meta" / "SETTINGS.yaml").read_text()
    assert after.count("\noffice:\n") + (1 if after.startswith("office:\n") else 0) == 1
    # Every v0.1 line still present (prose + identity + tax)
    for line in before.splitlines():
        assert line in after, f"v0.1 line dropped: {line!r}"


def test_idempotent_second_call_is_noop(v01_home):
    home, reg_path = v01_home
    ensure_v02()
    snapshot = (home / "_meta" / "SETTINGS.yaml").read_text()

    summary2 = ensure_v02()

    assert summary2["seeded_registry"] is False
    assert summary2["injected_blocks"] == []
    assert summary2["noop"] is True
    # SETTINGS.yaml untouched on the second run
    assert (home / "_meta" / "SETTINGS.yaml").read_text() == snapshot
    # Exactly ONE office: block, not two
    text = (home / "_meta" / "SETTINGS.yaml").read_text()
    assert text.count("office:") == 1


def _race_worker():
    from src.core.migration import ensure_v02
    return ensure_v02()


def test_concurrent_seed_is_race_safe(v01_home):
    """Two processes call ensure_v02() simultaneously on a fresh v0.1
    tree: exactly one seeds the registry, the other no-ops. One
    office: block ends up in SETTINGS.yaml."""
    home, reg_path = v01_home
    ctx = mp.get_context("fork")
    with ctx.Pool(2) as pool:
        # spawn with FREELANCE_OFFICES_REGISTRY already set via monkeypatch
        # (fork inherits the parent's env)
        results = pool.starmap(_race_worker, [()] * 2)
    # At least one claim of seeded=True must exist (could be both if the
    # race resolves as "both see empty then both seed" — our lock+recheck
    # should prevent that, so expect exactly one True)
    seeded_flags = sorted(r["seeded_registry"] for r in results)
    assert seeded_flags == [False, True], (
        f"expected exactly one seed winner, got {seeded_flags}"
    )
    reg = OfficeRegistry.load(reg_path)
    assert len(reg) == 1
    assert (home / "_meta" / "SETTINGS.yaml").read_text().count("office:") == 1


def test_missing_settings_file_does_not_crash(tmp_path, monkeypatch):
    """If FREELANCE_OFFICE_HOME doesn't even have a SETTINGS.yaml, the
    migration still seeds the registry — the office-block injection
    step silently skips for missing files."""
    home = tmp_path / "fo-empty"
    reg_path = tmp_path / "freelance_offices.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(reg_path))
    monkeypatch.setenv("FREELANCE_OFFICE_HOME", str(home))

    summary = ensure_v02()
    assert summary["seeded_registry"] is True
    assert summary["injected_blocks"] == []
    reg = OfficeRegistry.load(reg_path)
    assert len(reg) == 1


def test_preexisting_office_block_is_untouched(tmp_path, monkeypatch):
    """v0.2 install already has office: in SETTINGS.yaml — migration
    must NOT double-inject."""
    home = tmp_path / "fo-v02"
    (home / "_meta").mkdir(parents=True)
    v02_content = (
        "office:\n"
        '  id: "default"\n'
        '  country: "DE"\n'
        '  locale: "de-DE"\n'
        '  currency: "EUR"\n'
        '  invoice_language: "de"\n'
        "\n"
        "identity:\n"
        '  name: "Pre-existing"\n'
    )
    (home / "_meta" / "SETTINGS.yaml").write_text(v02_content)
    reg_path = tmp_path / "freelance_offices.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(reg_path))
    monkeypatch.setenv("FREELANCE_OFFICE_HOME", str(home))

    ensure_v02()
    after = (home / "_meta" / "SETTINGS.yaml").read_text()
    assert after == v02_content
    assert after.count("office:") == 1


def test_migration_does_not_touch_non_v01_content(v01_home):
    """Injecting `office:` block must preserve every other byte."""
    home, _ = v01_home
    before = (home / "_meta" / "SETTINGS.yaml").read_text()
    ensure_v02()
    after = (home / "_meta" / "SETTINGS.yaml").read_text()
    # Strip the added office: block (first ~7 lines) and compare the rest.
    after_body = after.split("\nidentity:", 1)[1]
    before_body = before.split("\nidentity:", 1)[1]
    assert before_body == after_body
