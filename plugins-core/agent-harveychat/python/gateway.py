"""LLM dispatch loop — the brains of the per-slot Python gateway.

For each inbound frame:
  1. Render the Phase 3 identity block from the slot's TOML scope.
  2. Preflight `tools` whitelist (defense in depth).
  3. Preflight `allowed_paths` / `forbidden_paths` (defense in depth).
  4. Hand text + identity-block to the LLM dispatcher.
  5. Translate the LLM's reply into an OutboundFrame.
  6. Send.

The LLM call itself is abstracted behind `LlmDispatch` so unit tests
can substitute a mock without mocking an HTTP client.
"""

from __future__ import annotations

import asyncio
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Awaitable, Callable, List, Optional

# Flat sibling imports — the supervisor cd's into this directory so
# the import path is just the filenames. This avoids hyphenated-dir
# Python-module-name pain (`plugins-core/` is not a valid module
# identifier).
from bridge import (  # type: ignore[import-not-found]
    InboundFrame,
    IpcClient,
    OutboundFrame,
    env_makakoo_home,
    env_slot_id,
    ipc_socket_path,
)
from file_enforcement import (  # type: ignore[import-not-found]
    PathNotInScopeError,
    PathScope,
    check_path,
)
from tool_dispatcher import (  # type: ignore[import-not-found]
    ToolNotInScopeError,
    ToolScope,
    check_tool,
)


# ── Slot config loader ──────────────────────────────────────────────


@dataclass
class SlotConfig:
    slot_id: str
    name: str
    persona: Optional[str]
    tool_scope: ToolScope
    path_scope: PathScope

    @classmethod
    def from_toml(cls, slot_id: str, toml_text: str) -> "SlotConfig":
        # tomllib (Python 3.11+) > tomli > toml. Whichever is
        # available wins — plugin.toml pins python>=3.9 so the
        # third-party fallbacks cover 3.9/3.10.
        data: dict
        try:
            import tomllib  # type: ignore[import-not-found]
            data = tomllib.loads(toml_text)
        except ImportError:
            try:
                import tomli  # type: ignore[import-not-found]
                data = tomli.loads(toml_text)
            except ImportError:
                import toml  # type: ignore[import-not-found]
                data = toml.loads(toml_text)
        return cls(
            slot_id=slot_id,
            name=str(data.get("name") or slot_id),
            persona=data.get("persona"),
            tool_scope=ToolScope.from_slot_dict(data),
            path_scope=PathScope.from_slot_dict(data),
        )


def load_slot_config(makakoo_home: Path, slot_id: str) -> SlotConfig:
    """Read `~/MAKAKOO/config/agents/<slot>.toml` and parse."""
    path = makakoo_home / "config" / "agents" / f"{slot_id}.toml"
    return SlotConfig.from_toml(slot_id, path.read_text(encoding="utf-8"))


# ── Identity block renderer (mirror of Rust render_identity_block) ──


def render_identity_block(
    slot_id: str,
    transport_kind: str,
    transport_id: str,
    sender_id: str,
    cfg: SlotConfig,
) -> str:
    """Identity block byte-compatible with Rust
    `agents::identity::render_identity_block`. Locked phrasing:

        You are {name}. Your slot id is {slot}. This message arrived
        via {transport_kind}. Your allowed tools are {tools}. Your
        allowed paths are {paths}.

    The `transport_id` and `sender_id` arguments are accepted for
    parity with future v3 schema but currently NOT included in the
    rendered block (Rust impl doesn't include them either). Keep them
    in the signature so call sites don't shift when v3 expands the
    block.
    """
    _ = (transport_id, sender_id)  # accepted for forward-compat
    name = cfg.name if cfg.name else slot_id
    tools = (
        ", ".join(cfg.tool_scope.tools)
        if cfg.tool_scope.tools
        else "(baseline)"
    )
    paths = (
        ", ".join(cfg.path_scope.allowed)
        if cfg.path_scope.allowed
        else "(none — least-privilege default)"
    )
    block = f"You are {name}. Your slot id is {slot_id}."
    if transport_kind:
        block += f" This message arrived via {transport_kind}."
    block += f" Your allowed tools are {tools}. Your allowed paths are {paths}."
    return block


# ── LLM dispatch abstraction ────────────────────────────────────────


@dataclass
class LlmRequest:
    identity_block: str
    user_text: str
    persona: Optional[str]
    history: List[dict] = field(default_factory=list)
    # Tool-call requests the LLM might want to make. The dispatcher
    # populates these as it streams; the gateway's preflight checks
    # each against the slot's scope BEFORE the call goes out.
    pending_tool_calls: List[dict] = field(default_factory=list)


# Type alias for the async LLM dispatch fn. Accepts an LlmRequest,
# returns a reply string. Tests substitute a fake.
LlmDispatch = Callable[[LlmRequest], Awaitable[str]]


async def echo_dispatch(req: LlmRequest) -> str:
    """Default dispatch for tests / smoke. Echoes back so the loop is
    observable end-to-end without an LLM. Phase 4 plugs in the real
    SwitchAILocal client."""
    return f"echo: {req.user_text}"


