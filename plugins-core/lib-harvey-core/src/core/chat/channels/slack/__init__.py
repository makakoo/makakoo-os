"""
Slack channel plugin — STUB.

Registers with ChannelRegistry on import. Real implementation requires a
Slack workspace + bot token + signing secret + Socket Mode app. Deferred
until Sebastian adds a workspace — the architecture is ready.
"""

from core.chat.channels.registry import ChannelRegistry
from .channel import SlackChannel

ChannelRegistry.register("slack", SlackChannel)

__all__ = ["SlackChannel"]
