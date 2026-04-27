"""Atomic INV-YYYY-NNN allocator for ~/freelance-office/.

**Sidecar-lock protocol (LD#9 — never lock the data fd).**

For year ``Y``, state lives at
``finances/<Y>/_invoice_counter.json`` with shape
``{"schema_version": 1, "year": <Y>, "last_number": <int>}``.

Allocation takes an ``fcntl.LOCK_EX`` on the sibling
``_invoice_counter.json.lock`` file (created on demand). The data
file is only opened AFTER the lock is held, read + bumped + fsync'd,
then the lock is released. A corrupted data file never locks the
whole plugin.

**Disk-seed (pi G5).** If the data file is missing on the first
allocation of a year, we scan ``clients/*/projects/*/invoices/``
for existing ``INV-<Y>-NNN.md`` files, seed ``last_number`` to the
highest NNN found, then proceed with the bump. This protects against
running the plugin for the first time in a year that already has
manual invoices on disk.
"""
from __future__ import annotations

import fcntl
import json
import os
import re
from pathlib import Path
from typing import Optional, Tuple

from . import paths
from .errors import FreelanceError

SCHEMA_VERSION = 1
_INV_FILENAME_RE = re.compile(r"^INV-(\d{4})-(\d{3,})\.md$")


def counter_data_path(year: int, root: Optional[Path] = None) -> Path:
    base = paths.finances_dir(year) if root is None else Path(root) / "finances" / str(year)
    return base / "_invoice_counter.json"


def counter_lock_path(year: int, root: Optional[Path] = None) -> Path:
    return counter_data_path(year, root).with_suffix(".json.lock")


def _highest_on_disk(year: int, root: Optional[Path] = None) -> int:
    """Scan clients/*/projects/*/invoices/ for INV-<year>-NNN.md files.
    Returns the highest NNN seen, or 0 if nothing matches."""
    base = (root if root is not None else paths.freelance_home()) / "clients"
    highest = 0
    if not base.is_dir():
        return 0
    for inv_dir in base.glob("*/projects/*/invoices"):
        if not inv_dir.is_dir():
            continue
        for f in inv_dir.iterdir():
            m = _INV_FILENAME_RE.match(f.name)
            if m and int(m.group(1)) == year:
                n = int(m.group(2))
                if n > highest:
                    highest = n
    return highest


def _read_or_seed(year: int, data_path: Path, root: Optional[Path]) -> Tuple[int, bool]:
    """Read counter file, or seed from disk if missing/empty.
    Returns (last_number, seeded_from_disk)."""
    if data_path.is_file():
        try:
            data = json.loads(data_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as e:
            raise FreelanceError(f"invoice counter corrupt at {data_path}: {e}") from e
        if data.get("year") != year:
            raise FreelanceError(
                f"invoice counter year mismatch: file has {data.get('year')}, requested {year}"
            )
        return int(data.get("last_number", 0)), False

    seed = _highest_on_disk(year, root)
    return seed, True


def _write_atomic(data_path: Path, payload: dict) -> None:
    data_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = data_path.with_suffix(data_path.suffix + ".tmp")
    with tmp.open("w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, data_path)


def allocate(year: int, root: Optional[Path] = None) -> Tuple[str, int, bool]:
    """Allocate the next INV-<year>-NNN number atomically.

    Returns ``(inv_no_str, numeric_n, seeded_from_disk)``.
    """
    data_path = counter_data_path(year, root)
    lock_path = counter_lock_path(year, root)
    data_path.parent.mkdir(parents=True, exist_ok=True)
    # Touch the sidecar lock file; flock needs a real fd.
    lock_fd = os.open(str(lock_path), os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        last, seeded = _read_or_seed(year, data_path, root)
        next_n = last + 1
        payload = {"schema_version": SCHEMA_VERSION, "year": year, "last_number": next_n}
        _write_atomic(data_path, payload)
        return f"INV-{year}-{next_n:03d}", next_n, seeded
    finally:
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_UN)
        finally:
            os.close(lock_fd)


def peek(year: int, root: Optional[Path] = None) -> int:
    """Read current ``last_number`` without incrementing. Seeds from
    disk if the counter file is missing (read-only seed — no write)."""
    data_path = counter_data_path(year, root)
    if data_path.is_file():
        try:
            data = json.loads(data_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            return 0
        return int(data.get("last_number", 0))
    return _highest_on_disk(year, root)


def repair(year: int, root: Optional[Path] = None) -> Tuple[int, int]:
    """Rewrite counter to match ``max(NNN)`` on disk. Returns
    ``(old_value, new_value)``. Takes the same sidecar lock."""
    data_path = counter_data_path(year, root)
    lock_path = counter_lock_path(year, root)
    data_path.parent.mkdir(parents=True, exist_ok=True)
    lock_fd = os.open(str(lock_path), os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        old = peek(year, root)
        new = _highest_on_disk(year, root)
        if new < old:
            new = old
        payload = {"schema_version": SCHEMA_VERSION, "year": year, "last_number": new}
        _write_atomic(data_path, payload)
        return old, new
    finally:
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_UN)
        finally:
            os.close(lock_fd)
