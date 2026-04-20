"""
Shared error classification for channel transports.

Every channel eventually hits the same failure modes: DNS failures,
connection resets, rate limits, auth failures, blocked users. Rather than
each channel reinventing retry logic, errors are classified into a small
enum that the polling/cooldown infrastructure understands.

Ported from openclaw's v22-channel architecture (see multi-channel
refactor plan § 6).
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Optional


class ErrorCategory(Enum):
    """How a network error should be handled."""

    # Safe to retry immediately — request never reached the server
    # (ECONNREFUSED, ENOTFOUND, EAI_AGAIN, ENETUNREACH, DNS failures)
    PRE_CONNECT = "pre_connect"

    # May have partially succeeded — don't retry the send (message might
    # have gone through), but the *next* poll should retry connection
    # (ECONNRESET, ETIMEDOUT, ESOCKETTIMEDOUT, mid-stream aborts)
    RECOVERABLE = "recoverable"

    # Channel API returned "too many requests" — back off significantly,
    # honor Retry-After header if present
    RATE_LIMITED = "rate_limited"

    # User blocked the bot — stop sending to this chat for a while
    # (Telegram 403 "bot was blocked by the user")
    BOT_BLOCKED = "bot_blocked"

    # Chat doesn't exist or was deleted
    CHAT_NOT_FOUND = "chat_not_found"

    # Message editing targeted a message that's too old / deleted
    MESSAGE_TOO_OLD = "message_too_old"

    # Message couldn't be parsed (bad HTML/Markdown) — retry as plain text
    PARSE_ERROR = "parse_error"

    # Auth failure, invalid token, permission denied — unrecoverable
    FATAL = "fatal"

    # Unknown — log + surface, treat as RECOVERABLE
    UNKNOWN = "unknown"


@dataclass
class ChannelError:
    """A classified channel transport failure."""

    category: ErrorCategory
    message: str
    original: Optional[BaseException] = None
    http_status: Optional[int] = None
    retry_after_seconds: Optional[float] = None

    def is_retryable(self) -> bool:
        return self.category in (
            ErrorCategory.PRE_CONNECT,
            ErrorCategory.RECOVERABLE,
            ErrorCategory.RATE_LIMITED,
            ErrorCategory.PARSE_ERROR,
            ErrorCategory.UNKNOWN,
        )

    def should_cooldown_chat(self) -> bool:
        """Should this error trigger per-chat suppression?"""
        return self.category in (ErrorCategory.BOT_BLOCKED, ErrorCategory.CHAT_NOT_FOUND)


def classify_http_status(status: int, body: str = "") -> ErrorCategory:
    """Classify an HTTP status code into an ErrorCategory.

    Channels can call this as a cheap first-pass classifier and override
    with their own semantics for specific error codes.
    """
    if status == 400:
        body_l = body.lower()
        if "message to edit not found" in body_l or "message_id_invalid" in body_l:
            return ErrorCategory.MESSAGE_TOO_OLD
        if "can't parse" in body_l or "parse entities" in body_l:
            return ErrorCategory.PARSE_ERROR
        return ErrorCategory.FATAL
    if status == 401:
        return ErrorCategory.FATAL
    if status == 403:
        return ErrorCategory.BOT_BLOCKED
    if status == 404:
        return ErrorCategory.CHAT_NOT_FOUND
    if status == 429:
        return ErrorCategory.RATE_LIMITED
    if 500 <= status <= 599:
        return ErrorCategory.RECOVERABLE
    return ErrorCategory.UNKNOWN


def classify_os_error(exc: BaseException) -> ErrorCategory:
    """Classify a Python OSError/connection exception.

    Strategy: match explicit message markers first (most reliable), then
    fall back to class-name heuristics from most-specific to least. Order
    matters — `ConnectionResetError` class name contains "connect" but is
    actually a recoverable mid-stream failure.
    """
    name = type(exc).__name__.lower()
    msg = str(exc).lower()

    pre_connect_markers = (
        "getaddrinfo",
        "name or service not known",
        "nodename nor servname",
        "temporary failure in name resolution",
        "econnrefused",
        "connection refused",
        "network is unreachable",
    )
    recoverable_markers = (
        "econnreset",
        "connection reset",
        "timed out",
        "read timeout",
        "server disconnected",
        "remote end closed",
    )

    # 1. Explicit PRE_CONNECT markers in message
    if any(m in msg for m in pre_connect_markers):
        return ErrorCategory.PRE_CONNECT

    # 2. Explicit RECOVERABLE markers in message
    if any(m in msg for m in recoverable_markers):
        return ErrorCategory.RECOVERABLE

    # 3. Class-name heuristics — most specific first to avoid false matches
    if "reset" in name:
        return ErrorCategory.RECOVERABLE
    if "timeout" in name:
        return ErrorCategory.RECOVERABLE
    if "refused" in name or "unreachable" in name:
        return ErrorCategory.PRE_CONNECT
    if "connecterror" in name:
        return ErrorCategory.PRE_CONNECT

    return ErrorCategory.UNKNOWN