# ── Preflight gates ──────────────────────────────────────────────────
#
# Defense-in-depth: the Rust MCP/grant layer is the authoritative
# scope enforcer. These functions run BEFORE the LLM tries to invoke
# a tool so the model sees a friendlier error than a 403.


def preflight_tool_call(cfg: SlotConfig, tool_name: str) -> None:
    """Reject tool calls outside the slot's whitelist.
    Raises `ToolNotInScopeError` on denial."""
    check_tool(cfg.slot_id, cfg.tool_scope, tool_name)


def preflight_path_access(cfg: SlotConfig, path: str) -> None:
    """Reject path access outside the slot's allowed_paths /
    forbidden_paths.
    Raises `PathNotInScopeError` on denial."""
    check_path(cfg.slot_id, cfg.path_scope, path)


def preflight_request(cfg: SlotConfig, req: LlmRequest) -> None:
    """Apply every scope gate to the pending tool calls in `req`.
    Raises on the first denial; the dispatcher should NOT proceed
    with the LLM call until the LLM either retracts the offending
    tool call or rephrases."""
    for call in req.pending_tool_calls:
        tool = call.get("tool")
        if tool:
            preflight_tool_call(cfg, tool)
        path = call.get("path")
        if path:
            preflight_path_access(cfg, path)


# ── Frame → reply ───────────────────────────────────────────────────


async def handle_inbound(
    frame: InboundFrame,
    cfg: SlotConfig,
    dispatch: LlmDispatch,
) -> OutboundFrame:
    """Translate one inbound frame into one outbound frame.

    Flow:
      1. Render identity block (byte-compatible with Rust).
      2. Build LlmRequest. The dispatch function may populate
         `pending_tool_calls` as it considers tool invocations.
      3. Run `preflight_request` to gate any tool/path the dispatcher
         flagged. Denials raise ToolNotInScopeError / PathNotInScopeError;
         the gateway loop catches them and converts to a polite
         reply (the Rust MCP layer would reject the same call with a
         403 audit-log entry).
      4. Get the reply text from dispatch.
      5. Pin the outbound transport_id / kind / conversation_id /
         thread_id to the inbound's, enforcing the cross-transport
         reply rule.
    """
    identity = render_identity_block(
        slot_id=cfg.slot_id,
        transport_kind=frame.transport_kind,
        transport_id=frame.transport_id,
        sender_id=frame.sender_id,
        cfg=cfg,
    )
    request = LlmRequest(
        identity_block=identity,
        user_text=frame.text,
        persona=cfg.persona,
    )
    reply_text = await dispatch(request)
    # After dispatch returns, run preflight on whatever tool calls
    # the dispatcher recorded. In practice the dispatcher will
    # short-circuit before we get here on denial — but defense in
    # depth means we still gate at this layer.
    try:
        preflight_request(cfg, request)
    except (ToolNotInScopeError, PathNotInScopeError) as e:
        reply_text = f"I can't do that: {e}"
    return OutboundFrame(
        transport_id=frame.transport_id,
        transport_kind=frame.transport_kind,
        conversation_id=frame.conversation_id,
        thread_id=frame.thread_id,
        thread_kind=frame.thread_kind,
        text=reply_text,
        reply_to_message_id=frame.message_id,
    )


# ── Top-level loop ──────────────────────────────────────────────────


async def run_gateway_loop(
    makakoo_home: Path,
    slot_id: str,
    *,
    dispatch: LlmDispatch | None = None,
    client: IpcClient | None = None,
) -> None:
    """Connect to the supervisor, then handle inbound frames forever.

    `dispatch` defaults to `echo_dispatch` so the loop is exercisable
    without an LLM. `client` is overridable so tests can inject a fake
    that doesn't open a real socket.
    """
    cfg = load_slot_config(makakoo_home, slot_id)
    dispatch = dispatch or echo_dispatch
    client = client or IpcClient(ipc_socket_path(makakoo_home, slot_id))

    async for frame in client.frames():
        try:
            outbound = await handle_inbound(frame, cfg, dispatch)
            await client.send(outbound)
        except Exception as e:  # noqa: BLE001
            # Gateway must not die on a single bad frame. Log and
            # continue — the supervisor's restart budget will trip
            # if errors really are persistent.
            print(
                json.dumps(
                    {
                        "event": "gateway.handle_error",
                        "slot_id": slot_id,
                        "transport_id": frame.transport_id,
                        "error": str(e),
                    }
                ),
                flush=True,
                file=sys.stderr,
            )


def main() -> None:
    """Entry point invoked by the supervisor: `python3 gateway.py
    --slot <slot>`."""
    import argparse

    parser = argparse.ArgumentParser(description="Makakoo Python gateway")
    parser.add_argument("--slot", help="slot id (overrides $MAKAKOO_AGENT_SLOT)")
    args = parser.parse_args()

    slot_id = args.slot or env_slot_id()
    makakoo_home = env_makakoo_home()
    asyncio.run(run_gateway_loop(makakoo_home, slot_id))


if __name__ == "__main__":
    main()
