"""AR — Argentina tax regime.

Two regimes:

- **Monotributo** (simplified regime for small taxpayers).
  Categories A–K with increasing ARS annual thresholds. Within
  Monotributo: no IVA billed on invoices, threshold watch is category-
  specific. Exceeding the top of the current category → warn at 80%,
  red at 100%.
- **Responsable Inscripto** (general VAT regime).
  IVA 21% on every service invoice; no income-threshold concept.

Pick the regime via SETTINGS ``tax.ar_regime``:

.. code-block:: yaml

    tax:
      ar_regime: "monotributo"    # or "responsable_inscripto"
      ar_monotributo_category: "B"

The Monotributo category table is pinned to the 2026 AFIP figures
(source: AFIP resolution, pinned at module load). For precise
decisions a human should cross-check the AFIP site — this is a
live-document regime that updates each year.
"""
from __future__ import annotations

from typing import Any, Dict

from .protocol import ThresholdStatus, VATRegime

ISO_CODE = "AR"
CURRENCY = "ARS"
DEFAULT_LOCALE = "es-AR"
INVOICE_TEMPLATE = "INVOICE_AR.md.j2"
EXPENSES_TEMPLATE = "EXPENSES_AR.md.j2"
EARNINGS_TEMPLATE = "EARNINGS_AR.md.j2"

EXPENSE_CATEGORIES: Dict[str, str] = {
    "bienes_de_uso":         "## 🖥 Bienes de Uso (Amortización)",
    "servicios_profesionales": "## 💻 Servicios Profesionales & Software",
    "capacitacion":          "## 📚 Capacitación",
    "oficina":               "## 🏠 Oficina / Home-Office",
    "comunicaciones":        "## 📱 Comunicaciones (teléfono / internet)",
    "transporte":            "## 🚗 Transporte",
    "otros":                 "## ☕ Otros Gastos Deducibles",
}

# ARS annual thresholds by Monotributo category — pinned to 2026 AFIP
# figures for SERVICIOS. Review annually — AFIP updates the scale each
# year. Source: resolución-general vigente al 2026-04 (Sebastian — check
# at onboarding, these drift ~40-60% per year with inflation).
MONOTRIBUTO_CATEGORIES: Dict[str, float] = {
    "A": 7_813_063.45,
    "B": 11_447_046.44,
    "C": 16_050_091.57,
    "D": 19_553_419.58,
    "E": 22_953_650.62,
    "F": 28_671_898.13,
    "G": 34_390_143.61,
    "H": 53_298_417.30,
    "I": 59_657_887.55,
    "J": 68_318_880.36,
    "K": 82_370_281.28,
}

WARN_PCT = 80.0


def vat_regime(settings, client_meta: Dict[str, Any]) -> VATRegime:
    """AR VAT decision."""
    regime = str(getattr(settings.tax, "ar_regime", "monotributo") or "monotributo").lower()
    # The raw settings map may have ar_regime one level up
    if regime == "monotributo" and "ar_regime" in (settings.raw.get("tax") or {}):
        regime = str((settings.raw.get("tax") or {}).get("ar_regime", "monotributo")).lower()

    if regime == "monotributo":
        return VATRegime(
            apply_vat=False,
            vat_rate=0.0,
            label="Monotributo (sin IVA discriminado)",
            reverse_charge=False,
        )
    return VATRegime(
        apply_vat=True,
        vat_rate=0.21,
        label="IVA 21% (Responsable Inscripto)",
        reverse_charge=False,
    )


def check_threshold(settings, ytd_net: float) -> ThresholdStatus:
    """AR threshold check — Monotributo only (RI has no cap)."""
    tax_raw = settings.raw.get("tax") or {}
    regime = str(tax_raw.get("ar_regime", "monotributo")).lower()
    if regime != "monotributo":
        return ThresholdStatus(
            level="n/a",
            exit_code=0,
            ytd_net=ytd_net,
            limit=None,
            pct_used=None,
            message="Responsable Inscripto; no threshold — IVA 21% applies to every invoice.",
        )

    category = str(tax_raw.get("ar_monotributo_category", "B")).upper()
    if category not in MONOTRIBUTO_CATEGORIES:
        return ThresholdStatus(
            level="n/a",
            exit_code=0,
            ytd_net=ytd_net,
            limit=None,
            pct_used=None,
            message=(
                f"Monotributo active, but category {category!r} is not in the "
                f"pinned table {sorted(MONOTRIBUTO_CATEGORIES)} — update "
                "SETTINGS.tax.ar_monotributo_category."
            ),
        )
    limit = MONOTRIBUTO_CATEGORIES[category]
    pct = round(ytd_net / limit * 100, 2)
    if ytd_net >= limit:
        next_cat = _next_category(category)
        next_msg = (
            f" Move to category {next_cat} (limit ARS {MONOTRIBUTO_CATEGORIES[next_cat]:,.0f})."
            if next_cat else
            " No higher Monotributo category — consider switching to Responsable Inscripto."
        )
        return ThresholdStatus(
            level="red",
            exit_code=2,
            ytd_net=ytd_net,
            limit=limit,
            pct_used=pct,
            message=(
                f"Monotributo category {category} limit exceeded. "
                f"YTD ARS {ytd_net:,.2f} / ARS {limit:,.2f} ({pct}%)." + next_msg
            ),
        )
    if pct >= WARN_PCT:
        return ThresholdStatus(
            level="yellow",
            exit_code=0,
            ytd_net=ytd_net,
            limit=limit,
            pct_used=pct,
            message=(
                f"WARN approaching Monotributo category {category} ceiling: "
                f"ARS {ytd_net:,.2f} / ARS {limit:,.2f} ({pct}%)."
            ),
        )
    return ThresholdStatus(
        level="green",
        exit_code=0,
        ytd_net=ytd_net,
        limit=limit,
        pct_used=pct,
        message=(
            f"Monotributo category {category}: ARS {ytd_net:,.2f} / "
            f"ARS {limit:,.2f} ({pct}%)."
        ),
    )


def _next_category(cat: str) -> str:
    keys = list(MONOTRIBUTO_CATEGORIES.keys())
    try:
        i = keys.index(cat)
        return keys[i + 1] if i + 1 < len(keys) else ""
    except ValueError:
        return ""
