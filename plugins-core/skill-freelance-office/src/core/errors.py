"""Exception hierarchy for freelance-office."""
from __future__ import annotations


class FreelanceError(Exception):
    """Base — any freelance-office failure callers should catch."""


class NotInitialisedError(FreelanceError):
    """~/freelance-office/ is missing or SETTINGS.yaml doesn't exist."""


class DuplicateClientError(FreelanceError):
    """clients/<slug>/ already exists."""


class DuplicateInvoiceError(FreelanceError):
    """An INV-YYYY-NNN file with this number is already on disk."""


class KleinunternehmerExceededError(FreelanceError):
    """§19 UStG YTD limit (22.000 EUR) was reached."""


class RateFloorWarning(FreelanceError):
    """Negotiated rate is below RATES.yaml floor (caller decides to block or warn)."""
