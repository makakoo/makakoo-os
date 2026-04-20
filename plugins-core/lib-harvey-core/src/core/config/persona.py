"""Persona config loader.

The platform is Makakoo OS. The AI persona is user-configurable via a single
``persona.json`` file at ``$MAKAKOO_HOME/config/persona.json``. This decouples
"what the platform is called" from "what the user named their AI".

Sebastian's install ships with ``name="Harvey"`` so every existing prompt,
journal reference, wikilink, and skill file keeps working unchanged. Other
users can set any name they want without forking the codebase.

Precedence for locating the config file:

    1. $MAKAKOO_HOME/config/persona.json   (canonical, 2026+)
    2. $HARVEY_HOME/config/persona.json    (legacy alias, still honored)
    3. ~/.makakoo/config/persona.json      (out-of-tree install)
    4. ~/.harvey/config/persona.json       (legacy)

If nothing is found, the loader returns a default Persona with
``name="Harvey"`` — the same value Sebastian's install expects, so fresh
dev checkouts "just work" without a config file.

Python 3.9 compatible — no PEP 604 unions, no `match` statements.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional

_CACHE: "Optional[Persona]" = None

_DEFAULT_NAME = "Harvey"
_DEFAULT_USER = "Sebastian"
_DEFAULT_PRONOUNS = "he/him"
_SCHEMA_VERSION = 1


@dataclass
class Persona:
    name: str = _DEFAULT_NAME
    user: str = _DEFAULT_USER
    home: str = ""
    pronouns: str = _DEFAULT_PRONOUNS
    version: int = _SCHEMA_VERSION

    def greeting(self) -> str:
        """Used by prompt templates that want a self-referential opener."""
        return "You are {name}, {user}'s autonomous cognitive extension.".format(
            name=self.name, user=self.user
        )


def _candidate_paths() -> "List[Path]":
    """Return every place a persona.json could live, in precedence order."""
    paths: "List[Path]" = []
    makakoo = os.environ.get("MAKAKOO_HOME")
    harvey = os.environ.get("HARVEY_HOME")
    if makakoo:
        paths.append(Path(os.path.expanduser(makakoo)) / "config" / "persona.json")
    if harvey and harvey != makakoo:
        paths.append(Path(os.path.expanduser(harvey)) / "config" / "persona.json")
    paths.append(Path.home() / ".makakoo" / "config" / "persona.json")
    paths.append(Path.home() / ".harvey" / "config" / "persona.json")
    return paths


def _resolved_home() -> str:
    """Best-guess platform home for the default Persona, without crashing."""
    for env_var in ("MAKAKOO_HOME", "HARVEY_HOME"):
        val = os.environ.get(env_var)
        if val:
            return os.path.abspath(os.path.expanduser(val))
    return os.path.abspath(os.path.expanduser("~/MAKAKOO"))


def _default_persona() -> Persona:
    return Persona(
        name=_DEFAULT_NAME,
        user=_DEFAULT_USER,
        home=_resolved_home(),
        pronouns=_DEFAULT_PRONOUNS,
        version=_SCHEMA_VERSION,
    )


def _from_dict(data: dict) -> Persona:
    """Build a Persona from a parsed JSON dict, ignoring unknown keys."""
    allowed = {"name", "user", "home", "pronouns", "version"}
    filtered = {k: v for k, v in data.items() if k in allowed}
    merged = {
        "name": _DEFAULT_NAME,
        "user": _DEFAULT_USER,
        "home": _resolved_home(),
        "pronouns": _DEFAULT_PRONOUNS,
        "version": _SCHEMA_VERSION,
    }
    merged.update(filtered)
    return Persona(**merged)  # type: ignore[arg-type]


def load() -> Persona:
    """Return the cached Persona, loading it from disk on first call.

    Never raises: JSON errors, missing files, and permission errors all
    silently fall back to the default Persona. Call ``reload()`` to bust
    the cache and pick up edits to persona.json at runtime.
    """
    global _CACHE
    if _CACHE is not None:
        return _CACHE

    for path in _candidate_paths():
        try:
            if path.is_file():
                raw = path.read_text(encoding="utf-8")
                data = json.loads(raw)
                if isinstance(data, dict):
                    _CACHE = _from_dict(data)
                    return _CACHE
        except (OSError, json.JSONDecodeError):
            continue

    _CACHE = _default_persona()
    return _CACHE


def reload() -> Persona:
    """Bust the cache and re-read persona.json from disk."""
    global _CACHE
    _CACHE = None
    return load()
