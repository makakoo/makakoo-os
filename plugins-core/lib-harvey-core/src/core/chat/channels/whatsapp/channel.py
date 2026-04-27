"""WhatsAppChannel — stub implementing ChannelPlugin.

No network calls, no subprocess spawns. `is_configured()` returns False so
`harvey agents boot` skips it. Replacing this with a real WABA Cloud client
is a pure additive change — the registry + capability handshake are already
in place.
"""

from __future__ import annotations

import logging
from typing import Any, Awaitable, Callable, Optional

from core.chat.channels.base import ChannelPlugin
from core.chat.channels.capabilities import ChannelCapability
from .config import WhatsAppConfig

log = logging.getLogger("harvey.chat.whatsapp")


class WhatsAppChannel(ChannelPlugin):
    """WABA Cloud API stub. Replace with a real client when a webhook host exists."""

    def __init__(self, config: Optional[WhatsAppConfig] = None, on_message: Optional[Callable] = None):
        self.config = config or WhatsAppConfig()
        self.on_message = on_message
        self._running = False

    @property
    def name(self) -> str:
        return "whatsapp"

    @property
    def capabilities(self) -> ChannelCapability:
        # WABA Cloud: no typing indicator, no edit_message, media only
        return ChannelCapability.WHATSAPP_CLOUD

    def is_configured(self) -> bool:
        return self.config.is_configured()

    async def start(self, on_message: Callable[..., Awaitable[Any]]) -> None:
        if not self.is_configured():
            log.info("[whatsapp] stub — no credentials, not starting")
            return
        log.warning("[whatsapp] stub start — real webhook integration not implemented")
        self.on_message = on_message
        self._running = True

    async def stop(self) -> None:
        self._running = False
        log.info("[whatsapp] stub stopped")

    async def send_text(
        self,
        target: str,
        text: str,
        *,
        reply_to: Optional[str] = None,
        parse_mode: Optional[str] = None,
    ) -> Optional[str]:
        raise NotImplementedError("whatsapp: send_text not yet implemented (stub)")
