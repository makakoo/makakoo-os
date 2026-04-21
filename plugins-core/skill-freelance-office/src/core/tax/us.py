"""US — United States tax regime.

No federal VAT. State sales tax varies (0–10%+) and is typically the
client's responsibility for B2B services. This regime:

- ``vat_regime`` returns ``apply_vat=False`` always. The invoice
  template tags the total as "plus applicable state sales tax (client
  responsibility)".
- ``check_threshold`` returns ``level="n/a"`` — no federal income cap.
  State-level notification thresholds vary (nexus rules); consult a CPA.

Expense categories mirror IRS Schedule C section layout where
applicable, translated to plugin slugs.
"""
from __future__ import annotations

from typing import Any, Dict

from .protocol import ThresholdStatus, VATRegime

ISO_CODE = "US"
CURRENCY = "USD"
DEFAULT_LOCALE = "en-US"
# US reuses the base templates (no bespoke INVOICE_US.md.j2) per v0.2
# scope cut. The base INVOICE.md.j2 handles the no-VAT / non-KU path.
INVOICE_TEMPLATE = "INVOICE.md.j2"
EXPENSES_TEMPLATE = "EXPENSES.md.j2"
EARNINGS_TEMPLATE = "EARNINGS.md.j2"

# IRS Schedule C Part II expense categories (adapted).
EXPENSE_CATEGORIES: Dict[str, str] = {
    "equipment":       "## 🖥 Equipment (Depreciable / Sec. 179)",
    "software":        "## 💻 Software & Online Services",
    "education":       "## 📚 Education & Professional Development",
    "home_office":     "## 🏠 Home Office (Form 8829)",
    "communications":  "## 📱 Telephone & Internet (Business Portion)",
    "travel":          "## 🚗 Travel & Mileage",
    "supplies":        "## ☕ Office Supplies",
}


def vat_regime(settings, client_meta: Dict[str, Any]) -> VATRegime:
    """US invoices carry no federal VAT. State sales tax is flagged
    but not computed here — that's a per-state / per-service-type call
    the client (or their CPA) handles."""
    return VATRegime(
        apply_vat=False,
        vat_rate=0.0,
        label="No federal VAT (US). State sales tax: client responsibility.",
        reverse_charge=False,
    )


def check_threshold(settings, ytd_net: float) -> ThresholdStatus:
    return ThresholdStatus(
        level="n/a",
        exit_code=0,
        ytd_net=ytd_net,
        limit=None,
        pct_used=None,
        message=(
            "US: no federal VAT threshold. "
            "Review state sales-tax nexus rules separately; consult a CPA."
        ),
    )
