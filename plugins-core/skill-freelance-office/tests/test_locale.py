"""Locale-aware money formatter."""
from __future__ import annotations

from src.core.locale import currency_symbol, format_money


def test_format_money_de_de():
    # European comma-decimal, dot thousand
    assert format_money(1234.56, "de-DE") == "1.234,56"
    assert format_money(0, "de-DE") == "0,00"
    assert format_money(1_234_567.89, "de-DE") == "1.234.567,89"


def test_format_money_en_us():
    assert format_money(1234.56, "en-US") == "1,234.56"
    assert format_money(1_234_567.89, "en-US") == "1,234,567.89"


def test_format_money_es_ar():
    # AR uses European format (comma decimal)
    assert format_money(1234.56, "es-AR") == "1.234,56"


def test_format_money_unknown_locale_defaults_to_de_format():
    # Historical v0.1 default — fall back to DE/European format
    assert format_money(1234.56, "zz-ZZ") == "1.234,56"


def test_currency_symbol_known():
    assert currency_symbol("EUR") == "€"
    assert currency_symbol("USD") == "$"
    assert currency_symbol("ARS") == "$"
    assert currency_symbol("GBP") == "£"


def test_currency_symbol_unknown_returns_code():
    assert currency_symbol("XYZ") == "XYZ"
