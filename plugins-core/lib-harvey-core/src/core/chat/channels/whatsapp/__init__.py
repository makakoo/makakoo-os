"""
WhatsApp channel plugin — STUB.

Registers with ChannelRegistry on import. The real implementation is
deferred until Harvey is deployed on a reachable HTTPS host (per sprint
§ 4a question 4 — WABA Cloud requires a public webhook endpoint).

Today this package exists so:
  1. AgentRegistry + ChannelRegistry discover it as "whatsapp"
  2. `harvey agents boot` correctly skips it (is_configured()=False)
  3. Adding the real WABA Cloud transport is a pure additive change
     inside channel.py — no registry wiring needed later.
"""

from core.chat.channels.registry import ChannelRegistry
from .channel import WhatsAppChannel

ChannelRegistry.register("whatsapp", WhatsAppChannel)

__all__ = ["WhatsAppChannel"]
