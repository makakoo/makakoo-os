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
    preflight_path_access,
    preflight_request,
    preflight_tool_call,
    render_identity_block,
)
from plugins_core.agent_harveychat.python.file_enforcement import PathNotInScopeError
from plugins_core.agent_harveychat.python.tool_dispatcher import ToolNotInScopeError


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


def test_render_identity_block_byte_compatible_with_rust():
    """Rust's `agents::identity::render_identity_block` produces:
        You are {name}. Your slot id is {slot}. This message
        arrived via {transport_kind}. Your allowed tools are X.
        Your allowed paths are Y.
    The Python output MUST match byte-for-byte (per Phase 3 spec)."""
    cfg = SlotConfig.from_toml("secretary", _slot_toml())
    block = render_identity_block(
        slot_id="secretary",
        transport_kind="telegram",
        transport_id="telegram-main",
        sender_id="746496145",
        cfg=cfg,
    )
    assert block == (
        "You are Secretary. Your slot id is secretary. "
        "This message arrived via telegram. "
        "Your allowed tools are brain_search, write_file. "
        "Your allowed paths are ~/Office/, /var/lib/secretary."
    )


def test_render_identity_block_least_privilege_when_empty_tools():
    slot = """
slot_id = "x"
inherit_baseline = false
"""
    cfg = SlotConfig.from_toml("x", slot)
    block = render_identity_block("x", "telegram", "t", "1", cfg)
    assert "(baseline)" in block or "(none — least-privilege default)" in block


def test_preflight_tool_call_rejects_outside_whitelist():
    cfg = SlotConfig.from_toml("secretary", _slot_toml())
    # In whitelist — passes.
    preflight_tool_call(cfg, "brain_search")
    # Outside whitelist — raises.
    with pytest.raises(ToolNotInScopeError):
        preflight_tool_call(cfg, "run_command")


def test_preflight_path_access_rejects_outside_allowed():
    cfg = SlotConfig.from_toml("secretary", _slot_toml())
    preflight_path_access(cfg, "/var/lib/secretary/conv.db")
    with pytest.raises(PathNotInScopeError):
        preflight_path_access(cfg, "/etc/passwd")


def test_preflight_request_rolls_through_tool_calls_in_request():
    cfg = SlotConfig.from_toml("secretary", _slot_toml())
    req = LlmRequest(
        identity_block="ignored",
        user_text="x",
        persona=None,
        pending_tool_calls=[
            {"tool": "brain_search"},
            {"tool": "write_file", "path": "/var/lib/secretary/draft.md"},
        ],
    )
    preflight_request(cfg, req)  # all in scope — no raise

    req.pending_tool_calls.append({"tool": "run_command"})
    with pytest.raises(ToolNotInScopeError):
        preflight_request(cfg, req)


@pytest.mark.asyncio
async def test_handle_inbound_converts_scope_violation_to_polite_reply():
    """When the dispatcher records a tool call outside scope, the
    gateway turns it into a polite outbound text instead of crashing
    or letting the call slip through."""
    cfg = SlotConfig.from_toml("secretary", _slot_toml())

    async def fake(req: LlmRequest) -> str:
        # Dispatcher claimed it wants to call a forbidden tool.
        req.pending_tool_calls.append({"tool": "run_command"})
        return "I'll do that for you."

    out = await handle_inbound(_frame("hi"), cfg, fake)
    # Reply was REPLACED by the polite denial because preflight
    # caught the scope violation.
    assert "I can't do that" in out.text
    assert "run_command" in out.text


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
    # Identity block uses Rust prose form ("You are X. Your slot id
    # is Y..."), NOT the bracket form. Verified byte-for-byte by
    # `test_render_identity_block_byte_compatible_with_rust`.
    assert "You are Secretary" in req.identity_block
    assert "Your slot id is secretary" in req.identity_block
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
