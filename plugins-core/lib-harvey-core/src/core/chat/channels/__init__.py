"""Chat channels — plugin-based architecture.

Legacy interface: `BaseChannel` (thin — still used by live Telegram channel).
New interface: `ChannelPlugin` (rich, capability-flagged — used by stubs + Phase 2+).
"""

from .base import BaseChannel, ChannelPlugin
from .capabilities import ChannelCapability, has
from .cooldowns import ChannelErrorCooldown
from .errors import ChannelError, ErrorCategory, classify_http_status, classify_os_error
from .registry import ChannelRegistry

__all__ = [
    "BaseChannel",
    "ChannelPlugin",
    "ChannelCapability",
    "ChannelError",
    "ChannelErrorCooldown",
    "ChannelRegistry",
    "ErrorCategory",
    "classify_http_status",
    "classify_os_error",
    "has",
]
