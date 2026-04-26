"""Tests for the Brain journal attribution helper."""

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path

import pytest

from plugins_core.agent_harveychat.python.brain_sync import (
    PREFIX_TEMPLATE,
    append_journal_line,
    journal_path,
    prefix_line,
    render_logseq_bullet,
)


def test_prefix_line_adds_tag():
    assert prefix_line("secretary", "drafted email").startswith("[agent:secretary]")


def test_prefix_line_is_idempotent():
    line = prefix_line("secretary", "x")
    again = prefix_line("secretary", line)
    # Counting "[agent:" occurrences must stay at 1.
    assert again.count("[agent:") == 1


def test_journal_path_uses_yyyy_mm_dd():
    home = Path("/m")
    fixed = datetime(2026, 4, 26, 12, 0, 0, tzinfo=timezone.utc)
    p = journal_path(home, fixed)
    assert str(p) == "/m/data/Brain/journals/2026_04_26.md"


def test_render_logseq_bullet_format():
    out = render_logseq_bullet("secretary", "drafted invoice")
    assert out.startswith("- ")
    assert PREFIX_TEMPLATE.format(slot_id="secretary") in out
    assert out.endswith("\n")


def test_append_journal_line_creates_dir_and_writes(tmp_path):
    fixed = datetime(2026, 4, 26, 12, 0, 0, tzinfo=timezone.utc)
    p = append_journal_line(tmp_path, "secretary", "drafted invoice", day=fixed)
    assert p.exists()
    body = p.read_text(encoding="utf-8")
    assert body.startswith("- [agent:secretary] drafted invoice")
    assert body.endswith("\n")


def test_append_journal_line_appends_not_overwrites(tmp_path):
    fixed = datetime(2026, 4, 26, 12, 0, 0, tzinfo=timezone.utc)
    append_journal_line(tmp_path, "secretary", "first", day=fixed)
    append_journal_line(tmp_path, "secretary", "second", day=fixed)
    body = (tmp_path / "data/Brain/journals/2026_04_26.md").read_text(encoding="utf-8")
    assert body.count("[agent:secretary]") == 2
    assert "first" in body and "second" in body
