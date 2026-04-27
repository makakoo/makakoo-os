"""Per-country tax regime dispatch.

Each country module in this package (``core.tax.<cc>``) is duck-typed
against the :class:`TaxRegime` protocol in :mod:`core.tax.protocol`.
``get_regime("AR")`` lazily imports ``core.tax.ar`` and returns its
module-level attributes as a regime.

Adding a country: drop a ``<cc>.py`` file with the required top-level
symbols, ensure ``ISO_CODE`` matches, and it's automatically
discoverable.
"""
from __future__ import annotations

from importlib import import_module
from pathlib import Path
from typing import Any

from ..errors import FreelanceError


class UnsupportedCountryError(FreelanceError):
    """The requested ISO country code has no tax module installed."""


def get_regime(country_code: str) -> Any:
    """Return the tax-regime module for the given ISO-3166 alpha-2 code.

    Raises :class:`UnsupportedCountryError` with the list of installed
    modules when no module matches.
    """
    cc = country_code.strip().lower()
    if not cc:
        raise UnsupportedCountryError("country_code is empty")
    try:
        return import_module(f"src.core.tax.{cc}")
    except ImportError:
        installed = _installed_modules()
        raise UnsupportedCountryError(
            f"no tax regime module installed for country {country_code!r}; "
            f"installed: {sorted(installed)}"
        )


def _installed_modules():
    pkg_dir = Path(__file__).resolve().parent
    names = []
    for f in pkg_dir.iterdir():
        if f.name.startswith("_") or f.suffix != ".py":
            continue
        if f.stem in ("protocol",):
            continue
        names.append(f.stem.upper())
    return names
