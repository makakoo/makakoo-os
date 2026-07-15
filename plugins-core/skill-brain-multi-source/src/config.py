"""Brain-sources config loader.

Reads `$MAKAKOO_HOME/config/brain_sources.json`. If the file is absent,
returns a sane default (one Logseq source at `$MAKAKOO_HOME/data/Brain`) so
first-run of any consumer module never crashes waiting for config.
"""

from __future__ import annotations

import json
import os
import stat
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

from brain_source import (
    SUPPORTED_SOURCE_TYPES,
    BrainSource,
    build_source,
    resolve_source_root,
)


CONFIG_FILENAME = "brain_sources.json"
LOCK_FILENAME = "brain_sources.lock"
TEMP_FILENAME = ".brain_sources.json.tmp"
BACKUP_FILENAME = ".brain_sources.json.backup"
OWNER_FILENAME = ".brain_sources.json.owner"
RECOVERY_MARKER_PREFIX = (
    "makakoo-brain-sources-recovery-v1\ntarget=brain_sources.json\ncontent:\n"
)


def _makakoo_home() -> Path:
    home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME")
    if home:
        return Path(home).expanduser()
    return Path.home() / "MAKAKOO"


def config_path() -> Path:
    return _makakoo_home() / "config" / CONFIG_FILENAME


def _lock_path() -> Path:
    return config_path().parent / LOCK_FILENAME


def _temporary_path() -> Path:
    return config_path().parent / TEMP_FILENAME


def _backup_path() -> Path:
    return config_path().parent / BACKUP_FILENAME


def _owner_path() -> Path:
    return config_path().parent / OWNER_FILENAME


def _entry_exists(path: Path) -> bool:
    return os.path.lexists(path)


