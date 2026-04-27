"""
ChannelRegistry — dynamic discovery of chat channel plugins.

Each channel package registers itself on import. The gateway looks up
configured channels by name and instantiates them without any hardcoded
imports. Adding a new channel becomes a drop-in package with an
`__init__.py` that calls `ChannelRegistry.register(name, cls)`.

Usage:
    # In channels/telegram/__init__.py
    from core.chat.channels.registry import ChannelRegistry
    from .channel import TelegramChannel
    ChannelRegistry.register("telegram", TelegramChannel)

    # In gateway
    for name, cfg in config.channels.items():
        if cfg.get("enabled"):
            ch = ChannelRegistry.create(name, cfg, on_message)
            self.channels.append(ch)
"""

from __future__ import annotations

import logging
from typing import Any, Callable, Dict, List, Optional, Type

log = logging.getLogger("harvey.chat.registry")


class ChannelRegistry:
    """Global singleton-style registry. Channel classes register on import."""

    _channels: Dict[str, Type[Any]] = {}

    @classmethod
    def register(cls, name: str, channel_class: Type[Any]) -> None:
        if name in cls._channels:
            log.warning(f"ChannelRegistry: re-registering channel '{name}' (was {cls._channels[name].__name__})")
        cls._channels[name] = channel_class
        log.info(f"ChannelRegistry: registered '{name}' → {channel_class.__name__}")

    @classmethod
    def unregister(cls, name: str) -> bool:
        return cls._channels.pop(name, None) is not None

    @classmethod
    def get(cls, name: str) -> Optional[Type[Any]]:
        return cls._channels.get(name)

    @classmethod
    def create(cls, name: str, *args: Any, **kwargs: Any) -> Any:
        """Instantiate a registered channel with the given config/args.

        Raises ValueError if the channel isn't registered.
        """
        if name not in cls._channels:
            available = ", ".join(cls._channels.keys()) or "(none)"
            raise ValueError(
                f"Unknown channel: '{name}'. Registered: {available}"
            )
        return cls._channels[name](*args, **kwargs)

    @classmethod
    def available(cls) -> List[str]:
        return sorted(cls._channels.keys())

    @classmethod
    def clear(cls) -> None:
        """Test hook — wipe the registry."""
        cls._channels.clear()
