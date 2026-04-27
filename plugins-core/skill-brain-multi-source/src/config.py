"""Brain-sources config loader.

Reads `$MAKAKOO_HOME/config/brain_sources.json`. If the file is absent,
returns a sane default (one Logseq source at `$MAKAKOO_HOME/data/Brain`) so
first-run of any consumer module never crashes waiting for config.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

from brain_source import BrainSource, build_source


CONFIG_FILENAME = "brain_sources.json"


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home).expanduser()
    return Path.home() / "MAKAKOO"


def config_path() -> Path:
    return _makakoo_home() / "config" / CONFIG_FILENAME


def _default_config() -> dict:
    return {
        "default": "default",
        "sources": [
            {
                "name": "default",
                "type": "logseq",
                "path": "$MAKAKOO_HOME/data/Brain",
                "writable": True,
            }
        ],
    }


@dataclass
class BrainSourceRegistry:
    """In-memory view of the config — list of sources + default pointer."""

    default_name: str
    sources: list[BrainSource]

    def get(self, name: str) -> BrainSource:
        for s in self.sources:
            if s.name == name:
                return s
        raise KeyError(f"no brain source named {name!r}")

    def get_default(self) -> BrainSource:
        return self.get(self.default_name)

    def names(self) -> list[str]:
        return [s.name for s in self.sources]


def load_registry() -> BrainSourceRegistry:
    """Read config (or default) and materialize every source as a BrainSource."""
    path = config_path()
    if path.exists():
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            data = _default_config()
    else:
        data = _default_config()

    entries = data.get("sources") or _default_config()["sources"]
    default_name = data.get("default") or entries[0]["name"]
    sources = [build_source(e) for e in entries]
    return BrainSourceRegistry(default_name=default_name, sources=sources)


def save_registry(registry_data: dict) -> Path:
    """Write the config atomically. registry_data is the raw JSON shape, not the dataclass."""
    path = config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(registry_data, indent=2), encoding="utf-8")
    tmp.replace(path)
    return path


def add_source(entry: dict) -> Path:
    """Add or update a source by name. Writes config atomically."""
    path = config_path()
    if path.exists():
        data = json.loads(path.read_text(encoding="utf-8"))
    else:
        data = _default_config()
    data.setdefault("sources", [])
    # Replace if name exists, else append
    replaced = False
    for i, existing in enumerate(data["sources"]):
        if existing.get("name") == entry.get("name"):
            data["sources"][i] = entry
            replaced = True
            break
    if not replaced:
        data["sources"].append(entry)
    return save_registry(data)


def remove_source(name: str) -> Path:
    """Remove a source by name. Refuses to remove the default source."""
    path = config_path()
    if not path.exists():
        raise FileNotFoundError(f"no config at {path}")
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("default") == name:
        raise ValueError(f"cannot remove default source {name!r}; set default to another source first")
    before = len(data.get("sources", []))
    data["sources"] = [s for s in data.get("sources", []) if s.get("name") != name]
    if len(data["sources"]) == before:
        raise KeyError(f"no source named {name!r}")
    return save_registry(data)


def set_default(name: str) -> Path:
    """Change which source is the write default."""
    path = config_path()
    data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else _default_config()
    names = {s.get("name") for s in data.get("sources", [])}
    if name not in names:
        raise KeyError(f"no source named {name!r}")
    data["default"] = name
    return save_registry(data)
