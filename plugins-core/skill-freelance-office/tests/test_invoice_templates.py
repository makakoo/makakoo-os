"""Per-country invoice template rendering.

Smoke-tests each country's template with a representative context
and asserts the key labels + money figures are present. Golden-file
byte-lock for these would be too brittle (jinja whitespace quirks),
so we assert shape rather than exact bytes. Format fidelity for the
DE template is already covered by test_format_fidelity.py.
"""
from __future__ import annotations

from src.core import render


def _base_context(country="DE", **overrides):
    ctx = {
        "invoice_number": "INV-2026-001",
        "issued": "2026-04-21",
        "due": "2026-05-21",
        "leistungszeitraum": "2026-04-01 bis 2026-04-30",
        "project_slug": "demo-project",
        "description": "Sprint 1 deliverable",
        "days_billed": 10,
        "day_rate": 1400.0,
        "net": 14000.0,
        "ust": 2660.0,
        "brutto": 16660.0,
        "payment_terms_days": 30,
        "kleinunternehmer": False,
        "reverse_charge": False,
        "apply_vat": True,
        "from_name": "Sebastian Schkudlara",
        "from_dba": "Jevve Labs",
        "from_email": "seb@example.com",
        "from_ust_id": "DE123456789",
        "to_name": "Demo Client",
        "to_email": "ops@demo.com",
        "to_ust_id": "",
        "bank_iban": "DE00000000000000000000",
        "bank_bic": "BANKDEFF",
        "bank_name": "Demo Bank",
    }
    ctx.update(overrides)
    return ctx


def test_de_invoice_renders_german_labels():
    out = render.render_invoice(_base_context(), template_name="INVOICE.md.j2")
    assert "RECHNUNG" in out
    assert "Leistungszeitraum" in out
    assert "Fällig bis" in out
    assert "USt 19%" in out
    assert "2.660.00" in out or "2660.00" in out  # numeric format of VAT


def test_ar_invoice_renders_spanish_labels():
    ctx = _base_context(
        apply_vat=False,   # Monotributo default
        reverse_charge=False,
        ust=0.0,
        brutto=14000.0,
    )
    out = render.render_invoice(ctx, template_name="INVOICE_AR.md.j2")
    assert "FACTURA" in out
    assert "Monotributo" in out
    assert "Período de servicio" in out
    assert "Vencimiento" in out


def test_ar_responsable_inscripto_shows_iva_21():
    ctx = _base_context(
        apply_vat=True,
        ust=2940.0,  # 21% of 14000
        brutto=16940.0,
    )
    out = render.render_invoice(ctx, template_name="INVOICE_AR.md.j2")
    assert "IVA 21%" in out


def test_es_invoice_renders_spanish_labels():
    out = render.render_invoice(_base_context(), template_name="INVOICE_ES.md.j2")
    assert "FACTURA" in out
    assert "Base imponible" in out
    assert "IVA 21%" in out
    assert "Vencimiento" in out


def test_es_reverse_charge_renders_inversion():
    ctx = _base_context(
        apply_vat=False,
        reverse_charge=True,
        ust=0.0,
        brutto=14000.0,
    )
    out = render.render_invoice(ctx, template_name="INVOICE_ES.md.j2")
    assert "Inversión del sujeto pasivo" in out
    assert "IVA 21%" not in out  # explicitly no IVA charge
