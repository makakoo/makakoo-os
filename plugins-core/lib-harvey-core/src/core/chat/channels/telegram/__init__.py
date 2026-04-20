"""
Telegram channel package.

Phase 2b rename: the legacy `channels/telegram.py` module became a
package so future sub-modules (send.py, monitor.py, inbound.py, errors.py)
can be added without touching callers. For now everything lives in
`channel.py` and is re-exported here so the existing import path —
`from core.chat.channels.telegram import TelegramChannel` — keeps working.

Also registers with ChannelRegistry on import so the new plugin discovery
flow in core.agents.cli can find the live Telegram channel alongside the
WhatsApp/Slack stubs.
"""

from core.chat.channels.registry import ChannelRegistry
from .channel import TelegramChannel

ChannelRegistry.register("telegram", TelegramChannel)

__all__ = ["TelegramChannel"]
