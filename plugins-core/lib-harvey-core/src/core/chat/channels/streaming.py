"""
StreamingProgressBridge — turns durable task events into live channel edits.

Subscribes to `task.*` and `tool.*` events on the PersistentEventBus and
translates each one into a `channel.edit_text()` call on a draft message,
giving the user live visibility into what Harvey is doing during long
agentic tasks (instead of several minutes of silence followed by a single
response).

Design decisions (from sprint § 3.2 + § 4a):
  - Uses `ChannelPlugin` (new interface) — capability-gated via
    `ChannelCapability.STREAMING`. Channels without that capability fall
    back to "send the final message only", no draft / no edits.
  - One draft message per task_id. Tracks `{task_id: (target, message_id)}`.
  - Rate-limit: at most 1 edit per 1.5s per draft (Telegram allows ~1/s).
  - Best-effort: edit failures are logged and suppressed — the user will
    still see the final message.
  - task.completed replaces the draft with the final text.
  - task.failed edits to a visible error + a hint.

Usage:

    bridge = StreamingProgressBridge(channel=telegram_channel, event_bus=bus)
    bridge.start()  # subscribes to events
    # ... agent does its work, events fire, bridge edits draft in real time
    bridge.stop()
"""

from __future__ import annotations

import logging
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, Optional

from .base import ChannelPlugin
from .capabilities import ChannelCapability

log = logging.getLogger("harvey.chat.streaming")


# Minimum seconds between two edits on the same draft message.
# Telegram allows ~1 edit/sec; 1.5s gives us headroom.
MIN_EDIT_INTERVAL_SECONDS = 1.5


@dataclass
class DraftState:
    target: str                        # chat_id / user_id the draft lives in
    message_id: str                    # provider-returned message id
    last_edit_at: float = 0.0
    last_content: str = ""
    closed: bool = False               # true after task.completed / task.failed


