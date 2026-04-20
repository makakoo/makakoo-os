"""
Channel capability flags — bitmask of what a channel supports.

Lets the gateway and task system ask "can this channel do X?" before
calling methods that might not exist. Example: the streaming progress
bridge checks for `STREAMING` before trying to edit a draft message;
channels without that capability receive one final message instead.
"""

from enum import Flag, auto


class ChannelCapability(Flag):
    NONE = 0

    # Typing / action indicators (Telegram sendChatAction, Slack "typing...")
    TYPING_INDICATOR = auto()

    # Message editing — the backbone of the streaming progress bridge
    EDIT_MESSAGE = auto()
    DELETE_MESSAGE = auto()

    # Media
    SEND_DOCUMENT = auto()
    SEND_PHOTO = auto()
    SEND_VIDEO = auto()
    SEND_AUDIO = auto()

    # Threading / replies
    REPLY_TO_MESSAGE = auto()
    FORWARD_MESSAGE = auto()
    PIN_MESSAGE = auto()

    # Interactive
    INLINE_BUTTONS = auto()

    # Streaming progress — channel can receive live edits during long tasks
    # (implies EDIT_MESSAGE but is a distinct capability so channels can
    # opt out of streaming even if they support edits — e.g. email)
    STREAMING = auto()

    # Batch send
    MEDIA_GROUP = auto()

    # Common presets
    TELEGRAM_STANDARD = (
        TYPING_INDICATOR
        | EDIT_MESSAGE
        | DELETE_MESSAGE
        | SEND_DOCUMENT
        | SEND_PHOTO
        | SEND_VIDEO
        | SEND_AUDIO
        | REPLY_TO_MESSAGE
        | FORWARD_MESSAGE
        | PIN_MESSAGE
        | INLINE_BUTTONS
        | STREAMING
        | MEDIA_GROUP
    )

    SLACK_STANDARD = (
        EDIT_MESSAGE
        | DELETE_MESSAGE
        | SEND_DOCUMENT
        | SEND_PHOTO
        | REPLY_TO_MESSAGE
        | INLINE_BUTTONS
        | STREAMING
    )

    # WABA Cloud: no typing indicator, no edit, only media sending
    WHATSAPP_CLOUD = SEND_DOCUMENT | SEND_PHOTO | SEND_VIDEO | SEND_AUDIO


def has(capabilities: ChannelCapability, cap: ChannelCapability) -> bool:
    """Convenience: `has(ch.capabilities, ChannelCapability.EDIT_MESSAGE)`."""
    return bool(capabilities & cap)
