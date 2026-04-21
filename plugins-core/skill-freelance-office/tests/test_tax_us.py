"""US — United States tax regime."""
from __future__ import annotations

from types import SimpleNamespace

from src.core.tax import us


def _settings():
    return SimpleNamespace(
        tax=SimpleNamespace(kleinunternehmer=False),
        office=SimpleNamespace(country="US"),
        raw={"tax": {}},
    )


def test_us_invoice_has_no_federal_vat():
    s = _settings()
    vr = us.vat_regime(s, {"client_country": "US", "b2b": True, "ust_id": ""})
    assert vr.apply_vat is False
    assert vr.vat_rate == 0.0
    assert "No federal VAT" in vr.label


def test_us_has_no_revenue_threshold():
    s = _settings()
    status = us.check_threshold(s, 500_000.00)
    assert status.level == "n/a"
    assert status.exit_code == 0
    assert "nexus" in status.message.lower() or "state" in status.message.lower()


def test_us_reuses_base_templates():
    # pi scope-cut #1: US does NOT get bespoke templates.
    assert us.INVOICE_TEMPLATE == "INVOICE.md.j2"
    assert us.EXPENSES_TEMPLATE == "EXPENSES.md.j2"


def test_us_schedule_c_categories_present():
    expected = {"equipment", "software", "education", "home_office",
                "communications", "travel", "supplies"}
    assert set(us.EXPENSE_CATEGORIES.keys()) == expected
