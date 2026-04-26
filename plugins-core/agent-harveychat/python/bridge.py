"""Newline-JSON IPC client — Python ↔ Rust supervisor.

Locked contract: `docs/specs/ipc-contract-v2.md`.

* Connect to the Unix-domain socket at
  `~/MAKAKOO/run/agents/<slot>/ipc.sock`.
* Read frames as newline-delimited JSON.
* Write frames as newline-delimited JSON.
* On disconnect, reconnect with exponential backoff
  (500 ms → 30 s, jittered).
* Per-stream serialization: one writer task; readers are
  deserialized in arrival order.
"""

from __future__ import annotations

import asyncio
import json
import os
import random
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, AsyncIterator, Optional


def ipc_socket_path(makakoo_home: Path, slot_id: str) -> Path:
    """Locked path: `~/MAKAKOO/run/agents/<slot>/ipc.sock`."""
    return makakoo_home / "run" / "agents" / slot_id / "ipc.sock"


@dataclass
class InboundFrame:
    """Mirror of `MakakooInboundFrame` from frame.rs."""

    agent_slot_id: str
    transport_id: str
    transport_kind: str
    account_id: str
    conversation_id: str
    sender_id: str
    thread_id: Optional[str]
    thread_kind: Optional[str]
    message_id: str
    text: str
    transport_timestamp: Optional[str]
    received_at: str
    raw_metadata: dict = field(default_factory=dict)

    @classmethod
    def from_dict(cls, d: dict) -> "InboundFrame":
        return cls(
            agent_slot_id=d["agent_slot_id"],
            transport_id=d["transport_id"],
            transport_kind=d["transport_kind"],
            account_id=d["account_id"],
            conversation_id=d["conversation_id"],
            sender_id=d["sender_id"],
            thread_id=d.get("thread_id"),
            thread_kind=d.get("thread_kind"),
            message_id=d["message_id"],
            text=d["text"],
            transport_timestamp=d.get("transport_timestamp"),
            received_at=d["received_at"],
            raw_metadata=d.get("raw_metadata", {}) or {},
        )


@dataclass
class OutboundFrame:
    """Mirror of `MakakooOutboundFrame` from frame.rs."""

    transport_id: str
    transport_kind: str
    conversation_id: str
    text: str
    thread_id: Optional[str] = None
    thread_kind: Optional[str] = None
    reply_to_message_id: Optional[str] = None

    def to_envelope(self) -> dict:
        return {
            "kind": "outbound",
            "frame": {
                "transport_id": self.transport_id,
                "transport_kind": self.transport_kind,
                "conversation_id": self.conversation_id,
                "thread_id": self.thread_id,
                "thread_kind": self.thread_kind,
                "text": self.text,
                "reply_to_message_id": self.reply_to_message_id,
            },
        }


def parse_envelope(line: str) -> Optional[InboundFrame]:
    """Parse one newline-stripped JSON line. Returns None for
    non-inbound envelopes (the gateway only reads inbound)."""
    obj = json.loads(line)
    kind = obj.get("kind")
    if kind != "inbound":
        return None
    frame = obj.get("frame")
    if not isinstance(frame, dict):
        raise ValueError(f"inbound frame missing or wrong type: {obj!r}")
    return InboundFrame.from_dict(frame)


def encode_outbound(frame: OutboundFrame) -> str:
    """Encode an OutboundFrame as one newline-terminated JSON line."""
    return json.dumps(frame.to_envelope(), separators=(",", ":")) + "\n"


# ── Backoff schedule ─────────────────────────────────────────────────

_BACKOFF_MIN_S = 0.5
_BACKOFF_MAX_S = 30.0


def backoff_for_attempt(attempt: int) -> float:
    """Locked schedule: 0.5, 1, 2, 4, ..., 30 (jittered ±25%)."""
    base = _BACKOFF_MIN_S * (2 ** min(attempt, 8))
    capped = min(base, _BACKOFF_MAX_S)
    jitter = capped * 0.25 * (random.random() * 2 - 1)
    return max(_BACKOFF_MIN_S, capped + jitter)


# ── IPC client ───────────────────────────────────────────────────────


class IpcClient:
    """Async newline-JSON IPC client.

    Usage::

        client = IpcClient(socket_path)
        async for frame in client.frames():
            reply = ...
            await client.send(reply)
    """

    def __init__(self, socket_path: Path) -> None:
        self.socket_path = socket_path
        self._writer: Optional[asyncio.StreamWriter] = None
        self._writer_lock = asyncio.Lock()
        self._reader: Optional[asyncio.StreamReader] = None

    async def _connect(self) -> None:
        """Open one connection. Blocks until the socket is reachable
        (the supervisor binds before spawning the gateway, so there
        should be no race in practice)."""
        attempt = 0
        while True:
            try:
                self._reader, self._writer = await asyncio.open_unix_connection(
                    str(self.socket_path)
                )
                return
            except (FileNotFoundError, ConnectionRefusedError):
                delay = backoff_for_attempt(attempt)
                await asyncio.sleep(delay)
                attempt += 1

    async def frames(self) -> AsyncIterator[InboundFrame]:
        """Yield InboundFrames forever, reconnecting on disconnect."""
        attempt = 0
        while True:
            if self._reader is None:
                await self._connect()
                attempt = 0
            assert self._reader is not None
            try:
                line = await self._reader.readline()
            except (ConnectionResetError, BrokenPipeError):
                line = b""
            if not line:
                # Disconnect — reset and reconnect on next loop.
                self._reader = None
                self._writer = None
                delay = backoff_for_attempt(attempt)
                attempt += 1
                await asyncio.sleep(delay)
                continue
            text = line.decode("utf-8").rstrip("\n")
            try:
                frame = parse_envelope(text)
            except (json.JSONDecodeError, ValueError) as e:
                # Malformed line — log to stderr and skip. The Rust
                # side never emits malformed lines so this is a real
                # bug worth surfacing.
                print(f"bridge: bad inbound line: {text!r}: {e}", flush=True)
                continue
            if frame is None:
                continue
            yield frame

    async def send(self, frame: OutboundFrame) -> None:
        """Write one outbound frame. Per-writer lock serializes
        concurrent calls so frames don't interleave."""
        async with self._writer_lock:
            if self._writer is None:
                await self._connect()
            assert self._writer is not None
            payload = encode_outbound(frame).encode("utf-8")
            self._writer.write(payload)
            await self._writer.drain()


def env_makakoo_home() -> Path:
    """Resolve `$MAKAKOO_HOME`, falling back to `~/MAKAKOO`."""
    raw = os.environ.get("MAKAKOO_HOME")
    if raw:
        return Path(raw).expanduser()
    return Path.home() / "MAKAKOO"


def env_slot_id() -> str:
    """Resolve `$MAKAKOO_AGENT_SLOT`. Empty / missing → raise."""
    slot = os.environ.get("MAKAKOO_AGENT_SLOT", "").strip()
    if not slot:
        raise RuntimeError(
            "MAKAKOO_AGENT_SLOT not set — the Python gateway must be "
            "spawned by the supervisor, which sets this var."
        )
    return slot
