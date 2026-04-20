"""
HarveyChat — External communication gateway for Harvey OS.

Core Tool #3 alongside Brain (memory) and switchAILocal (inference).
Lets Sebastian talk to Harvey from any device via Telegram, with
extensible channel architecture for WhatsApp/Slack/Discord later.

Usage:
    from core.chat.gateway import HarveyChat
    chat = HarveyChat()
    chat.run()

CLI:
    python3 -m core.chat start    # Start gateway daemon
    python3 -m core.chat stop     # Stop gateway
    python3 -m core.chat status   # Health check
"""

__version__ = "0.1.0"
