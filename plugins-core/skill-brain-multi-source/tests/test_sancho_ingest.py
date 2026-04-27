"""Tests for the SANCHO multi-source ingest task."""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "src"))

import config as cfg  # noqa: E402
import sancho_ingest  # noqa: E402


@pytest.fixture
def tmp_home(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.delenv("HARVEY_HOME", raising=False)
    (tmp_path / "data" / "Brain" / "pages").mkdir(parents=True)
    # Seed a valid markdown doc so the default Logseq source has something to walk
    (tmp_path / "data" / "Brain" / "pages" / "seed.md").write_text("# seed")
    return tmp_path


def test_tick_reports_status_ok(tmp_home, capsys):
    rc = sancho_ingest.tick()
    out = capsys.readouterr().out
    assert rc == 0
    payload = json.loads(out)
    assert payload["status"] in ("ok", "partial")
    assert "per_source" in payload


def test_tick_reports_new_docs_on_first_run(tmp_home, capsys):
    rc = sancho_ingest.tick()
    out = capsys.readouterr().out
    payload = json.loads(out)
    assert payload["per_source"].get("default", 0) >= 1


def test_tick_is_idempotent_when_no_changes(tmp_home, capsys):
    sancho_ingest.tick()
    capsys.readouterr()  # drain
    # Second tick should see 0 new docs
    sancho_ingest.tick()
    out = capsys.readouterr().out
    payload = json.loads(out)
    assert payload["per_source"].get("default", 0) == 0


def test_tick_picks_up_new_file(tmp_home, capsys):
    sancho_ingest.tick()
    capsys.readouterr()

    # Add a new file AFTER first tick, with a future mtime
    new_doc = tmp_home / "data" / "Brain" / "pages" / "fresh.md"
    new_doc.write_text("# fresh")
    future = time.time() + 60
    os.utime(new_doc, (future, future))

    sancho_ingest.tick()
    out = capsys.readouterr().out
    payload = json.loads(out)
    assert payload["per_source"].get("default", 0) >= 1


def test_tick_cursor_persists_across_runs(tmp_home, capsys):
    sancho_ingest.tick()
    capsys.readouterr()
    cursor_file = sancho_ingest._cursor_path()
    assert cursor_file.exists()
    cursors = json.loads(cursor_file.read_text())
    assert "default" in cursors


def test_tick_with_multiple_sources(tmp_home, capsys):
    # Register an extra Obsidian-style source
    extra = tmp_home / "my_vault"
    extra.mkdir()
    (extra / "note1.md").write_text("# note1")
    (extra / "note2.md").write_text("# note2")

    cfg.add_source({
        "name": "personal",
        "type": "obsidian",
        "path": str(extra),
        "writable": False,
    })

    sancho_ingest.tick()
    out = capsys.readouterr().out
    payload = json.loads(out)
    assert "default" in payload["per_source"]
    assert "personal" in payload["per_source"]
    assert payload["per_source"]["personal"] == 2
