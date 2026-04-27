"""Tests for the session memory observer.

Covers cursor tracking, transcript normalization, active-session skip, and
the MIN_NEW_MESSAGES gate. Extractor invocation is mocked — we only want
to confirm the observer calls it with the right shape.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from pathlib import Path
from unittest.mock import MagicMock

import pytest

# Make observer.py importable without going through the plugin entrypoint
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import observer  # noqa: E402


@pytest.fixture
def tmp_home(tmp_path, monkeypatch):
    """Create an isolated MAKAKOO_HOME so state doesn't leak between tests."""
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    (tmp_path / "state").mkdir()
    return tmp_path


@pytest.fixture
def session_file(tmp_path):
    """Build a fake Claude Code JSONL session."""
    path = tmp_path / "abc12345-session.jsonl"
    lines = [
        {"type": "user", "message": {"role": "user", "content": "hey, save my preference: I hate tabs"}},
        {"type": "assistant", "message": {"role": "assistant", "content": "noted"}},
        {"type": "user", "message": {"role": "user", "content": "and remember I'm based in Spain"}},
        {"type": "assistant", "message": {"role": "assistant", "content": "got it"}},
        {"type": "user", "message": {"role": "user", "content": "one more: rate floor is €60/h"}},
        {"type": "assistant", "message": {"role": "assistant", "content": "logged"}},
        {"type": "user", "message": {"role": "user", "content": "and confirm I prefer contract over employment"}},
        {"type": "assistant", "message": {"role": "assistant", "content": "noted"}},
    ]
    path.write_text("\n".join(json.dumps(l) for l in lines))
    os.utime(path, (time.time() - 3600, time.time() - 3600))
    return path


def test_extract_text_content_string():
    msg = {"message": {"content": "plain text"}}
    assert observer._extract_text_content(msg) == "plain text"


def test_extract_text_content_blocks():
    msg = {"message": {"content": [
        {"type": "text", "text": "hello"},
        {"type": "tool_use", "name": "Read"},
        {"type": "text", "text": "world"},
    ]}}
    out = observer._extract_text_content(msg)
    assert "hello" in out and "world" in out
    assert "tool_use: Read" in out


def test_build_transcript_counts_user_turns():
    messages = [
        {"type": "user", "message": {"role": "user", "content": "a"}},
        {"type": "assistant", "message": {"role": "assistant", "content": "b"}},
        {"type": "user", "message": {"role": "user", "content": "c"}},
        {"type": "system", "message": {"content": "ignored"}},
    ]
    text, turns = observer._build_transcript(messages)
    assert turns == 2
    assert "USER" in text and "ASSISTANT" in text


def test_read_new_messages_from_cursor(session_file):
    all_msgs, end_offset = observer._read_new_messages(session_file, 0)
    assert len(all_msgs) == 8

    # Reading again from end_offset should give nothing
    nothing, end2 = observer._read_new_messages(session_file, end_offset)
    assert nothing == []
    assert end2 == end_offset


def test_read_new_messages_truncation_resets(session_file):
    # Pretend the file shrank under us
    oversized_cursor = session_file.stat().st_size + 1000
    msgs, new = observer._read_new_messages(session_file, oversized_cursor)
    assert len(msgs) == 8
    assert new == session_file.stat().st_size


def test_active_session_skip(tmp_path):
    recent = tmp_path / "recent.jsonl"
    recent.write_text('{"type":"user","message":{"content":"x"}}\n')
    # mtime = now → active
    assert observer._session_is_active(recent) is True

    # Age it past cooldown
    old_time = time.time() - (observer.ACTIVE_SESSION_COOLDOWN_S + 60)
    os.utime(recent, (old_time, old_time))
    assert observer._session_is_active(recent) is False


def test_process_session_skips_thin_delta(tmp_path, monkeypatch):
    path = tmp_path / "thin.jsonl"
    # Only 2 user turns, below MIN_NEW_MESSAGES=4
    lines = [
        {"type": "user", "message": {"role": "user", "content": "one"}},
        {"type": "user", "message": {"role": "user", "content": "two"}},
    ]
    path.write_text("\n".join(json.dumps(l) for l in lines))
    os.utime(path, (time.time() - 3600, time.time() - 3600))

    fake_extract = MagicMock()
    drafts = tmp_path / "drafts"
    drafts.mkdir()

    new_cursor, drafts_written = observer.process_session(path, 0, fake_extract, drafts)
    assert drafts_written == 0
    fake_extract.assert_not_called()
    # Cursor still advances so we don't re-scan next tick
    assert new_cursor == path.stat().st_size


def test_process_session_invokes_extractor(session_file, tmp_path):
    fake_extract = MagicMock()
    drafts = tmp_path / "drafts"
    drafts.mkdir()

    new_cursor, drafts_written = observer.process_session(
        session_file, 0, fake_extract, drafts
    )
    assert drafts_written == 1
    fake_extract.assert_called_once()
    args, _kwargs = fake_extract.call_args
    transcript_arg, out_dir_arg, label_arg = args
    assert "USER" in transcript_arg
    assert out_dir_arg == drafts
    assert label_arg.startswith("2")  # YYYY... timestamp


def test_cursor_roundtrip(tmp_home):
    cursors = {"session_a": 100, "session_b": 250}
    observer._save_cursors(cursors)
    loaded = observer._load_cursors()
    assert loaded == cursors


def test_cursor_load_missing_returns_empty(tmp_home):
    assert observer._load_cursors() == {}


def test_cursor_corrupt_returns_empty(tmp_home):
    state = observer._state_dir()
    (state / observer.CURSOR_FILENAME).write_text("{not json")
    assert observer._load_cursors() == {}


def test_process_session_extractor_failure_keeps_cursor(session_file, tmp_path):
    bad_extract = MagicMock(side_effect=RuntimeError("LLM down"))
    drafts = tmp_path / "drafts"
    drafts.mkdir()

    new_cursor, drafts_written = observer.process_session(
        session_file, 0, bad_extract, drafts
    )
    assert drafts_written == 0
    # cursor NOT advanced — we'll retry next tick
    assert new_cursor == 0
