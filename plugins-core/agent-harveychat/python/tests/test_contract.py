"""IPC contract test: ensures Python parses the EXACT wire shape Rust
emits. Cross-language anchor: both sides assert against the same
fixture file (`fixtures/sample_inbound.json`) — Rust's frame.rs has
a companion test that asserts its serialization matches this file
verbatim. If either side drifts, both tests fail in lockstep.

Phase 3 of v2-mega delivers `docs/specs/ipc-contract-v2.md` as the
spec; this test enforces the spec against the live Python parser.
"""

from __future__ import annotations

import json
from pathlib import Path

from plugins_core.agent_harveychat.python.bridge import (
    InboundFrame,
    OutboundFrame,
    encode_outbound,
    parse_envelope,
)


_FIXTURE_DIR = Path(__file__).parent / "fixtures"


def _load_rust_sample_inbound() -> str:
    """Read the locked Rust-Python golden fixture. Rust's
    `frame.rs::rust_emits_exact_bytes_of_python_contract_fixture`
    asserts its serialization round-trips this file structurally."""
    return (_FIXTURE_DIR / "sample_inbound.json").read_text(encoding="utf-8").strip()


RUST_SAMPLE_INBOUND = _load_rust_sample_inbound()


def test_python_parses_rust_sample_verbatim():
    frame = parse_envelope(RUST_SAMPLE_INBOUND)
    assert frame is not None
    assert frame.agent_slot_id == "secretary"
    assert frame.transport_id == "telegram-main"
    assert frame.transport_kind == "telegram"
    assert frame.account_id == "12345678"
    assert frame.conversation_id == "746496145"
    assert frame.sender_id == "746496145"
    assert frame.thread_id is None
    assert frame.thread_kind is None
    assert frame.message_id == "42"
    assert frame.text == "hello"
    # Values match `frame.rs::sample_inbound()`. Rust's
    # `rust_emits_exact_bytes_of_python_contract_fixture` test
    # enforces the same fixture structurally.
    assert frame.transport_timestamp == "1714123456"
    assert "2026-04-26" in frame.received_at
    assert frame.raw_metadata == {}


def test_python_outbound_matches_rust_expected_shape():
    """The Rust router decodes outbound frames via this exact shape.
    Any drift breaks the cross-transport-reply check."""
    out = OutboundFrame(
        transport_id="telegram-main",
        transport_kind="telegram",
        conversation_id="746496145",
        text="hi back",
        thread_id=None,
        thread_kind=None,
        reply_to_message_id="42",
    )
    decoded = json.loads(encode_outbound(out))
    assert decoded == {
        "kind": "outbound",
        "frame": {
            "transport_id": "telegram-main",
            "transport_kind": "telegram",
            "conversation_id": "746496145",
            "thread_id": None,
            "thread_kind": None,
            "text": "hi back",
            "reply_to_message_id": "42",
        },
    }


def test_thread_kind_enum_serialization_telegram_forum():
    """Rust enum `ThreadKind::TelegramForum` serializes as
    `"telegram_forum"` (snake_case via serde rename_all)."""
    out = OutboundFrame(
        transport_id="telegram-main",
        transport_kind="telegram",
        conversation_id="C0123",
        text="x",
        thread_id="thread-42",
        thread_kind="telegram_forum",
    )
    decoded = json.loads(encode_outbound(out))
    assert decoded["frame"]["thread_kind"] == "telegram_forum"


def test_thread_kind_enum_serialization_slack_thread():
    out = OutboundFrame(
        transport_id="slack-main",
        transport_kind="slack",
        conversation_id="C0123",
        text="x",
        thread_id="1700000000.123456",
        thread_kind="slack_thread",
    )
    decoded = json.loads(encode_outbound(out))
    assert decoded["frame"]["thread_kind"] == "slack_thread"
