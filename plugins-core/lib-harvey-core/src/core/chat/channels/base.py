"""
Base channel interfaces — all chat channels implement one of these.

Two ABCs live here during the Phase 2 transition:

  BaseChannel    — original thin interface (name/start/stop/send). The live
                   TelegramChannel still implements this. Kept for backward
                   compatibility until Phase 2 migrates it.

  ChannelPlugin  — new rich interface with capability flags and typed
                   send_text/send_document/send_photo/edit_text/send_typing.
                   New channels (WhatsApp, Slack stubs) implement this.
                   Phase 2 migrates Telegram over and deletes BaseChannel.
"""

from abc import ABC, abstractmethod
from typing import Any, Awaitable, Callable, Optional

from .capabilities import ChannelCapability


# ═══════════════════════════════════════════════════════════════
#  Legacy interface (still used by TelegramChannel today)
# ═══════════════════════════════════════════════════════════════


class BaseChannel(ABC):
    """
    A chat channel (Telegram, WhatsApp, Slack, etc.).

    Each channel handles its own transport (polling, webhooks, websocket)
    and normalizes messages into a common format before passing to the
    message handler callback.
    """

    @property
    @abstractmethod
    def name(self) -> str:
        """Channel identifier (e.g., 'telegram', 'whatsapp')."""
        ...

    @abstractmethod
    async def start(self, on_message: Callable[..., Awaitable[str]]):
        """
        Start listening for messages.

        Args:
            on_message: async callback(channel, user_id, username, text) -> response_text
        """
        ...

    @abstractmethod
    async def stop(self):
        """Stop the channel gracefully."""
        ...

    @abstractmethod
    async def send(self, user_id: str, text: str):
        """Send a message to a specific user."""
        ...

    def is_configured(self) -> bool:
        """Check if this channel has required configuration."""
        return True


# ═══════════════════════════════════════════════════════════════
#  New rich interface (Phase 1.5 — used by new channels)
# ═══════════════════════════════════════════════════════════════


class ChannelPlugin(ABC):
    """
    Rich channel plugin interface with capability flags.

    Every chat channel implements this. Callers should inspect
    `.capabilities` before invoking methods that may not be supported —
    default implementations raise NotImplementedError for clean capability
    fallback (rather than returning False-y values that silently hide bugs).

    Example:
        if ChannelCapability.SEND_DOCUMENT in channel.capabilities:
            await channel.send_document(user_id, pdf_path)
        else:
            await channel.send_text(user_id, "here's the report: " + url)
    """

    # ─── Identity and capabilities ──────────────────────────────

    @property
    @abstractmethod
    def name(self) -> str:
        """Channel identifier: 'telegram', 'whatsapp', 'slack'."""
        ...

    @property
    def capabilities(self) -> ChannelCapability:
        """Bitmask of supported features. Subclasses override."""
        return ChannelCapability.NONE

    def is_configured(self) -> bool:
        """Check if required credentials/tokens are present."""
        return True

    # ─── Lifecycle ──────────────────────────────────────────────

    @abstractmethod
    async def start(self, on_message: Callable[..., Awaitable[Any]]) -> None:
        """Start the channel's message listener (poll/webhook/etc.)."""
        ...

    @abstractmethod
    async def stop(self) -> None:
        """Stop gracefully."""
        ...

    # ─── Sending ────────────────────────────────────────────────

    @abstractmethod
    async def send_text(
        self,
        target: str,
        text: str,
        *,
        reply_to: Optional[str] = None,
        parse_mode: Optional[str] = None,
    ) -> Optional[str]:
        """Send a text message. Returns provider message ID on success."""
        ...

    async def send_typing(self, target: str, active: bool = True) -> None:
        """Show/hide typing indicator. No-op if capability not set."""
        if ChannelCapability.TYPING_INDICATOR not in self.capabilities:
            raise NotImplementedError(
                f"{self.name} does not support typing indicators"
            )
        raise NotImplementedError(f"{self.name} did not override send_typing")

    async def edit_text(
        self,
        target: str,
        message_id: str,
        text: str,
        *,
        parse_mode: Optional[str] = None,
    ) -> bool:
        """Edit an existing message. Raises if EDIT_MESSAGE not in capabilities."""
        if ChannelCapability.EDIT_MESSAGE not in self.capabilities:
            raise NotImplementedError(f"{self.name} does not support editing messages")
        raise NotImplementedError(f"{self.name} did not override edit_text")

    async def delete_message(self, target: str, message_id: str) -> bool:
        """Delete a message. Raises if DELETE_MESSAGE not in capabilities."""
        if ChannelCapability.DELETE_MESSAGE not in self.capabilities:
            raise NotImplementedError(f"{self.name} does not support deleting messages")
        raise NotImplementedError(f"{self.name} did not override delete_message")

    async def send_document(
        self,
        target: str,
        path: str,
        *,
        caption: Optional[str] = None,
        filename: Optional[str] = None,
    ) -> Optional[str]:
        """Send a file. Raises if SEND_DOCUMENT not in capabilities."""
        if ChannelCapability.SEND_DOCUMENT not in self.capabilities:
            raise NotImplementedError(f"{self.name} does not support documents")
        raise NotImplementedError(f"{self.name} did not override send_document")

    async def send_photo(
        self,
        target: str,
        path: str,
        *,
        caption: Optional[str] = None,
    ) -> Optional[str]:
        """Send a photo. Raises if SEND_PHOTO not in capabilities."""
        if ChannelCapability.SEND_PHOTO not in self.capabilities:
            raise NotImplementedError(f"{self.name} does not support photos")
        raise NotImplementedError(f"{self.name} did not override send_photo")
