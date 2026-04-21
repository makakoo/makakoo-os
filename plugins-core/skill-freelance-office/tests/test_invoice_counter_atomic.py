"""Invoice counter: race + disk-seed + counter-file integrity."""
from __future__ import annotations

import multiprocessing as mp
import os
import tempfile
from pathlib import Path

import pytest

from src.core import invoice_counter


def _worker(q, year, root_str):
    inv, n, seeded = invoice_counter.allocate(year, Path(root_str))
    q.put((inv, n, seeded))


@pytest.fixture
def clean_fs_root(tmp_path):
    (tmp_path / "finances" / "2026").mkdir(parents=True)
    (tmp_path / "clients").mkdir()
    return tmp_path


def test_ten_concurrent_allocations_are_distinct(clean_fs_root):
    ctx = mp.get_context("fork")
    q = ctx.Queue()
    procs = [ctx.Process(target=_worker, args=(q, 2026, str(clean_fs_root))) for _ in range(10)]
    for p in procs:
        p.start()
    for p in procs:
        p.join()
    nums = sorted(q.get()[1] for _ in range(10))
    assert nums == list(range(1, 11))


def test_disk_seed_when_counter_missing_but_invoice_on_disk(clean_fs_root):
    inv_dir = clean_fs_root / "clients" / "x" / "projects" / "y" / "invoices"
    inv_dir.mkdir(parents=True)
    (inv_dir / "INV-2026-005.md").touch()
    (inv_dir / "INV-2026-012.md").touch()

    inv, n, seeded = invoice_counter.allocate(2026, clean_fs_root)
    assert n == 13
    assert seeded is True
    assert inv == "INV-2026-013"


def test_second_allocation_does_not_reseed(clean_fs_root):
    inv_dir = clean_fs_root / "clients" / "x" / "projects" / "y" / "invoices"
    inv_dir.mkdir(parents=True)
    (inv_dir / "INV-2026-005.md").touch()

    invoice_counter.allocate(2026, clean_fs_root)
    _, n2, seeded2 = invoice_counter.allocate(2026, clean_fs_root)
    assert n2 == 7
    assert seeded2 is False


def test_corrupted_counter_raises(clean_fs_root):
    p = invoice_counter.counter_data_path(2026, clean_fs_root)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("not json")
    from src.core.errors import FreelanceError
    with pytest.raises(FreelanceError):
        invoice_counter.allocate(2026, clean_fs_root)


def test_counter_year_mismatch_raises(clean_fs_root):
    p = invoice_counter.counter_data_path(2026, clean_fs_root)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text('{"schema_version":1,"year":2025,"last_number":7}')
    from src.core.errors import FreelanceError
    with pytest.raises(FreelanceError):
        invoice_counter.allocate(2026, clean_fs_root)


def test_peek_reports_current_without_bump(clean_fs_root):
    assert invoice_counter.peek(2026, clean_fs_root) == 0
    invoice_counter.allocate(2026, clean_fs_root)
    assert invoice_counter.peek(2026, clean_fs_root) == 1
    # peek again unchanged
    assert invoice_counter.peek(2026, clean_fs_root) == 1
