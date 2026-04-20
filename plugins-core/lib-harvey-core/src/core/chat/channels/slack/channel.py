"""SlackChannel — stub implementing ChannelPlugin.

Unlike WhatsApp, Slack supports message editing + streaming, so when the real
implementation lands it can immediately participate in the StreamingProgress
Bridge from Phase 3 (live draft edits as the agent works).
"""

from __future__ import annotations

import logging
from typing import Any, Awaitable, Callable, Optional

from core.chat.channels.base import ChannelPlugin
from core.chat.channels.capabilities import ChannelCapability
from .config import SlackConfig

log = logging.getLogger("harvey.chat.slack")


class SlackChannel(ChannelPlugin):
    """Slack Bolt stub. Replace with a real Socket Mode client when a workspace exists."""

    def __init__(self, config: Optional[SlackConfig] = None, on_message: Optional[Callable] = None):
        self.config = config or SlackConfig()
        self.on_message = on_message
        self._running = False

    @property
    def name(self) -> str:
        return "slack"

    @property
    def capabilities(self) -> ChannelCapability:
        # Slack supports edit_message + streaming (chat.update) — participates
        # in StreamingProgressBridge as soon as the real impl lands
        return ChannelCapability.SLACK_STANDARD

    def is_configured(self) -> bool:
        return self.config.is_configured()

    async def start(self, on_message: Callable[..., Awaitable[Any]]) -> None:
        if not self.is_configured():
            log.info("[slack] stub — no credentials, not starting")
            return
        log.warning("[slack] stub start — real Socket Mode client not implemented")
        self.on_message = on_message
        self._running = True

    async def stop(self) -> None:
        self._running = False
        log.info("[slack] stub stopped")

    async def send_text(
        self,
        target: str,
        text: str,
        *,
        reply_to: Optional[str] = None,
        parse_mode: Optional[str] = None,
    ) -> Optional[str]:
        raise NotImplementedError("slack: send_text not yet implemented (stub)")
