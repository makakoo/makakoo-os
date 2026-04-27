"""Tax-regime Protocol. Every ``core.tax.<cc>`` module duck-types this."""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, Optional, Protocol, runtime_checkable


@dataclass(frozen=True)
class VATRegime:
    apply_vat: bool
    vat_rate: float              # 0.0 if no VAT applies
    label: str                   # "§ 19 UStG", "IVA 21%", "Reverse Charge …", "Monotributo"
    reverse_charge: bool = False
    extras: Dict[str, Any] = None  # country-specific extras (e.g. recargo_equivalencia)

    def ust_on(self, net: float) -> float:
        if not self.apply_vat:
            return 0.0
        return round(net * self.vat_rate, 2)


@dataclass(frozen=True)
class ThresholdStatus:
    level: str                   # "green" | "yellow" | "red" | "n/a"
    exit_code: int               # 0 / 0 / 2 / 0
    ytd_net: float
    limit: Optional[float]
    pct_used: Optional[float]
    message: str


@runtime_checkable
class TaxRegime(Protocol):
    """Every country module must expose these top-level attributes."""

    ISO_CODE: str                # "DE", "AR", "ES", "US"
    CURRENCY: str                # "EUR", "ARS", "USD"
    DEFAULT_LOCALE: str          # "de-DE", "es-AR", ...
    INVOICE_TEMPLATE: str        # "INVOICE_DE.md.j2" or "INVOICE.md.j2" (base)
    EXPENSES_TEMPLATE: str       # "EXPENSES_DE.md.j2" or "EXPENSES.md.j2" (base)
    EXPENSE_CATEGORIES: Dict[str, str]   # slug -> section header / label

    def vat_regime(self, settings, client_meta: Dict[str, Any]) -> VATRegime: ...

    def check_threshold(self, settings, ytd_net: float) -> ThresholdStatus: ...
