"""ES — Spain tax regime.

Autónomo (self-employed freelance):

- Default: **IVA 21%** on every service invoice to domestic clients.
- **Inversión del sujeto pasivo** (reverse charge) for EU B2B clients
  with a valid EU VAT number (NIF-IVA).
- Special case: **Recargo de Equivalencia** — a small-retailer regime
  where the supplier charges both IVA and a surcharge (5.2% on 21% IVA).
  Activated via SETTINGS ``tax.recargo_equivalencia: true``.

There is **no general revenue threshold** equivalent to the German
Kleinunternehmer — ES Autónomo starts invoicing IVA from the first
EUR. ``check_threshold`` returns ``level="n/a"``.
"""
from __future__ import annotations

from typing import Any, Dict

from .protocol import ThresholdStatus, VATRegime

ISO_CODE = "ES"
CURRENCY = "EUR"
DEFAULT_LOCALE = "es-ES"
INVOICE_TEMPLATE = "INVOICE_ES.md.j2"
EXPENSES_TEMPLATE = "EXPENSES_ES.md.j2"
EARNINGS_TEMPLATE = "EARNINGS_ES.md.j2"

EXPENSE_CATEGORIES: Dict[str, str] = {
    "equipamiento":       "## 🖥 Equipamiento (amortizable)",
    "software_licencias": "## 💻 Software & Licencias",
    "formacion":          "## 📚 Formación",
    "oficina":            "## 🏠 Oficina / Home-Office",
    "telecomunicaciones": "## 📱 Telecomunicaciones",
    "transporte":         "## 🚗 Transporte",
    "suministros":        "## ☕ Suministros",
}

IVA_STANDARD = 0.21
RECARGO_STANDARD = 0.052   # surcharge on 21% IVA


def vat_regime(settings, client_meta: Dict[str, Any]) -> VATRegime:
    """ES IVA decision."""
    tax_raw = settings.raw.get("tax") or {}
    recargo = bool(tax_raw.get("recargo_equivalencia", False))

    client_country = str(client_meta.get("client_country", "ES")).upper()
    b2b = bool(client_meta.get("b2b", True))
    ust_id = str(client_meta.get("ust_id", "")).strip()

    # Inversión del sujeto pasivo — EU B2B with valid EU VAT number
    if b2b and client_country != "ES" and _is_eu_country(client_country) and ust_id:
        return VATRegime(
            apply_vat=False,
            vat_rate=0.0,
            label="Inversión del sujeto pasivo (Art. 84 LIVA)",
            reverse_charge=True,
        )

    # Non-EU client: out of scope, no IVA
    if not _is_eu_country(client_country) and b2b:
        return VATRegime(
            apply_vat=False,
            vat_rate=0.0,
            label="Operación no sujeta (cliente extracomunitario)",
            reverse_charge=False,
        )

    if recargo:
        return VATRegime(
            apply_vat=True,
            vat_rate=IVA_STANDARD,
            label=f"IVA {int(IVA_STANDARD*100)}% + Recargo de Equivalencia {RECARGO_STANDARD*100}%",
            reverse_charge=False,
            extras={"recargo_equivalencia_rate": RECARGO_STANDARD},
        )

    return VATRegime(
        apply_vat=True,
        vat_rate=IVA_STANDARD,
        label=f"IVA {int(IVA_STANDARD*100)}%",
        reverse_charge=False,
    )


def check_threshold(settings, ytd_net: float) -> ThresholdStatus:
    """ES Autónomo has no revenue-threshold equivalent."""
    return ThresholdStatus(
        level="n/a",
        exit_code=0,
        ytd_net=ytd_net,
        limit=None,
        pct_used=None,
        message=(
            "ES Autónomo: IVA aplicable desde el primer euro. "
            "No hay umbral de franquicia equivalente a Kleinunternehmer."
        ),
    )


# ISO-3166-1 alpha-2 codes of EU member states (2026)
_EU_COUNTRIES = {
    "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR",
    "DE", "GR", "HU", "IE", "IT", "LV", "LT", "LU", "MT", "NL",
    "PL", "PT", "RO", "SK", "SI", "ES", "SE",
}


def _is_eu_country(cc: str) -> bool:
    return cc.upper() in _EU_COUNTRIES
