"""Thin wrapper over the kernel's ``harvey_telegram_send`` capability.

Isolated in its own module so tests can monkey-patch
``telegram_send`` without reaching into every handler. In
production this resolves to whatever the kernel exposes; in test
environments the kernel bridge is absent and we no-op unless the
caller installed a fake.
"""
from __future__ import annotations

from typing import Any, Callable, Optional


_SENDER: Optional[Callable[..., Any]] = None


def telegram_send(text: str, **kwargs) -> Any:
    """Send ``text`` to the user's Telegram. Returns whatever the
    configured sender returns (or ``None`` when unconfigured)."""
    if _SENDER is None:
        return None
    return _SENDER(text, **kwargs)


def set_sender(fn: Optional[Callable[..., Any]]) -> None:
    """Register a sender — tests + the kernel bridge both call this."""
    global _SENDER
    _SENDER = fn
