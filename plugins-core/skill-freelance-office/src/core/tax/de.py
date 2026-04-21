"""DE — Germany tax regime.

Reference implementation. **MUST match v0.1 behavior byte-for-byte** —
this module is a refactor, not a behavior change.

- Kleinunternehmer (§19 UStG): YTD ≤ €22.000 / year (net). If active,
  no VAT on any invoice.
- Reverse Charge for EU B2B with client USt-IdNr. (Art. 196 MwSt-SystRL).
- Default regime: +19% USt.
"""
from __future__ import annotations

from typing import Any, Dict

from .protocol import ThresholdStatus, VATRegime

ISO_CODE = "DE"
CURRENCY = "EUR"
DEFAULT_LOCALE = "de-DE"
INVOICE_TEMPLATE = "INVOICE.md.j2"          # v0.1 DE template — base INVOICE.md.j2
EXPENSES_TEMPLATE = "EXPENSES.md.j2"        # v0.1 DE template — base EXPENSES.md.j2
EARNINGS_TEMPLATE = "EARNINGS.md.j2"

EXPENSE_CATEGORIES: Dict[str, str] = {
    "equipment":     "## 🖥 Equipment (AfA — Abschreibung über 3 Jahre)",
    "software":      "## 💻 Software & Lizenzen",
    "fortbildung":   "## 📚 Fortbildung",
    "homeoffice":    "## 🏠 Homeoffice-Pauschale",
    "telefon":       "## 📱 Telefon & Internet (anteilig, geschäftlich)",
    "fahrt":         "## 🚗 Fahrtkosten",
    "arbeitsmittel": "## ☕ Arbeitsmittel (einmalig < 800 €)",
}

KLEINUNTERNEHMER_LIMIT = 22_000.00
KLEINUNTERNEHMER_WARN_PCT = 80.0


def vat_regime(settings, client_meta: Dict[str, Any]) -> VATRegime:
    """DE VAT decision tree. Mirrors v0.1's ``_vat_regime`` exactly."""
    ku = bool(settings.tax.kleinunternehmer)
    client_country = str(client_meta.get("client_country", "DE")).upper()
    b2b = bool(client_meta.get("b2b", True))
    ust_id = str(client_meta.get("ust_id", "")).strip()

    if ku:
        return VATRegime(
            apply_vat=False,
            vat_rate=0.0,
            label="§ 19 UStG (Kleinunternehmer)",
            reverse_charge=False,
        )

    if b2b and client_country and client_country != "DE" and bool(ust_id):
        return VATRegime(
            apply_vat=False,
            vat_rate=0.0,
            label="Reverse Charge (Art. 196 MwSt-SystRL)",
            reverse_charge=True,
        )

    return VATRegime(
        apply_vat=True,
        vat_rate=0.19,
        label="USt 19%",
        reverse_charge=False,
    )


def check_threshold(settings, ytd_net: float) -> ThresholdStatus:
    """§19 UStG YTD check. ≥ 100% exits 2; ≥ 80% → yellow."""
    if not settings.tax.kleinunternehmer:
        return ThresholdStatus(
            level="n/a",
            exit_code=0,
            ytd_net=ytd_net,
            limit=None,
            pct_used=None,
            message="Regular VAT regime; no Kleinunternehmer limit applies.",
        )
    pct = round(ytd_net / KLEINUNTERNEHMER_LIMIT * 100, 2)
    if ytd_net >= KLEINUNTERNEHMER_LIMIT:
        return ThresholdStatus(
            level="red",
            exit_code=2,
            ytd_net=ytd_net,
            limit=KLEINUNTERNEHMER_LIMIT,
            pct_used=pct,
            message=(
                f"Limit exceeded. €{ytd_net}/€{int(KLEINUNTERNEHMER_LIMIT)} ({pct}%). "
                "USt-IdNr. + USt-Voranmeldung required for every new invoice."
            ),
        )
    if pct >= KLEINUNTERNEHMER_WARN_PCT:
        return ThresholdStatus(
            level="yellow",
            exit_code=0,
            ytd_net=ytd_net,
            limit=KLEINUNTERNEHMER_LIMIT,
            pct_used=pct,
            message=(
                f"WARN approaching §19 UStG limit: €{ytd_net}/€{int(KLEINUNTERNEHMER_LIMIT)} "
                f"({pct}%). Consider switching to regular VAT."
            ),
        )
    return ThresholdStatus(
        level="green",
        exit_code=0,
        ytd_net=ytd_net,
        limit=KLEINUNTERNEHMER_LIMIT,
        pct_used=pct,
        message=(
            f"Kleinunternehmer status: active. YTD: €{ytd_net}/€{int(KLEINUNTERNEHMER_LIMIT)}. "
            f"{pct}% used."
        ),
    )
