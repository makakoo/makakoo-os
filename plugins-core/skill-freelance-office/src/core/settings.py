"""Typed access to ~/freelance-office/_meta/{SETTINGS,RATES}.yaml.

Both files are pure YAML. Cached per process — each subcommand run
gets one load, so tests can staging fresh fixtures by constructing
new processes.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Optional

import yaml

from . import paths
from .errors import FreelanceError


@dataclass(frozen=True)
class Office:
    id: str = "default"
    country: str = "DE"
    locale: str = "de-DE"
    currency: str = "EUR"
    invoice_language: str = "de"


@dataclass(frozen=True)
class Identity:
    name: str = ""
    dba: str = ""
    website: str = ""
    email: str = ""
    github: str = ""
    location: str = ""


@dataclass(frozen=True)
class Tax:
    kleinunternehmer: bool = False
    tax_number: str = ""
    ust_id: str = ""
    vat_reversed: bool = False


@dataclass(frozen=True)
class Bank:
    name: str = ""
    iban: str = ""
    bic: str = ""


@dataclass(frozen=True)
class Finance:
    bank: Bank = field(default_factory=Bank)
    invoice_currency: str = "EUR"
    payment_terms_days: int = 30
    invoice_language: str = "de"


@dataclass(frozen=True)
class Business:
    freelance_since: str = ""
    tax_consultant: str = ""
    insurance: str = ""
    bookeeping_tool: str = ""
    invoicing_tool: str = ""


@dataclass(frozen=True)
class Settings:
    identity: Identity
    tax: Tax
    finance: Finance
    business: Business
    office: Office = field(default_factory=Office)
    raw: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class Rates:
    day_rates: Dict[str, int] = field(default_factory=dict)
    hourly_rates: Dict[str, int] = field(default_factory=dict)
    billing: Dict[str, Any] = field(default_factory=dict)
    version: str = ""
    raw: Dict[str, Any] = field(default_factory=dict)

    @property
    def floor_day_rate(self) -> Optional[int]:
        """Lowest rate in ``day_rates``. None if the block is empty."""
        if not self.day_rates:
            return None
        return min(int(v) for v in self.day_rates.values())


def _load_yaml(path: Path) -> Dict[str, Any]:
    if not path.is_file():
        raise FreelanceError(f"required YAML file missing: {path}")
    try:
        with path.open("r", encoding="utf-8") as f:
            return yaml.safe_load(f) or {}
    except yaml.YAMLError as e:
        raise FreelanceError(f"failed to parse {path}: {e}") from e


def load_settings(path: Optional[Path] = None) -> Settings:
    raw = _load_yaml(path or paths.settings_path())
    ident = raw.get("identity", {}) or {}
    tax = raw.get("tax", {}) or {}
    fin = raw.get("finance", {}) or {}
    biz = raw.get("business", {}) or {}
    bank = fin.get("bank", {}) or {}
    off = raw.get("office", {}) or {}
    return Settings(
        office=Office(
            id=str(off.get("id", "default")),
            country=str(off.get("country", "DE")).upper(),
            locale=str(off.get("locale", "de-DE")),
            currency=str(off.get("currency", "EUR")),
            invoice_language=str(off.get("invoice_language", fin.get("invoice_language", "de"))),
        ),
        identity=Identity(
            name=ident.get("name", ""),
            dba=ident.get("dba", ""),
            website=ident.get("website", ""),
            email=ident.get("email", ""),
            github=ident.get("github", ""),
            location=ident.get("location", ""),
        ),
        tax=Tax(
            kleinunternehmer=bool(tax.get("kleinunternehmer", False)),
            tax_number=tax.get("tax_number", ""),
            ust_id=tax.get("ust_id", ""),
            vat_reversed=bool(tax.get("vat_reversed", False)),
        ),
        finance=Finance(
            bank=Bank(
                name=bank.get("name", ""),
                iban=bank.get("iban", ""),
                bic=bank.get("bic", ""),
            ),
            invoice_currency=fin.get("invoice_currency", "EUR"),
            payment_terms_days=int(fin.get("payment_terms_days", 30)),
            invoice_language=fin.get("invoice_language", "de"),
        ),
        business=Business(
            freelance_since=str(biz.get("freelance_since", "")),
            tax_consultant=biz.get("tax_consultant", ""),
            insurance=biz.get("insurance", ""),
            bookeeping_tool=biz.get("bookeeping_tool", ""),
            invoicing_tool=biz.get("invoicing_tool", ""),
        ),
        raw=raw,
    )


def load_settings_at(root: Path) -> Settings:
    """Office-aware: load SETTINGS.yaml from the explicit office root."""
    return load_settings(paths.settings_path_for(root))


def load_rates_at(root: Path) -> "Rates":
    """Office-aware: load RATES.yaml from the explicit office root."""
    return load_rates(paths.rates_path_for(root))


def load_rates(path: Optional[Path] = None) -> Rates:
    raw = _load_yaml(path or paths.rates_path())
    return Rates(
        day_rates={k: int(v) for k, v in (raw.get("day_rates") or {}).items()},
        hourly_rates={k: int(v) for k, v in (raw.get("hourly_rates") or {}).items()},
        billing=raw.get("billing") or {},
        version=str(raw.get("version", "")),
        raw=raw,
    )
