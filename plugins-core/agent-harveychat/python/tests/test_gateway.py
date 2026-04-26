"""Tests for the gateway dispatch loop."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import List

import pytest

from plugins_core.agent_harveychat.python.bridge import (
    InboundFrame,
    OutboundFrame,
)
from plugins_core.agent_harveychat.python.gateway import (
    LlmRequest,
    SlotConfig,
    handle_inbound,
    load_slot_config,
    render_identity_block,
)


def _slot_toml() -> str:
    return """
slot_id = "secretary"
name = "Secretary"
persona = "Sharp professional secretary."
inherit_baseline = false
allowed_paths = ["~/Office/", "/var/lib/secretary"]
forbidden_paths = ["/var/lib/secretary/secret"]
tools = ["brain_search", "write_file"]
process_mode = "supervised_pair"
"""


def _frame(text: str = "hi", thread_id: str | None = None) -> InboundFrame:
    return InboundFrame(
        agent_slot_id="secretary",
        transport_id="telegram-main",
        transport_kind="telegram",
        account_id="12345678",
        conversation_id="746496145",
        sender_id="746496145",
        thread_id=thread_id,
        thread_kind="telegram_forum" if thread_id else None,
        message_id="42",
        text=text,
        transport_timestamp="1700000000",
        received_at="2026-04-26T12:00:00.000000000Z",
    )


def test_slot_config_from_toml_parses_scope():
    cfg = SlotConfig.from_toml("secretary", _slot_toml())
    assert cfg.slot_id == "secretary"
    assert cfg.name == "Secretary"
    assert cfg.persona == "Sharp professional secretary."
    assert cfg.tool_scope.tools == ["brain_search", "write_file"]
    assert cfg.tool_scope.inherit_baseline is False
    assert "/var/lib/secretary" in cfg.path_scope.allowed


def test_load_slot_config_reads_from_makakoo_home(tmp_path):
    cfg_dir = tmp_path / "config" / "agents"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "secretary.toml").write_text(_slot_toml())
    cfg = load_slot_config(tmp_path, "secretary")
    assert cfg.slot_id == "secretary"
    assert cfg.tool_scope.tools


def test_render_identity_block_locked_phrasing():
    cfg = SlotConfig.from_toml("secretary", _slot_toml())
    block = render_identity_block(
        slot_id="secretary",
        transport_kind="telegram",
        transport_id="telegram-main",
        sender_id="746496145",
        cfg=cfg,
    )
    assert "[agent: secretary]" in block
    assert "[transport: telegram-main (telegram)]" in block
    assert "[user: 746496145]" in block
    assert "[scope.tools: brain_search, write_file]" in block
    assert "/var/lib/secretary" in block
    assert "[scope.paths.forbidden: /var/lib/secretary/secret]" in block


def test_render_identity_block_least_privilege_when_empty_tools():
    slot = """
slot_id = "x"
inherit_baseline = false
"""
    cfg = SlotConfig.from_toml("x", slot)
    block = render_identity_block("x", "telegram", "t", "1", cfg)
    assert "(none — least-privilege default)" in block


@pytest.mark.asyncio
async def test_handle_inbound_calls_dispatch_and_returns_outbound():
    cfg = SlotConfig.from_toml("secretary", _slot_toml())
    captured: List[LlmRequest] = []

    async def fake(req: LlmRequest) -> str:
        captured.append(req)
        return "reply text"

    out = await handle_inbound(_frame("hello"), cfg, fake)
    assert isinstance(out, OutboundFrame)
    assert out.transport_id == "telegram-main"
    assert out.transport_kind == "telegram"
    assert out.conversation_id == "746496145"
    assert out.text == "reply text"
    assert out.reply_to_message_id == "42"

    # Dispatch saw the rendered identity block + raw user text.
    assert len(captured) == 1
    req = captured[0]
    assert "[agent: secretary]" in req.identity_block
    assert req.user_text == "hello"
    assert req.persona == "Sharp professional secretary."


@pytest.mark.asyncio
async def test_handle_inbound_propagates_thread_metadata():
    cfg = SlotConfig.from_toml("secretary", _slot_toml())

    async def fake(_req: LlmRequest) -> str:
        return "x"

    out = await handle_inbound(_frame("hi", thread_id="thread-42"), cfg, fake)
    assert out.thread_id == "thread-42"
    assert out.thread_kind == "telegram_forum"


@pytest.mark.asyncio
async def test_handle_inbound_pins_outbound_to_inbound_transport():
    """Cross-transport reply forbidden in v1: the outbound MUST echo
    the inbound transport_id."""
    cfg = SlotConfig.from_toml("secretary", _slot_toml())

    async def fake(_req: LlmRequest) -> str:
        return "y"

    inbound = _frame("hi")
    out = await handle_inbound(inbound, cfg, fake)
    assert out.transport_id == inbound.transport_id
    assert out.transport_kind == inbound.transport_kind
    assert out.conversation_id == inbound.conversation_id
