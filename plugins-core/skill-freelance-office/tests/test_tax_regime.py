"""Tax regime plug-in dispatch."""
from __future__ import annotations

import pytest

from src.core.tax import UnsupportedCountryError, get_regime


def test_get_regime_de_returns_module():
    regime = get_regime("DE")
    assert regime.ISO_CODE == "DE"
    assert regime.CURRENCY == "EUR"
    assert hasattr(regime, "vat_regime")
    assert hasattr(regime, "check_threshold")


def test_get_regime_is_case_insensitive():
    r_upper = get_regime("DE")
    r_lower = get_regime("de")
    r_mixed = get_regime("De")
    assert r_upper is r_lower is r_mixed


def test_get_regime_unknown_country_raises():
    with pytest.raises(UnsupportedCountryError) as exc:
        get_regime("XX")
    # Error message must name installed modules
    assert "DE" in str(exc.value)


def test_get_regime_empty_country_raises():
    with pytest.raises(UnsupportedCountryError):
        get_regime("")
