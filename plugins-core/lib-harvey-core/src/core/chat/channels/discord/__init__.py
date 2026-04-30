"""
Discord channel package — registers DiscordChannel via ChannelRegistry.

This enables the gateway to auto-discover and boot the Discord channel
without any hardcoded imports. Drop-in model: add this package to
the channel scan path and it registers itself.
"""

from core.chat.channels.registry import ChannelRegistry
from .channel import DiscordChannel

ChannelRegistry.register("discord", DiscordChannel)
