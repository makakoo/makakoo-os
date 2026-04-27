"""freelance-office onboard-client — sign a client."""
from __future__ import annotations

import argparse

import pytest

from src.commands import onboard_client as cmd
from src.core import client_meta
from src.core.errors import DuplicateClientError


def _ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("hourly_rate", None)
    kw.setdefault("payment_terms_days", 30)
    kw.setdefault("sector", "")
    kw.setdefault("contact_email", "")
    kw.setdefault("ust_id", "")
    kw.setdefault("b2b", "true")
    kw.setdefault("client_country", "DE")
    return argparse.Namespace(**kw)


def test_onboard_creates_client_dir_and_meta(tmp_freelance_home, no_brain):
    args = _ns(slug="northbound", name="Northbound GmbH", day_rate=1400)
    r = cmd.run(args)
    assert r["status"] == "ok"
    client_dir = tmp_freelance_home / "clients" / "northbound"
    assert client_dir.is_dir()
    meta = client_meta.ClientMeta.load(client_dir / "meta.yaml").flat()
    assert meta["slug"] == "northbound"
    assert meta["name"] == "Northbound GmbH"
    assert meta["day_rate_agreed"] == 1400
    assert meta["payment_terms_days"] == 30
    assert meta["current_status"] == "prospecting"
    assert no_brain[0].startswith("Onboarded [[northbound]]")


def test_onboard_duplicate_raises(tmp_freelance_home, no_brain):
    cmd.run(_ns(slug="dup", name="D", day_rate=1200))
    with pytest.raises(DuplicateClientError):
        cmd.run(_ns(slug="dup", name="D2", day_rate=1200))


def test_onboard_template_is_unchanged(tmp_freelance_home, no_brain):
    tpl = tmp_freelance_home / "clients" / "_template" / "meta.yaml"
    before = tpl.read_bytes()
    cmd.run(_ns(slug="alpha", name="Alpha", day_rate=1200))
    assert tpl.read_bytes() == before, "_template/meta.yaml must not be mutated"


def test_onboard_warns_on_below_floor_rate(tmp_freelance_home, no_brain):
    r = cmd.run(_ns(slug="cheap", name="Cheap GmbH", day_rate=500))
    assert any("below" in w.lower() or "floor" in w.lower() for w in r["warnings"])


def test_onboard_dry_run(tmp_freelance_home, no_brain):
    r = cmd.run(_ns(slug="drx", name="X", day_rate=1400, dry_run=True))
    assert r["dry_run"] is True
    assert not (tmp_freelance_home / "clients" / "drx").exists()


def test_onboard_client_respects_office_settings(tmp_path, monkeypatch, no_brain):
    """pi blocker #3 — new client's currency is inherited from the target
    office's SETTINGS.office.currency, not hardcoded EUR/de."""
    import argparse
    from src.commands import init as init_cmd
    from src.core.registry import OfficeRegistry
    from src.core import client_meta

    reg_path = tmp_path / "freelance_offices.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(reg_path))

    home_ar = tmp_path / "fo-ar"
    reg = OfficeRegistry.load(reg_path)
    reg.add("ar-main", home_ar, "AR", default=True)

    init_cmd.run(argparse.Namespace(
        json=False, dry_run=False, dba="", email="", office="ar-main",
    ))

    cmd.run(_ns(slug="buenosaires-co", name="BA Co", day_rate=1000, office="ar-main"))
    meta_path = home_ar / "clients" / "buenosaires-co" / "meta.yaml"
    flat = client_meta.ClientMeta.load(meta_path).flat()
    assert flat["currency"] == "ARS", (
        f"AR office onboard must set currency=ARS, got {flat['currency']!r}"
    )
