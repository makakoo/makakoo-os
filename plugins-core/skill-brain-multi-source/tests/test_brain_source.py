"""Tests for BrainSource adapter pattern."""

from __future__ import annotations

import os
import sys
from datetime import date
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "src"))

from brain_source import (  # noqa: E402
    BrainSource,
    LogseqSource,
    ObsidianSource,
    PlainMarkdownSource,
    build_source,
)


def test_logseq_journal_path_uses_underscores(tmp_path):
    src = LogseqSource(name="test", root=tmp_path)
    p = src.journal_path(date(2026, 4, 23))
    assert p.name == "2026_04_23.md"
    assert p.parent.name == "journals"


def test_logseq_format_adds_bullet_prefix(tmp_path):
    src = LogseqSource(name="test", root=tmp_path)
    assert src.format_journal_line("hello") == "- hello"
    # Already-prefixed lines left alone
    assert src.format_journal_line("- already prefixed") == "- already prefixed"


def test_logseq_page_path_under_pages_dir(tmp_path):
    src = LogseqSource(name="test", root=tmp_path)
    p = src.page_path("Some Entity")
    assert p.parent.name == "pages"
    assert p.name == "Some Entity.md"


def test_obsidian_journal_default_format(tmp_path):
    src = ObsidianSource(name="vault", root=tmp_path)
    p = src.journal_path(date(2026, 4, 23))
    assert p.name == "2026-04-23.md"  # dashes, not underscores
    assert p.parent == tmp_path


def test_obsidian_respects_daily_notes_config(tmp_path):
    (tmp_path / ".obsidian").mkdir()
    (tmp_path / ".obsidian" / "daily-notes.json").write_text(
        '{"format": "YYYY/MM/DD", "folder": "Daily"}'
    )
    src = ObsidianSource(name="vault", root=tmp_path)
    p = src.journal_path(date(2026, 4, 23))
    assert "2026/04/23" in str(p)
    assert "Daily" in str(p.parent)


def test_obsidian_format_is_flat(tmp_path):
    src = ObsidianSource(name="vault", root=tmp_path)
    assert src.format_journal_line("hello") == "hello"  # no prefix


def test_append_to_journal_creates_file(tmp_path):
    src = LogseqSource(name="test", root=tmp_path)
    path = src.append_to_journal("first entry")
    assert path.exists()
    assert "- first entry" in path.read_text()


def test_append_to_journal_preserves_existing(tmp_path):
    src = LogseqSource(name="test", root=tmp_path)
    src.append_to_journal("line 1")
    src.append_to_journal("line 2")
    content = src.journal_path(date.today()).read_text()
    assert "- line 1" in content
    assert "- line 2" in content


def test_read_only_refuses_write(tmp_path):
    src = ObsidianSource(name="vault", root=tmp_path, writable=False)
    with pytest.raises(PermissionError):
        src.append_to_journal("nope")


def test_iter_docs_walks_markdown(tmp_path):
    (tmp_path / "pages").mkdir()
    (tmp_path / "pages" / "Foo.md").write_text("# Foo\nbody")
    (tmp_path / "pages" / "Bar.md").write_text("no h1")
    (tmp_path / "not_markdown.txt").write_text("skip me")

    src = LogseqSource(name="test", root=tmp_path)
    docs = list(src.iter_docs())
    titles = sorted(d.title for d in docs)
    assert "Foo" in titles
    assert "Bar" in titles
    # Non-markdown skipped
    assert not any(".txt" in d.relative_path for d in docs)


def test_iter_docs_skips_hidden_dirs(tmp_path):
    (tmp_path / ".obsidian").mkdir()
    (tmp_path / ".obsidian" / "workspace.md").write_text("internal")
    (tmp_path / "real.md").write_text("# real")

    src = ObsidianSource(name="vault", root=tmp_path)
    docs = list(src.iter_docs())
    titles = [d.title for d in docs]
    assert "real" in titles
    assert "workspace" not in titles


def test_iter_docs_derives_title_from_h1(tmp_path):
    (tmp_path / "note.md").write_text("# Actual Title\nbody")
    src = ObsidianSource(name="vault", root=tmp_path)
    docs = list(src.iter_docs())
    assert docs[0].title == "Actual Title"


def test_iter_docs_falls_back_to_filename(tmp_path):
    (tmp_path / "some_note-name.md").write_text("no h1")
    src = ObsidianSource(name="vault", root=tmp_path)
    docs = list(src.iter_docs())
    assert docs[0].title == "some note name"


def test_plain_markdown_default_daily_format(tmp_path):
    src = PlainMarkdownSource(name="plain", root=tmp_path)
    p = src.journal_path(date(2026, 4, 23))
    assert p.name == "2026-04-23.md"


def test_plain_markdown_outliner_mode(tmp_path):
    src = PlainMarkdownSource(name="plain", root=tmp_path, write_style="outliner")
    assert src.format_journal_line("hi") == "- hi"


def test_build_source_factory(tmp_path):
    logseq = build_source({"name": "a", "type": "logseq", "path": str(tmp_path)})
    obsidian = build_source({"name": "b", "type": "obsidian", "path": str(tmp_path)})
    plain = build_source({"name": "c", "type": "plain", "path": str(tmp_path)})
    assert isinstance(logseq, LogseqSource)
    assert isinstance(obsidian, ObsidianSource)
    assert isinstance(plain, PlainMarkdownSource)


def test_build_source_unknown_type_raises(tmp_path):
    with pytest.raises(ValueError):
        build_source({"name": "x", "type": "notion-cloud", "path": str(tmp_path)})


def test_build_source_expands_env_vars(tmp_path, monkeypatch):
    monkeypatch.setenv("TESTVAULT", str(tmp_path))
    src = build_source({"name": "e", "type": "obsidian", "path": "$TESTVAULT/sub"})
    assert str(tmp_path) in str(src.root)
