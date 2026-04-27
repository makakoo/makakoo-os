"""ES — Spain tax regime."""
from __future__ import annotations

from types import SimpleNamespace

from src.core.tax import es


def _settings(recargo=False):
    return SimpleNamespace(
        tax=SimpleNamespace(kleinunternehmer=False),
        office=SimpleNamespace(country="ES"),
        raw={"tax": {"recargo_equivalencia": recargo}},
    )


def test_domestic_es_client_gets_iva_21():
    s = _settings()
    vr = es.vat_regime(s, {"client_country": "ES", "b2b": True, "ust_id": ""})
    assert vr.apply_vat is True
    assert vr.vat_rate == 0.21
    assert "IVA 21%" in vr.label
    assert vr.reverse_charge is False


def test_eu_b2b_with_vat_id_triggers_inversion():
    s = _settings()
    vr = es.vat_regime(
        s, {"client_country": "DE", "b2b": True, "ust_id": "DE123456789"}
    )
    assert vr.apply_vat is False
    assert vr.reverse_charge is True
    assert "Inversión" in vr.label


def test_non_eu_b2b_is_out_of_scope():
    s = _settings()
    vr = es.vat_regime(
        s, {"client_country": "US", "b2b": True, "ust_id": ""}
    )
    assert vr.apply_vat is False
    assert vr.reverse_charge is False
    assert "extracomunitario" in vr.label


def test_recargo_de_equivalencia_adds_surcharge():
    s = _settings(recargo=True)
    vr = es.vat_regime(s, {"client_country": "ES", "b2b": True, "ust_id": ""})
    assert vr.apply_vat is True
    assert vr.vat_rate == 0.21
    assert "Recargo" in vr.label
    assert vr.extras and vr.extras.get("recargo_equivalencia_rate") == 0.052


def test_no_revenue_threshold():
    s = _settings()
    status = es.check_threshold(s, 5_000.00)
    assert status.level == "n/a"
    assert status.exit_code == 0
