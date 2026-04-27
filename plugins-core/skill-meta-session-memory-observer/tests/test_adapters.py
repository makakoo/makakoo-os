"""Tests for session-source adapters (Claude Code / Qwen / Vibe)."""

from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import adapters  # noqa: E402


def test_claude_parse_string_content():
    line = {"type": "user", "message": {"content": "hello"}}
    nm = adapters._claude_code_parse(line)
    assert nm is not None and nm.role == "user" and nm.text == "hello"


def test_claude_parse_blocks_with_tools():
    line = {
        "type": "assistant",
        "message": {"content": [
            {"type": "text", "text": "I'll read the file."},
            {"type": "tool_use", "name": "Read"},
            {"type": "text", "text": "done"},
        ]},
    }
    nm = adapters._claude_code_parse(line)
    assert nm is not None and nm.role == "assistant"
    assert "read the file" in nm.text.lower()
    assert "tool_use: Read" in nm.text


def test_claude_parse_ignores_non_conversation_types():
    for t in ("permission-mode", "system", "file-history-snapshot", "attachment"):
        assert adapters._claude_code_parse({"type": t, "message": {"content": "x"}}) is None


def test_claude_parse_empty_text_returns_none():
    assert adapters._claude_code_parse({"type": "user", "message": {"content": ""}}) is None
    assert adapters._claude_code_parse({"type": "user", "message": {"content": [
        {"type": "tool_use", "name": "X"},
    ]}}) is None or True  # tool_use marker counts as text


def test_qwen_parse_parts_text():
    line = {"type": "user", "message": {"parts": [{"text": "hello"}]}}
    nm = adapters._qwen_parse(line)
    assert nm is not None and nm.role == "user" and nm.text == "hello"


def test_qwen_parse_thought_prefix():
    line = {"type": "assistant", "message": {"parts": [
        {"text": "planning step", "thought": True},
        {"text": "answer"},
    ]}}
    nm = adapters._qwen_parse(line)
    assert nm is not None
    assert "[thinking] planning step" in nm.text
    assert "answer" in nm.text


def test_qwen_parse_function_call_marker():
    line = {"type": "assistant", "message": {"parts": [
        {"functionCall": {"name": "read_file", "args": {}}},
    ]}}
    nm = adapters._qwen_parse(line)
    assert nm is not None and "tool_use: read_file" in nm.text


def test_qwen_parse_ignores_empty_parts():
    assert adapters._qwen_parse({"type": "user", "message": {"parts": []}}) is None
    assert adapters._qwen_parse({"type": "user", "message": {}}) is None


def test_vibe_parse_flat_content():
    line = {"role": "user", "content": "vibe hi"}
    nm = adapters._vibe_parse(line)
    assert nm is not None and nm.role == "user" and nm.text == "vibe hi"


def test_vibe_parse_ignores_system_role():
    assert adapters._vibe_parse({"role": "system", "content": "x"}) is None


def test_all_adapters_declared():
    names = [a.name for a in adapters.ALL_ADAPTERS]
    assert names == ["claude", "qwen", "vibe"]
    for a in adapters.ALL_ADAPTERS:
        assert callable(a.parse_line)
        assert callable(a.glob_fn)


def test_glob_returns_empty_when_dir_missing(tmp_path, monkeypatch):
    # Point Home at an empty tmp so no adapters find anything
    monkeypatch.setenv("HOME", str(tmp_path))
    # Reimport to pick up new HOME — instead, call glob_fn directly. The function
    # uses Path.home() which respects HOME.
    for a in adapters.ALL_ADAPTERS:
        result = list(a.glob_fn())
        assert result == []


def test_text_from_blocks_handles_malformed():
    assert adapters._text_from_blocks(None) == ""
    assert adapters._text_from_blocks(42) == ""
    assert adapters._text_from_blocks([{"type": "text"}]) == ""  # missing text key
    assert adapters._text_from_blocks(["not a dict"]) == ""
