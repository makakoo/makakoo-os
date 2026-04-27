"""Tests for the plugin-update-check SANCHO tick.

Journal line format is locked by SPRINT §5.C.4 — a downstream change that
breaks the format is a bug in the tick, not a test drift. Run under any
interpreter python3.9+."""
from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "src"))

import tick  # noqa: E402


def test_render_line_manifest_drift() -> None:
    line = tick._render_line({
        "name": "agent-browser-harness",
        "current": "abc1234def56789",
        "upstream": "def56789abc1234",
        "drift": True,
        "drift_type": "manifest",
    })
    assert line is not None
    assert line.startswith("- [[Harvey]] plugin update available: ")
    assert "agent-browser-harness" in line
    assert "abc1234" in line and "def5678" in line
    assert "(manifest)" in line
    assert "makakoo plugin update agent-browser-harness" in line


def test_render_line_content_drift() -> None:
    line = tick._render_line({
        "name": "browser-plugin",
        "current": "1111111aaaaaaa",
        "upstream": "2222222bbbbbbb",
        "drift": True,
        "drift_type": "content",
    })
    assert line is not None
    assert "(content)" in line


def test_render_line_skips_uptodate() -> None:
    assert tick._render_line({
        "name": "up-to-date-plugin",
        "drift": False,
    }) is None


def test_render_line_surfaces_error() -> None:
    line = tick._render_line({
        "name": "ghost-plugin",
        "error": "network unreachable",
    })
    assert line is not None
    assert line.startswith("- [[Harvey]] plugin update check failed: ")
    assert "network unreachable" in line


def test_short_truncates_to_7() -> None:
    assert tick._short("abcdef123456789") == "abcdef1"
    assert tick._short(None) == "(new)"
    assert tick._short("") == "(new)"
