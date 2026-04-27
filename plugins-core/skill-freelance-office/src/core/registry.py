"""Office registry — JSON store of every registered freelance-office
directory + which one is the default.

Lives at ``$MAKAKOO_HOME/config/freelance_offices.json`` (or
``$FREELANCE_OFFICES_REGISTRY`` for test staging). Sidecar-locked
per LD#9 — lock is on ``.lock``, never on the data fd.
"""
from __future__ import annotations

import fcntl
import json
import os
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Dict, Iterator, List, Optional

from .errors import FreelanceError

SCHEMA_VERSION = 1


class RegistryError(FreelanceError):
    """Any failure reading, writing, or mutating the office registry."""


class UnknownOfficeError(RegistryError):
    """The requested office id is not registered."""


class DuplicateOfficeError(RegistryError):
    """An office with this id already exists."""


@dataclass(frozen=True)
class OfficeEntry:
    id: str
    path: Path
    country: str
    added: str                      # ISO date YYYY-MM-DD

    def to_json(self) -> dict:
        return {
            "path": str(self.path),
            "country": self.country,
            "added": self.added,
        }


@dataclass
class OfficeRegistry:
    path: Path
    default: Optional[str] = None
    offices: Dict[str, OfficeEntry] = field(default_factory=dict)

    # ── locations ────────────────────────────────────────────────────

    @staticmethod
    def default_location() -> Path:
        env = os.environ.get("FREELANCE_OFFICES_REGISTRY")
        if env:
            return Path(env)
        makakoo_home = Path(os.environ.get("MAKAKOO_HOME", Path.home() / "MAKAKOO"))
        return makakoo_home / "config" / "freelance_offices.json"

    @property
    def lock_path(self) -> Path:
        return self.path.with_suffix(self.path.suffix + ".lock")

    # ── IO ───────────────────────────────────────────────────────────

    @classmethod
    def load(cls, path: Optional[Path] = None) -> "OfficeRegistry":
        p = path or cls.default_location()
        if not p.is_file():
            return cls(path=p)
        try:
            data = json.loads(p.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError) as e:
            raise RegistryError(f"failed to read registry {p}: {e}") from e
        offices_raw = data.get("offices", {}) or {}
        offices = {
            oid: OfficeEntry(
                id=oid,
                path=Path(rec["path"]).expanduser(),
                country=str(rec.get("country", "DE")),
                added=str(rec.get("added", "")),
            )
            for oid, rec in offices_raw.items()
        }
        return cls(path=p, default=data.get("default"), offices=offices)

    def _save_locked(self) -> None:
        """Write the registry. Caller holds the lock."""
        self.path.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "schema_version": SCHEMA_VERSION,
            "default": self.default,
            "offices": {oid: e.to_json() for oid, e in self.offices.items()},
        }
        tmp = self.path.with_suffix(self.path.suffix + ".tmp")
        with tmp.open("w", encoding="utf-8") as f:
            json.dump(payload, f, indent=2, ensure_ascii=False)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, self.path)

    # ── locking ──────────────────────────────────────────────────────

    def _lock_fd(self):
        self.lock_path.parent.mkdir(parents=True, exist_ok=True)
        return os.open(str(self.lock_path), os.O_CREAT | os.O_RDWR, 0o644)

    # ── CRUD ─────────────────────────────────────────────────────────

    def add(self, id: str, path: Path, country: str, default: bool = False) -> OfficeEntry:
        lock_fd = self._lock_fd()
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX)
            # Re-load under lock to catch concurrent writes.
            reg = OfficeRegistry.load(self.path)
            if id in reg.offices:
                raise DuplicateOfficeError(
                    f"office {id!r} already registered at {reg.offices[id].path}"
                )
            entry = OfficeEntry(
                id=id,
                path=Path(path).expanduser().resolve(),
                country=country.upper(),
                added=str(date.today()),
            )
            reg.offices[id] = entry
            if default or reg.default is None:
                reg.default = id
            reg._save_locked()
            # Reflect into self so callers see the mutation.
            self.offices = reg.offices
            self.default = reg.default
            return entry
        finally:
            try:
                fcntl.flock(lock_fd, fcntl.LOCK_UN)
            finally:
                os.close(lock_fd)

    def remove(self, id: str) -> None:
        lock_fd = self._lock_fd()
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX)
            reg = OfficeRegistry.load(self.path)
            if id not in reg.offices:
                raise UnknownOfficeError(self._unknown_msg(reg, id))
            del reg.offices[id]
            if reg.default == id:
                reg.default = next(iter(reg.offices), None)
            reg._save_locked()
            self.offices = reg.offices
            self.default = reg.default
        finally:
            try:
                fcntl.flock(lock_fd, fcntl.LOCK_UN)
            finally:
                os.close(lock_fd)

    def set_default(self, id: str) -> None:
        lock_fd = self._lock_fd()
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX)
            reg = OfficeRegistry.load(self.path)
            if id not in reg.offices:
                raise UnknownOfficeError(self._unknown_msg(reg, id))
            reg.default = id
            reg._save_locked()
            self.offices = reg.offices
            self.default = reg.default
        finally:
            try:
                fcntl.flock(lock_fd, fcntl.LOCK_UN)
            finally:
                os.close(lock_fd)

    # ── query ────────────────────────────────────────────────────────

    def get(self, id: str) -> OfficeEntry:
        if id not in self.offices:
            raise UnknownOfficeError(self._unknown_msg(self, id))
        return self.offices[id]

    def __iter__(self) -> Iterator[OfficeEntry]:
        return iter(self.offices.values())

    def __len__(self) -> int:
        return len(self.offices)

    def list_ids(self) -> List[str]:
        return list(self.offices.keys())

    # ── error message ────────────────────────────────────────────────

    @staticmethod
    def _unknown_msg(reg: "OfficeRegistry", id: str) -> str:
        if not reg.offices:
            return f"unknown office {id!r}; no offices registered yet"
        summary = ", ".join(
            f"{e.id} ({e.country}) at {e.path}" for e in reg.offices.values()
        )
        return f"unknown office {id!r}; registered: [{summary}]"