class StreamingProgressBridge:
    """
    Subscribes to task lifecycle + tool events on the event bus and translates
    them into live `edit_text()` updates on a draft message.

    A draft message is created on `task.created` by calling
    `bridge.open_draft(task_id, target)` from the gateway — we don't create
    drafts blindly from events because the gateway knows the target chat
    and the event does not.

    Callers should:
        1. Instantiate one bridge per channel (or share across if the
           channel supports multiple concurrent targets)
        2. Call `.start()` to subscribe
        3. Call `.open_draft(task_id, target, initial_text)` when starting
           user-facing work
        4. Let events flow — the bridge handles the rest
        5. Call `.stop()` on shutdown
    """

    def __init__(
        self,
        channel: ChannelPlugin,
        event_bus: Any,
        *,
        min_edit_interval: float = MIN_EDIT_INTERVAL_SECONDS,
        send_text_fn: Optional[Callable[..., Any]] = None,
        edit_text_fn: Optional[Callable[..., Any]] = None,
    ):
        """
        Args:
            channel: The ChannelPlugin to send/edit messages on.
            event_bus: PersistentEventBus instance.
            min_edit_interval: Minimum seconds between edits per draft.
            send_text_fn, edit_text_fn: Test hooks for running the bridge
                synchronously (the channel's send_text/edit_text are async,
                which makes unit testing painful). When provided, these are
                called instead of the channel's async methods.
        """
        self.channel = channel
        self.bus = event_bus
        self.min_edit_interval = min_edit_interval
        self._send_text_fn = send_text_fn
        self._edit_text_fn = edit_text_fn

        self._drafts: Dict[str, DraftState] = {}
        self._drafts_lock = threading.RLock()
        self._subscribed = False
        self._edit_count = 0       # observability
        self._skip_count = 0       # throttled edits

    # ─── Capability check ────────────────────────────────────────

    @property
    def streaming_supported(self) -> bool:
        """True if the channel supports live draft edits."""
        caps = getattr(self.channel, "capabilities", ChannelCapability.NONE)
        return ChannelCapability.STREAMING in caps and ChannelCapability.EDIT_MESSAGE in caps

    # ─── Subscription lifecycle ──────────────────────────────────

    def start(self) -> None:
        """Subscribe to the relevant event topics. Idempotent."""
        if self._subscribed:
            return
        # Single catch-all handler that routes by topic
        self.bus.subscribe("task.*", self._on_event)
        self.bus.subscribe("tool.*", self._on_event)
        self.bus.subscribe("agent.*", self._on_event)
        self._subscribed = True
        log.info(
            f"[streaming] subscribed on channel={self.channel.name} "
            f"supported={self.streaming_supported}"
        )

    def stop(self) -> None:
        """Unsubscribe. Idempotent."""
        if not self._subscribed:
            return
        try:
            self.bus.unsubscribe("task.*", self._on_event)
            self.bus.unsubscribe("tool.*", self._on_event)
            self.bus.unsubscribe("agent.*", self._on_event)
        except Exception as e:
            log.warning(f"[streaming] unsubscribe error: {e}")
        self._subscribed = False

    # ─── Draft management ────────────────────────────────────────

    def open_draft(
        self,
        task_id: str,
        target: str,
        initial_text: str = "🧠 Working on it…",
    ) -> Optional[str]:
        """Send an initial 'working on it' draft and remember the message id.

        Returns the provider message id, or None if:
          - the channel does not support streaming (clean fallback: caller
            will send the final message directly when the task completes)
          - the send fails

        Must be called from the gateway AFTER the task is created but BEFORE
        any agent work starts, so the user sees the draft immediately.
        """
        if not self.streaming_supported:
            log.info(
                f"[streaming] channel={self.channel.name} does not support "
                f"streaming — no draft opened for task {task_id[:8]}"
            )
            return None

        message_id = self._invoke_send(target, initial_text)
        if not message_id:
            log.warning(f"[streaming] failed to open draft for task {task_id[:8]}")
            return None

        with self._drafts_lock:
            self._drafts[task_id] = DraftState(
                target=target,
                message_id=message_id,
                # Initialize to 0.0 so the FIRST edit event after the draft is
                # opened is always allowed. Throttling applies between edits,
                # not between draft-open and first edit.
                last_edit_at=0.0,
                last_content=initial_text,
            )
        log.info(f"[streaming] opened draft {message_id} for task {task_id[:8]}")
        return message_id

    def close_draft(self, task_id: str) -> None:
        """Drop the draft tracking entry. Called implicitly on task.completed."""
        with self._drafts_lock:
            state = self._drafts.pop(task_id, None)
            if state:
                state.closed = True

    # ─── Event handling ──────────────────────────────────────────

    def _on_event(self, event) -> None:
        """Route an incoming event to the right handler."""
        task_id = event.data.get("task_id")
        if not task_id:
            return

        with self._drafts_lock:
            state = self._drafts.get(task_id)
        if state is None or state.closed:
            return  # no draft open for this task, or already finalized

        topic = event.type
        if topic == "task.completed":
            self._handle_completed(task_id, event, state)
        elif topic == "task.failed":
            self._handle_failed(task_id, event, state)
        elif topic == "tool.called":
            tool = event.data.get("tool", "?")
            self._edit_throttled(task_id, state, f"🧠 Calling `{tool}`…")
        elif topic == "tool.result":
            tool = event.data.get("tool", "?")
            self._edit_throttled(task_id, state, f"🧠 `{tool}` done — continuing…")
        elif topic == "tool.error":
            tool = event.data.get("tool", "?")
            summary = (event.data.get("summary", "") or "")[:120]
            self._edit_throttled(
                task_id, state, f"⚠ `{tool}` failed: {summary} — retrying strategy"
            )
        elif topic == "task.artifact_created":
            path = event.data.get("path", "")
            kind = event.data.get("kind", "file")
            name = path.rsplit("/", 1)[-1] if path else kind
            self._edit_throttled(
                task_id, state, f"🧠 Created {kind}: `{name}` — wrapping up…"
            )
        elif topic == "task.resumed":
            self._edit_throttled(task_id, state, "🧠 Resuming prior work…")
        # agent.turn_start / turn_end are intentionally ignored — too chatty

    def _handle_completed(self, task_id: str, event, state: DraftState) -> None:
        """task.completed → replace draft with final response + close."""
        # The final response text lives on the gateway side (the event carries
        # only metadata). For now we replace the draft with a completion marker
        # and let the gateway send the full response as a separate message.
        # A follow-up Phase 3b can pass the final text through the event.
        artifact_count = event.data.get("artifact_count", 0)
        if artifact_count:
            final = f"✓ Done — {artifact_count} artifact(s) ready."
        else:
            final = "✓ Done."
        self._force_edit(task_id, state, final)
        self.close_draft(task_id)

    def _handle_failed(self, task_id: str, event, state: DraftState) -> None:
        """task.failed → replace draft with error + close."""
        err = (event.data.get("error", "") or "unknown")[:200]
        self._force_edit(task_id, state, f"❌ Failed: {err}")
        self.close_draft(task_id)

    # ─── Edit helpers ────────────────────────────────────────────

    def _edit_throttled(self, task_id: str, state: DraftState, text: str) -> None:
        """Edit the draft if enough time has passed and content changed."""
        now = time.time()
        if text == state.last_content:
            return
        if now - state.last_edit_at < self.min_edit_interval:
            self._skip_count += 1
            return
        self._do_edit(task_id, state, text, now)

    def _force_edit(self, task_id: str, state: DraftState, text: str) -> None:
        """Edit the draft unconditionally (used for terminal states)."""
        self._do_edit(task_id, state, text, time.time())

    def _do_edit(self, task_id: str, state: DraftState, text: str, now: float) -> None:
        ok = self._invoke_edit(state.target, state.message_id, text)
        if ok:
            with self._drafts_lock:
                state.last_content = text
                state.last_edit_at = now
            self._edit_count += 1
        else:
            log.warning(
                f"[streaming] edit failed for task {task_id[:8]} "
                f"draft={state.message_id}"
            )

    # ─── Channel invocation (sync facade over async methods) ─────

    def _invoke_send(self, target: str, text: str) -> Optional[str]:
        """Send via injected test hook OR channel.send_text. Supports sync+async."""
        if self._send_text_fn is not None:
            return self._send_text_fn(target=target, text=text)
        # Fall back to the real channel.send_text — this path is exercised
        # in production. Unit tests use the send_text_fn hook instead.
        try:
            import asyncio
            coro = self.channel.send_text(target, text)
            if asyncio.iscoroutine(coro):
                try:
                    loop = asyncio.get_event_loop()
                    if loop.is_running():
                        # Fire-and-forget from a running loop
                        asyncio.create_task(coro)
                        return None
                    return loop.run_until_complete(coro)
                except RuntimeError:
                    return asyncio.run(coro)
            return coro
        except Exception as e:
            log.warning(f"[streaming] send_text failed: {e}")
            return None

    def _invoke_edit(self, target: str, message_id: str, text: str) -> bool:
        """Edit via injected test hook OR channel.edit_text. Supports sync+async."""
        if self._edit_text_fn is not None:
            result = self._edit_text_fn(target=target, message_id=message_id, text=text)
            return bool(result) if result is not None else True
        try:
            import asyncio
            coro = self.channel.edit_text(target, message_id, text)
            if asyncio.iscoroutine(coro):
                try:
                    loop = asyncio.get_event_loop()
                    if loop.is_running():
                        asyncio.create_task(coro)
                        return True
                    result = loop.run_until_complete(coro)
                    return bool(result) if result is not None else True
                except RuntimeError:
                    result = asyncio.run(coro)
                    return bool(result) if result is not None else True
            return bool(coro) if coro is not None else True
        except Exception as e:
            log.warning(f"[streaming] edit_text failed: {e}")
            return False

    # ─── Observability ───────────────────────────────────────────

    def stats(self) -> Dict[str, int]:
        with self._drafts_lock:
            return {
                "active_drafts": len(self._drafts),
                "edits_sent": self._edit_count,
                "edits_throttled": self._skip_count,
            }
