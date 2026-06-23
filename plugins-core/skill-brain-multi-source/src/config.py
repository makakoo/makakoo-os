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
        "canonical": "default",
        "default": "default",
        "sources": [
            {
                "name": "default",
                "role": "canonical",
                "type": "logseq",
                "path": "$MAKAKOO_HOME/data/Brain",
                "writable": True,
            }
        ],
    }


@dataclass
class BrainSourceRegistry:
    """In-memory view of the config — list of sources + canonical pointer."""

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


def _normalize_data(data: dict) -> dict:
    """Return a backward-compatible canonical/enrichment registry shape.

    Older configs used `default` as a write-default pointer. The Brain safety
    contract now keeps `$MAKAKOO_HOME/data/Brain` canonical and treats every
    other source as enrichment unless it explicitly points at the same default
    Brain source. Keep the legacy `default` key as an alias so old readers don't
    crash, but write the newer `canonical` key too.
    """
    fallback = _default_config()
    raw_sources = data.get("sources") or []
    sources: list[dict] = []
    seen: set[str] = set()

    for raw in raw_sources:
        if not isinstance(raw, dict):
            continue
        entry = dict(raw)
        name = entry.get("name") or entry.get("type") or "source"
        entry["name"] = name
        if name in seen:
            continue
        seen.add(name)
        if "writable" not in entry:
            entry["writable"] = False if name != "default" else True
        if "role" not in entry:
            entry["role"] = "canonical" if name == "default" else "enrichment"
        sources.append(entry)

    if "default" not in seen:
        sources.insert(0, fallback["sources"][0])
        seen.add("default")

    # Only a source explicitly marked canonical can be canonical. If legacy
    # `default` pointed at an external vault, demote that pointer for safety.
    canonical = data.get("canonical") or data.get("default") or "default"
    if not any(s.get("name") == canonical and s.get("role") == "canonical" for s in sources):
        canonical = "default"
    for source in sources:
        if source.get("name") == canonical:
            source["role"] = "canonical"
            source["writable"] = True
        elif source.get("role") == "canonical":
            # Single canonical source only. Others become enrichment.
            source["role"] = "enrichment"

    return {"canonical": canonical, "default": canonical, "sources": sources}


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

    data = _normalize_data(data)
    entries = data["sources"]
    default_name = data["canonical"]
    sources = [build_source(e) for e in entries]
    return BrainSourceRegistry(default_name=default_name, sources=sources)


def save_registry(registry_data: dict) -> Path:
    """Write the config atomically. registry_data is the raw JSON shape, not the dataclass."""
    registry_data = _normalize_data(registry_data)
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
    data = _normalize_data(data)
    data.setdefault("sources", [])
    entry = dict(entry)
    entry.setdefault("role", "canonical" if entry.get("name") == "default" else "enrichment")
    entry.setdefault("writable", False if entry.get("role") == "enrichment" else True)
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


def apply_changes(adds: list[dict], default_name: str | None = None) -> Path:
    """Apply source upserts plus optional canonical change in one atomic write."""
    path = config_path()
    if path.exists():
        data = json.loads(path.read_text(encoding="utf-8"))
    else:
        data = _default_config()
    data = _normalize_data(data)
    data.setdefault("sources", [])

    for entry in adds:
        entry = dict(entry)
        entry.setdefault("role", "canonical" if entry.get("name") == "default" else "enrichment")
        entry.setdefault("writable", False if entry.get("role") == "enrichment" else True)
        replaced = False
        for i, existing in enumerate(data["sources"]):
            if existing.get("name") == entry.get("name"):
                data["sources"][i] = entry
                replaced = True
                break
        if not replaced:
            data["sources"].append(entry)

    if default_name:
        names = {s.get("name") for s in data.get("sources", [])}
        if default_name not in names:
            raise KeyError(f"no source named {default_name!r}")
        role = next((s.get("role") for s in data["sources"] if s.get("name") == default_name), None)
        if role != "canonical":
            raise ValueError(f"cannot make enrichment source {default_name!r} canonical")
        data["canonical"] = default_name
        data["default"] = default_name

    return save_registry(data)


def remove_source(name: str) -> Path:
    """Remove a source by name. Refuses to remove the canonical source."""
    path = config_path()
    if not path.exists():
        raise FileNotFoundError(f"no config at {path}")
    data = _normalize_data(json.loads(path.read_text(encoding="utf-8")))
    if data.get("canonical") == name:
        raise ValueError(f"cannot remove canonical source {name!r}")
    before = len(data.get("sources", []))
    data["sources"] = [s for s in data.get("sources", []) if s.get("name") != name]
    if len(data["sources"]) == before:
        raise KeyError(f"no source named {name!r}")
    return save_registry(data)


def set_default(name: str) -> Path:
    """Legacy command: change canonical source only when target is canonical-role."""
    path = config_path()
    data = _normalize_data(json.loads(path.read_text(encoding="utf-8")) if path.exists() else _default_config())
    names = {s.get("name") for s in data.get("sources", [])}
    if name not in names:
        raise KeyError(f"no source named {name!r}")
    role = next((s.get("role") for s in data["sources"] if s.get("name") == name), None)
    if role != "canonical":
        raise ValueError(f"cannot make enrichment source {name!r} canonical")
    data["canonical"] = name
    data["default"] = name
    return save_registry(data)
