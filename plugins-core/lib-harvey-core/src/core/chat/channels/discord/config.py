"""Discord bot config — credentials and filter settings for the Discord channel."""

from dataclasses import dataclass
from typing import List, Optional


@dataclass
class DiscordConfig:
    """Discord bot credentials and channel filters.

    Loaded from data/chat/config.json → discord section, with env overrides:
        DISCORD_BOT_TOKEN     (Bot ...)
        DISCORD_GUILD_ID     (server snowflake)
        DISCORD_CHANNEL_ID   (#office channel snowflake)
        DISCORD_ALLOWED_USER_IDS  (comma-separated user IDs, optional)
    """

    bot_token: Optional[str] = ""
    guild_id: Optional[int] = None
    channel_id: Optional[int] = None  # the #office channel
    allowed_user_ids: List[int] = None  # empty = allow anyone in the configured channel
    polling_timeout: int = 30
    ignore_bots: bool = True  # silently drop messages from other bots

    def __post_init__(self):
        if self.allowed_user_ids is None:
            self.allowed_user_ids = []

    def is_configured(self) -> bool:
        return bool(self.bot_token and self.guild_id and self.channel_id)
