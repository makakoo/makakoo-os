"""Locale-aware money formatter.

Number format differs per locale:

    de-DE / es-AR / es-ES   1.234,56
    en-US                   1,234.56

Currency symbol is NOT included — templates handle that so the same
formatter works for invoices, dashboards, and journal lines.
"""
from __future__ import annotations


def format_money(amount: float, locale: str = "de-DE") -> str:
    loc = (locale or "de-DE").lower()
    base = f"{amount:,.2f}"   # produces 1,234.56 (python C-locale default)
    if loc.startswith(("de", "es", "it", "fr", "pt")):
        # European comma-decimal
        return base.replace(",", "X").replace(".", ",").replace("X", ".")
    if loc.startswith(("en",)):
        return base
    # Unknown locale — default to DE since that's the historical v0.1 shape.
    return base.replace(",", "X").replace(".", ",").replace("X", ".")


def currency_symbol(currency: str) -> str:
    """Best-effort symbol for rendering. Templates can override."""
    return {
        "EUR": "€",
        "USD": "$",
        "ARS": "$",
        "GBP": "£",
        "BRL": "R$",
        "CHF": "CHF",
    }.get(currency.upper(), currency.upper())
