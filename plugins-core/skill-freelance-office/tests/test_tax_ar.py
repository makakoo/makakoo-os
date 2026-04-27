"""AR — Argentina tax regime."""
from __future__ import annotations

from dataclasses import dataclass
from types import SimpleNamespace

import pytest

from src.core.tax import ar


def _settings(ar_regime="monotributo", category="B", ku=False):
    """Build a minimal settings-shaped object."""
    tax_raw = {"ar_regime": ar_regime, "ar_monotributo_category": category}
    return SimpleNamespace(
        tax=SimpleNamespace(kleinunternehmer=ku),
        office=SimpleNamespace(country="AR"),
        raw={"tax": tax_raw},
    )


def test_monotributo_vat_zero():
    s = _settings(ar_regime="monotributo")
    vr = ar.vat_regime(s, {"client_country": "AR", "b2b": True, "ust_id": ""})
    assert vr.apply_vat is False
    assert vr.vat_rate == 0.0
    assert "Monotributo" in vr.label


def test_responsable_inscripto_adds_iva_21():
    s = _settings(ar_regime="responsable_inscripto")
    vr = ar.vat_regime(s, {"client_country": "AR", "b2b": True, "ust_id": ""})
    assert vr.apply_vat is True
    assert vr.vat_rate == 0.21
    assert "IVA 21%" in vr.label


def test_monotributo_category_b_green_under_80pct():
    s = _settings(ar_regime="monotributo", category="B")
    status = ar.check_threshold(s, 5_000_000.00)  # well under B's 11.4M
    assert status.level == "green"
    assert status.exit_code == 0


def test_monotributo_category_b_red_over_limit():
    s = _settings(ar_regime="monotributo", category="B")
    status = ar.check_threshold(s, 12_000_000.00)  # over B's 11.4M
    assert status.level == "red"
    assert status.exit_code == 2
    assert "category C" in status.message  # suggests upgrading to next tier


def test_responsable_inscripto_has_no_threshold():
    s = _settings(ar_regime="responsable_inscripto")
    status = ar.check_threshold(s, 999_999_999.00)
    assert status.level == "n/a"
    assert status.exit_code == 0
    assert "Responsable Inscripto" in status.message
