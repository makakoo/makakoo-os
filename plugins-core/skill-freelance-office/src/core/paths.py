"""Path resolution for freelance-office.

Two layers:

1. **Legacy single-home resolvers** (``freelance_home()``, ``meta_dir()``,
   ``finances_dir()``, …) read ``$FREELANCE_OFFICE_HOME`` or fall back
   to ``~/freelance-office``. They remain for backwards compatibility
   with v0.1 call sites that aren't office-aware yet.

2. **Office-aware resolvers** (``resolve_office_root(args)``,
   ``meta_dir_for(root)``, ``finances_dir_for(root, year)``, …) take an
   explicit ``root`` and belong to v0.2+. Every command that accepts
   ``--office`` should resolve once at entry and pass the resulting
   path as ``root=`` to every core helper call.
"""
from __future__ import annotations

import os
from pathlib import Path
from typing import Any, Optional

from . import registry as reg_mod
from .errors import NotInitialisedError


# ── legacy (single-home) ─────────────────────────────────────────────


def freelance_home() -> Path:
    """Root of the hand-maintained filesystem. Overridable via
    ``$FREELANCE_OFFICE_HOME`` so tests can stage a tmp dir."""
    env = os.environ.get("FREELANCE_OFFICE_HOME")
    return Path(env).expanduser() if env else Path.home() / "freelance-office"


def meta_dir() -> Path:
    return freelance_home() / "_meta"


def clients_dir() -> Path:
    return freelance_home() / "clients"


def templates_dir() -> Path:
    return freelance_home() / "templates"


def finances_dir(year: int) -> Path:
    return freelance_home() / "finances" / str(year)


def settings_path() -> Path:
    return meta_dir() / "SETTINGS.yaml"


def rates_path() -> Path:
    return meta_dir() / "RATES.yaml"


def require_initialised() -> None:
    """Raise :class:`NotInitialisedError` if the root is missing or
    SETTINGS.yaml isn't present."""
    if not freelance_home().is_dir():
        raise NotInitialisedError(
            f"{freelance_home()} does not exist — run `freelance-office init` first"
        )
    if not settings_path().is_file():
        raise NotInitialisedError(
            f"{settings_path()} missing — run `freelance-office init` first"
        )


# ── office-aware (v0.2+) ─────────────────────────────────────────────


def resolve_office_root(args: Optional[Any] = None) -> Path:
    """Resolve which office the caller is targeting.

    Priority:

    1. Explicit ``args.office`` (the ``--office <id>`` CLI flag).
    2. Env ``$HARVEY_FREELANCE_OFFICE`` (per-shell override).
    3. Registry default (``registry.default``).
    4. Sole registered office — if exactly one is registered, use it.
    5. Registry empty AND ``$FREELANCE_OFFICE_HOME`` set — use that path
       directly (pre-migration fallback; once migration has run, the
       registry will hold this same home under id ``default``).
    6. Hard fallback: ``~/freelance-office`` (v0.1 implicit home).

    When the registry holds >1 office and no default + no explicit
    selector, raise :class:`NotInitialisedError` with a message naming
    every candidate by ``id + country + path``.
    """
    # 1. explicit
    explicit = getattr(args, "office", None) if args is not None else None

    # Load registry (may not exist yet before migration seeds it)
    registry = reg_mod.OfficeRegistry.load()
    if explicit:
        if explicit not in registry.offices:
            raise reg_mod.UnknownOfficeError(
                reg_mod.OfficeRegistry._unknown_msg(registry, explicit)  # noqa: SLF001
            )
        return registry.offices[explicit].path

    # 2. env
    env = os.environ.get("HARVEY_FREELANCE_OFFICE")
    if env:
        if env in registry.offices:
            return registry.offices[env].path
        # If env is a path (not an id), accept it directly.
        p = Path(env).expanduser()
        if p.exists():
            return p
        # otherwise fall through — registry default may still work

    # 3. registry default
    if registry.default and registry.default in registry.offices:
        return registry.offices[registry.default].path

    # 4. sole registered
    if len(registry.offices) == 1:
        return next(iter(registry.offices.values())).path

    # 5. env FREELANCE_OFFICE_HOME pre-migration
    if not registry.offices:
        env_home = os.environ.get("FREELANCE_OFFICE_HOME")
        if env_home:
            return Path(env_home).expanduser()
        # 6. v0.1 implicit
        return Path.home() / "freelance-office"

    # >1 office, no default, no explicit, no env → hard error
    summary = ", ".join(
        f"{e.id} ({e.country}) at {e.path}" for e in registry.offices.values()
    )
    raise NotInitialisedError(
        f"multiple offices registered and no default set; pass --office <id> "
        f"or run `makakoo skill freelance-office office use <id>`. "
        f"Registered: [{summary}]"
    )


def meta_dir_for(root: Path) -> Path:
    return Path(root) / "_meta"


def clients_dir_for(root: Path) -> Path:
    return Path(root) / "clients"


def templates_dir_for(root: Path) -> Path:
    return Path(root) / "templates"


def finances_dir_for(root: Path, year: int) -> Path:
    return Path(root) / "finances" / str(year)


def settings_path_for(root: Path) -> Path:
    return meta_dir_for(root) / "SETTINGS.yaml"


def rates_path_for(root: Path) -> Path:
    return meta_dir_for(root) / "RATES.yaml"


def require_initialised_at(root: Path) -> None:
    """Same as :func:`require_initialised` but targets an explicit office root."""
    if not Path(root).is_dir():
        raise NotInitialisedError(
            f"{root} does not exist — run `freelance-office init --office <id>` first"
        )
    sp = settings_path_for(root)
    if not sp.is_file():
        raise NotInitialisedError(
            f"{sp} missing — run `freelance-office init --office <id>` first"
        )
