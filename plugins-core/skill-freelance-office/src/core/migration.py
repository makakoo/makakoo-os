"""v0.1 → v0.2 migration. Runs at every command entry. Idempotent.

Two jobs:

1. **Seed the registry** if no ``freelance_offices.json`` exists yet.
   The single office points at ``$FREELANCE_OFFICE_HOME`` (or the v0.1
   default ``~/freelance-office``), id=``default``, country=``DE``.

2. **Inject the `office:` block** into SETTINGS.yaml for every
   registered office whose SETTINGS.yaml doesn't carry it. Exactly one
   new block per file, at the top of the tax-adjacent metadata.

Both steps are race-safe via the registry's sidecar lock.
"""
from __future__ import annotations

import fcntl
import os
from pathlib import Path
from typing import Iterable, Optional

from . import registry as reg_mod


OFFICE_BLOCK_TEMPLATE = (
    "\noffice:\n"
    '  id: "{id}"\n'
    '  country: "{country}"\n'
    '  locale: "{locale}"\n'
    '  currency: "{currency}"\n'
    '  invoice_language: "{invoice_language}"\n'
)

DEFAULT_COUNTRY_PROFILE = {
    "DE": {"locale": "de-DE", "currency": "EUR", "invoice_language": "de"},
    "AR": {"locale": "es-AR", "currency": "ARS", "invoice_language": "es"},
    "ES": {"locale": "es-ES", "currency": "EUR", "invoice_language": "es"},
    "US": {"locale": "en-US", "currency": "USD", "invoice_language": "en"},
}


def ensure_v02(env_home: Optional[Path] = None) -> dict:
    """Idempotent v0.1 → v0.2 migration. Returns a dict summary of
    what, if anything, was migrated. Caller may log it."""
    registry_path = reg_mod.OfficeRegistry.default_location()
    summary = {"seeded_registry": False, "injected_blocks": [], "noop": True}

    _ensure_registry(registry_path, env_home, summary)
    _ensure_office_blocks(registry_path, summary)

    summary["noop"] = (
        not summary["seeded_registry"] and not summary["injected_blocks"]
    )
    return summary


def _ensure_registry(registry_path: Path, env_home: Optional[Path], summary: dict) -> None:
    """Seed the registry atomically if missing. Race-safe: if two
    processes race on first-run, the second one finds the registry
    already present on re-read inside the lock and no-ops."""
    if registry_path.is_file():
        return

    lock_path = registry_path.with_suffix(registry_path.suffix + ".lock")
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    lock_fd = os.open(str(lock_path), os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        # Re-check inside the lock — another process may have seeded it
        # while we were waiting on the flock.
        if registry_path.is_file():
            return
        home = env_home or _v01_home()
        home = home.expanduser().resolve()
        registry = reg_mod.OfficeRegistry.load(registry_path)  # empty
        registry.offices = {}
        registry.default = None
        # Bypass .add() which takes the lock again (would be fine —
        # same process holds it — but avoid the re-lock dance).
        from datetime import date
        registry.offices["default"] = reg_mod.OfficeEntry(
            id="default",
            path=home,
            country="DE",
            added=str(date.today()),
        )
        registry.default = "default"
        registry._save_locked()  # noqa: SLF001
        summary["seeded_registry"] = True
        summary["seed_path"] = str(home)
    finally:
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_UN)
        finally:
            os.close(lock_fd)


def _ensure_office_blocks(registry_path: Path, summary: dict) -> None:
    """For every registered office, inject ``office:`` into its
    SETTINGS.yaml if absent. Keeps the rest of the file byte-identical."""
    registry = reg_mod.OfficeRegistry.load(registry_path)
    for entry in registry.offices.values():
        settings_file = entry.path / "_meta" / "SETTINGS.yaml"
        if not settings_file.is_file():
            continue
        text = settings_file.read_text(encoding="utf-8")
        if "\noffice:\n" in text or text.startswith("office:\n"):
            continue
        profile = DEFAULT_COUNTRY_PROFILE.get(
            entry.country, DEFAULT_COUNTRY_PROFILE["DE"]
        )
        block = OFFICE_BLOCK_TEMPLATE.format(
            id=entry.id, country=entry.country, **profile
        )
        # Insert at very top so yaml parsers see office: first.
        new_text = block.lstrip("\n") + "\n" + text
        settings_file.write_text(new_text, encoding="utf-8")
        summary["injected_blocks"].append(str(settings_file))


def _v01_home() -> Path:
    env = os.environ.get("FREELANCE_OFFICE_HOME")
    return Path(env) if env else Path.home() / "freelance-office"
