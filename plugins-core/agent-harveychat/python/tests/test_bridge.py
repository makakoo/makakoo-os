"""Tests for the IPC framing layer."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from plugins_core.agent_harveychat.python.bridge import (
    InboundFrame,
    OutboundFrame,
    backoff_for_attempt,
    encode_outbound,
    ipc_socket_path,
    parse_envelope,
)


def _inbound_envelope() -> str:
    return json.dumps(
        {
            "kind": "inbound",
            "frame": {
                "agent_slot_id": "secretary",
                "transport_id": "telegram-main",
                "transport_kind": "telegram",
                "account_id": "12345678",
                "conversation_id": "746496145",
                "sender_id": "746496145",
                "thread_id": None,
                "thread_kind": None,
                "message_id": "42",
                "text": "hello",
                "transport_timestamp": "1700000000",
                "received_at": "2026-04-26T12:00:00.000000000Z",
                "raw_metadata": {},
            },
        }
    )


def test_parse_envelope_decodes_inbound():
    line = _inbound_envelope()
    frame = parse_envelope(line)
    assert isinstance(frame, InboundFrame)
    assert frame.agent_slot_id == "secretary"
    assert frame.transport_id == "telegram-main"
    assert frame.transport_kind == "telegram"
    assert frame.text == "hello"
    assert frame.thread_id is None


def test_parse_envelope_returns_none_for_outbound():
    line = json.dumps(
        {
            "kind": "outbound",
            "frame": {"transport_id": "x", "text": "y"},
        }
    )
    assert parse_envelope(line) is None


def test_parse_envelope_raises_on_missing_frame():
    line = json.dumps({"kind": "inbound"})
    with pytest.raises(ValueError):
        parse_envelope(line)


def test_encode_outbound_round_trips():
    out = OutboundFrame(
        transport_id="telegram-main",
        transport_kind="telegram",
        conversation_id="746496145",
        text="hi back",
        reply_to_message_id="42",
    )
    line = encode_outbound(out)
    assert line.endswith("\n")
    decoded = json.loads(line)
    assert decoded["kind"] == "outbound"
    assert decoded["frame"]["transport_id"] == "telegram-main"
    assert decoded["frame"]["text"] == "hi back"
    assert decoded["frame"]["reply_to_message_id"] == "42"


def test_encode_outbound_carries_thread_when_set():
    out = OutboundFrame(
        transport_id="slack-main",
        transport_kind="slack",
        conversation_id="C0123",
        text="threaded reply",
        thread_id="1700000000.123456",
        thread_kind="slack_thread",
    )
    decoded = json.loads(encode_outbound(out))
    assert decoded["frame"]["thread_id"] == "1700000000.123456"
    assert decoded["frame"]["thread_kind"] == "slack_thread"


def test_ipc_socket_path_locked():
    home = Path("/Users/sebastian/MAKAKOO")
    p = ipc_socket_path(home, "secretary")
    assert str(p) == "/Users/sebastian/MAKAKOO/run/agents/secretary/ipc.sock"


def test_backoff_grows_then_caps():
    # First attempt is at the floor.
    assert 0.5 * 0.75 <= backoff_for_attempt(0) <= 0.5 * 1.25
    # Saturated at 30s ± 25% jitter for high attempts.
    high = backoff_for_attempt(20)
    assert 30 * 0.75 <= high <= 30 * 1.25


def test_inbound_frame_from_dict_roundtrip():
    src = {
        "agent_slot_id": "x",
        "transport_id": "t",
        "transport_kind": "telegram",
        "account_id": "1",
        "conversation_id": "2",
        "sender_id": "3",
        "thread_id": None,
        "thread_kind": None,
        "message_id": "4",
        "text": "hi",
        "transport_timestamp": None,
        "received_at": "2026-04-26T12:00:00Z",
    }
    f = InboundFrame.from_dict(src)
    assert f.agent_slot_id == "x"
    assert f.raw_metadata == {}