def _sync_directory(directory: Path) -> None:
    """Persist directory-entry changes on platforms that expose directory fds."""
    if os.name == "nt":
        return
    descriptor = os.open(directory, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _registry_marker_expected_body() -> str | None:
    marker = _owner_path()
    if not _entry_exists(marker):
        return None
    if not stat.S_ISREG(marker.lstat().st_mode):
        return None
    raw = marker.read_text(encoding="utf-8")
    if not raw.startswith(RECOVERY_MARKER_PREFIX):
        return None
    body = raw[len(RECOVERY_MARKER_PREFIX):]
    try:
        json.loads(body)
    except json.JSONDecodeError:
        return None
    return body


def _registry_marker_is_owned() -> bool:
    return _registry_marker_expected_body() is not None


def _require_owned_registry_artifacts() -> None:
    if not _registry_marker_is_owned():
        raise RuntimeError(
            f"refusing unowned brain source recovery artifacts in {config_path().parent}; "
            "move them aside or remove them manually"
        )
    for artifact in (_temporary_path(), _backup_path()):
        if _entry_exists(artifact) and not stat.S_ISREG(artifact.lstat().st_mode):
            raise RuntimeError(
                f"refusing non-file brain source recovery artifact {artifact}; "
                "move it aside or remove it manually"
            )


def _write_registry_marker(body: str) -> None:
    marker = _owner_path()
    if _entry_exists(marker):
        raise RuntimeError(
            f"brain source recovery marker collision at {marker}; "
            "move it aside or remove it manually"
        )
    with marker.open("x", encoding="utf-8") as handle:
        handle.write(RECOVERY_MARKER_PREFIX)
        handle.write(body)
        handle.flush()
        os.fsync(handle.fileno())
    _sync_directory(marker.parent)


def _remove_registry_marker() -> None:
    marker = _owner_path()
    if not _entry_exists(marker):
        return
    if not _registry_marker_is_owned():
        raise RuntimeError(
            f"refusing unowned brain source recovery marker {marker}; "
            "move it aside or remove it manually"
        )
    marker.unlink()
    _sync_directory(marker.parent)


def _acquire_file_lock(handle) -> None:
    """Acquire the same advisory lock used by Rust's ``fs2::FileExt``."""
    if os.name == "nt":
        import msvcrt

        handle.seek(0)
        if handle.read(1) == b"":
            handle.seek(0)
            handle.write(b"\0")
            handle.flush()
        handle.seek(0)
        msvcrt.locking(handle.fileno(), msvcrt.LK_LOCK, 1)
        return

    import fcntl

    fcntl.flock(handle.fileno(), fcntl.LOCK_EX)


def _release_file_lock(handle) -> None:
    if os.name == "nt":
        import msvcrt

        handle.seek(0)
        msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
        return

    import fcntl

    fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


@contextmanager
def _registry_lock() -> Iterator[None]:
    """Serialize Python and native registry access through one lock file."""
    path = _lock_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+b") as handle:
        _acquire_file_lock(handle)
        try:
            yield
        finally:
            _release_file_lock(handle)


def _recover_registry_files_unlocked() -> None:
    """Recover the fixed temp/backup names shared with the Rust CLI."""
    path = config_path()
    backup = _backup_path()
    temporary = _temporary_path()
    marker = _owner_path()
    backup_exists = _entry_exists(backup)
    temporary_exists = _entry_exists(temporary)
    if backup_exists or temporary_exists:
        _require_owned_registry_artifacts()
    elif _entry_exists(marker):
        expected = _registry_marker_expected_body()
        if expected is None:
            raise RuntimeError("brain source recovery marker has no intended config body")
        if not _entry_exists(path):
            with path.open("x", encoding="utf-8") as handle:
                handle.write(expected)
                handle.flush()
                os.fsync(handle.fileno())
            _sync_directory(path.parent)
        else:
            if not stat.S_ISREG(path.lstat().st_mode):
                raise RuntimeError(
                    f"refusing marker-only brain source recovery because primary is not "
                    f"a regular file: {path}"
                )
            if path.read_text(encoding="utf-8") != expected:
                # Marker became durable before temp creation. The primary was
                # never moved, so keep it and abort replay.
                _remove_registry_marker()
                return
        _remove_registry_marker()
        return

    changed = False
    temporary_consumed = False
    if not _entry_exists(path) and not backup_exists and temporary_exists:
        expected = _registry_marker_expected_body()
        if expected is None:
            raise RuntimeError("brain source recovery marker has no intended config body")
        try:
            temporary_matches = temporary.read_text(encoding="utf-8") == expected
        except (OSError, UnicodeError):
            temporary_matches = False
        if temporary_matches:
            with temporary.open("rb") as handle:
                os.fsync(handle.fileno())
            temporary.replace(path)
        else:
            temporary.unlink()
            _sync_directory(path.parent)
            with path.open("x", encoding="utf-8") as handle:
                handle.write(expected)
                handle.flush()
                os.fsync(handle.fileno())
        temporary_consumed = True
        changed = True
    elif not _entry_exists(path) and backup_exists:
        backup.replace(path)
        changed = True
    elif _entry_exists(path) and backup_exists:
        if not stat.S_ISREG(path.lstat().st_mode):
            raise RuntimeError(
                f"refusing to discard brain source backup because primary is not a regular file: {path}"
            )
        expected = _registry_marker_expected_body()
        if expected is None or path.read_text(encoding="utf-8") != expected:
            raise RuntimeError(
                "refusing to discard brain source backup because primary does not match "
                "the owned transaction"
            )
        # A previous promotion may have crashed before its directory fsync.
        # Persist the primary entry before discarding the fallback copy.
        _sync_directory(path.parent)
        backup.unlink()
        changed = True
    if temporary_exists and not temporary_consumed:
        temporary.unlink()
        changed = True
    if changed:
        _sync_directory(path.parent)
    _remove_registry_marker()


def _read_registry_data_unlocked(*, fallback_on_corrupt: bool = False) -> dict:
    _recover_registry_files_unlocked()
    path = config_path()
    if not _entry_exists(path):
        return _default_config()
    if not stat.S_ISREG(path.lstat().st_mode):
        raise RuntimeError(f"brain source config is not a regular file: {path}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        if fallback_on_corrupt:
            return _default_config()
        raise


def _write_registry_unlocked(registry_data: dict) -> Path:
    """Crash-recoverable registry replacement. Caller must hold the lock."""
    registry_data = _normalize_data(registry_data)
    path = config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = _temporary_path()
    backup = _backup_path()
    marker = _owner_path()
    if _entry_exists(temporary) or _entry_exists(backup) or _entry_exists(marker):
        raise RuntimeError("brain source recovery artifact appeared during registry update")
    if _entry_exists(path) and not stat.S_ISREG(path.lstat().st_mode):
        raise RuntimeError(f"refusing to replace non-file brain source config: {path}")
    body = json.dumps(registry_data, indent=2) + "\n"
    _write_registry_marker(body)
    try:
        handle = temporary.open("x", encoding="utf-8")
    except BaseException:
        _remove_registry_marker()
        raise
    with handle:
        handle.write(body)
        handle.flush()
        os.fsync(handle.fileno())

    if not _entry_exists(path):
        temporary.replace(path)
        _sync_directory(path.parent)
        _remove_registry_marker()
        return path

    if _entry_exists(backup):
        raise RuntimeError(f"brain source backup appeared during registry update: {backup}")
    path.replace(backup)
    _sync_directory(path.parent)
    try:
        temporary.replace(path)
    except BaseException:
        if not _entry_exists(path) and _entry_exists(backup):
            backup.replace(path)
            _sync_directory(path.parent)
            if _entry_exists(temporary):
                temporary.unlink()
                _sync_directory(path.parent)
            _remove_registry_marker()
        raise
    _sync_directory(path.parent)
    backup.unlink()
    _sync_directory(path.parent)
    _remove_registry_marker()
    return path


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


def _resolved_source_root(entry: dict) -> Path:
    return resolve_source_root(entry.get("path", ""), _makakoo_home())


def _validate_source_overlaps(sources: list[dict]) -> None:
    roots = [(source.get("name", "source"), _resolved_source_root(source)) for source in sources]
    for index, (name, root) in enumerate(roots):
        for other_name, other_root in roots[index + 1:]:
            if root == other_root or root in other_root.parents or other_root in root.parents:
                raise ValueError(
                    f"brain source roots overlap: {name!r} ({root}) and "
                    f"{other_name!r} ({other_root})"
                )


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
        if name in seen:
            continue
        seen.add(name)
        if name == "default":
            entry = dict(fallback["sources"][0])
        else:
            entry["name"] = name
            entry["role"] = "enrichment"
            source_type = str(entry.get("type") or "plain").strip().lower()
            if source_type not in SUPPORTED_SOURCE_TYPES:
                raise ValueError(f"unknown brain source type: {source_type!r}")
            entry["type"] = source_type
            entry["path"] = str(_resolved_source_root(entry))
            entry.setdefault("writable", False)
            if source_type == "okf":
                entry["writable"] = False
        sources.append(entry)

    if "default" not in seen:
        sources.insert(0, fallback["sources"][0])
        seen.add("default")

    _validate_source_overlaps(sources)
    return {"canonical": "default", "default": "default", "sources": sources}


def load_registry() -> BrainSourceRegistry:
    """Read config (or default) and materialize every source as a BrainSource."""
    with _registry_lock():
        data = _read_registry_data_unlocked(fallback_on_corrupt=True)

    data = _normalize_data(data)
    entries = data["sources"]
    default_name = data["canonical"]
    sources = [build_source(e) for e in entries]
    return BrainSourceRegistry(default_name=default_name, sources=sources)


def save_registry(registry_data: dict) -> Path:
    """Write the config under the cross-process registry lock."""
    with _registry_lock():
        _recover_registry_files_unlocked()
        return _write_registry_unlocked(_normalize_data(registry_data))


def _reject_canonical_override(entry: dict) -> None:
    if entry.get("name") == "default":
        raise ValueError(
            "canonical source 'default' is fixed at $MAKAKOO_HOME/data/Brain "
            "and cannot be replaced"
        )


def add_source(entry: dict) -> Path:
    """Add or update a source by name. Writes config atomically."""
    _reject_canonical_override(entry)
    with _registry_lock():
        data = _normalize_data(_read_registry_data_unlocked())
        data.setdefault("sources", [])
        entry = dict(entry)
        entry["role"] = "enrichment"
        entry.setdefault("writable", False)
        # Replace if name exists, else append
        replaced = False
        for i, existing in enumerate(data["sources"]):
            if existing.get("name") == entry.get("name"):
                data["sources"][i] = entry
                replaced = True
                break
        if not replaced:
            data["sources"].append(entry)
        return _write_registry_unlocked(_normalize_data(data))


def apply_changes(adds: list[dict], default_name: str | None = None) -> Path:
    """Apply source upserts plus optional canonical change in one atomic write."""
    with _registry_lock():
        data = _normalize_data(_read_registry_data_unlocked())
        data.setdefault("sources", [])

        for entry in adds:
            entry = dict(entry)
            _reject_canonical_override(entry)
            entry["role"] = "enrichment"
            entry.setdefault("writable", False)
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

        return _write_registry_unlocked(_normalize_data(data))


def remove_source(name: str) -> Path:
    """Remove a source by name. Refuses to remove the canonical source."""
    with _registry_lock():
        _recover_registry_files_unlocked()
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
        return _write_registry_unlocked(data)


def set_default(name: str) -> Path:
    """Legacy command: change canonical source only when target is canonical-role."""
    with _registry_lock():
        data = _normalize_data(_read_registry_data_unlocked())
        names = {s.get("name") for s in data.get("sources", [])}
        if name not in names:
            raise KeyError(f"no source named {name!r}")
        role = next((s.get("role") for s in data["sources"] if s.get("name") == name), None)
        if role != "canonical":
            raise ValueError(f"cannot make enrichment source {name!r} canonical")
        data["canonical"] = name
        data["default"] = name
        return _write_registry_unlocked(data)
