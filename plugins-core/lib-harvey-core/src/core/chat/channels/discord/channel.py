"""
DiscordChannel — implements ChannelPlugin for the Makakoo chat gateway.

Uses discord.py in polling/long-running mode. Filters messages to a single
configured guild + channel so the bot only responds in #office (or whatever
channel is configured), never in DMs or other servers.

The on_message callback signature matches the gateway's expected contract:
    on_message(channel="discord", user_id=str(user_id), username=username, text=text) -> str

Capability set:
    DISCORD_STANDARD = SEND_DOCUMENT | SEND_PHOTO | TYPING_INDICATOR | EDIT_MESSAGE | DELETE_MESSAGE
"""

from __future__ import annotations

import logging
import os
from typing import Any, Awaitable, Callable, List, Optional

import discord

from core.chat.channels.base import ChannelPlugin
from core.chat.channels.capabilities import ChannelCapability
from .config import DiscordConfig

log = logging.getLogger("harvey.chat.discord")


def _chunk_text(text: str, limit: int = 1900) -> List[str]:
    """Split long text into Discord-safe chunks at paragraph/line boundaries."""
    if len(text) <= limit:
        return [text]
    chunks: List[str] = []
    current = ""
    for line in text.splitlines():
        if len(current) + len(line) + 1 > limit:
            chunks.append(current)
            current = line
        else:
            current = f"{current}\n{line}" if current else line
    if current:
        chunks.append(current)
    return chunks


class DiscordChannel(ChannelPlugin):
    """Discord bot channel implementing ChannelPlugin."""

    def __init__(self, config: DiscordConfig):
        self.config = config
        self._client: Optional[discord.Client] = None
        self._on_message: Optional[Callable[..., Awaitable[str]]] = None
        self._target_channel: Optional[discord.abc.Messageable] = None
        self.last_activity_time: float = 0.0

    @property
    def name(self) -> str:
        return "discord"

    @property
    def capabilities(self) -> ChannelCapability:
        return ChannelCapability.DISCORD_STANDARD

    def is_configured(self) -> bool:
        return self.config.is_configured()

    async def start(self, on_message: Callable[..., Awaitable[str]]) -> None:
        """Start the Discord bot — long-running client, not webhooks."""
        if not self.is_configured():
            log.error("[discord] bot token, guild_id, or channel_id not configured")
            return

        self._on_message = on_message

        intents = discord.Intents.default()
        intents.message_content = True
        self._client = discord.Client(intents=intents)

        guild_id = self.config.guild_id
        channel_id = self.config.channel_id

        @self._client.event
        async def on_ready():
            log.info(
                f"[discord] bot ready as {self._client.user} "
                f"(guild={guild_id}, channel={channel_id})"
            )
            try:
                guild = self._client.get_guild(guild_id)
                if guild:
                    self._target_channel = guild.get_channel(channel_id)
                    log.info(f"[discord] target channel: {self._target_channel}")
            except Exception as e:
                log.warning(f"[discord] could not resolve target channel: {e}")

        @self._client.event
        async def on_message(discord_msg: discord.Message):
            """Bridge Discord message to gateway's on_message callback."""
            author_name = getattr(discord_msg.author, 'username', None) or \
                          getattr(discord_msg.author, 'display_name', None) or \
                          getattr(discord_msg.author, 'name', None) or 'unknown'
            log.info(
                f"[discord] RAW msg from {author_name} "
                f"guild={discord_msg.guild.id if discord_msg.guild else None} "
                f"channel={discord_msg.channel.id} content={discord_msg.content[:50]!r}"
            )

            if discord_msg.author.bot and self.config.ignore_bots:
                log.info("[discord] skipped: author is a bot")
                return
            if guild_id and discord_msg.guild and discord_msg.guild.id != guild_id:
                log.info(f"[discord] skipped: guild {discord_msg.guild.id} != {guild_id}")
                return
            if channel_id and discord_msg.channel.id != channel_id:
                log.info(f"[discord] skipped: channel {discord_msg.channel.id} != {channel_id}")
                return

            text = discord_msg.content.strip()
            if not text or len(text) < 2:
                log.info(f"[discord] skipped: text too short: {text!r}")
                return
            if text.startswith("http"):
                log.info("[discord] skipped: URL")
                return
            if text.startswith("```") and text.endswith("```"):
                log.info("[discord] skipped: code block")
                return

            user_id = str(discord_msg.author.id)
            username = getattr(discord_msg.author, 'username', None) or \
                       getattr(discord_msg.author, 'display_name', None) or \
                       getattr(discord_msg.author, 'name', None) or user_id
            self.last_activity_time = discord_msg.created_at.timestamp()

            log.info(
                f"[discord] HANDLING msg from @{username} ({user_id}) "
                f"in channel {discord_msg.channel.id}: {text[:80]!r}"
            )

            # Discord hides bot typing indicators in guild channels.
            # Send a visible "Thinking..." placeholder instead.
            thinking_msg: Optional[discord.Message] = None
            try:
                thinking_msg = await discord_msg.channel.send(
                    "🤔 Thinking..."
                )
            except discord.DiscordException:
                pass  # still process even if placeholder fails

            try:
                response = await self._on_message(
                    channel="discord",
                    user_id=user_id,
                    username=username,
                    text=text,
                )
            except Exception as e:
                log.error(f"[discord] on_message error: {e}", exc_info=True)
                response = "Something broke on my end. Check the CLI for details."

            if not response:
                if thinking_msg:
                    try:
                        await thinking_msg.delete()
                    except discord.DiscordException:
                        pass
                return

            self._target_channel = discord_msg.channel

            chunks = _chunk_text(response)
            if thinking_msg and len(chunks) == 1 and len(chunks[0]) <= 1900:
                # Single short response — edit the placeholder in-place
                try:
                    await thinking_msg.edit(content=chunks[0])
                    return
                except discord.DiscordException:
                    pass  # fall through to delete + resend

            # Multi-chunk or edit failed — delete placeholder and send fresh
            if thinking_msg:
                try:
                    await thinking_msg.delete()
                except discord.DiscordException:
                    pass

            for chunk in chunks:
                try:
                    await discord_msg.channel.send(chunk)
                except discord.DiscordException as e:
                    log.warning(f"[discord] send error: {e}")

        await self._client.start(self.config.bot_token)

    async def stop(self) -> None:
        """Stop the Discord bot gracefully."""
        if self._client:
            log.info("[discord] stopping bot...")
            await self._client.close()
            self._client = None
            self._target_channel = None
            log.info("[discord] bot stopped.")

    def _resolve_target(self, target: str) -> Optional[discord.abc.Messageable]:
        """Resolve a target string to a Discord channel."""
        if self._target_channel:
            return self._target_channel
        if self._client:
            for guild in self._client.guilds:
                ch = guild.get_channel(int(target))
                if ch:
                    return ch
        return None

    async def send_text(
        self,
        target: str,
        text: str,
        *,
        reply_to: Optional[str] = None,
        parse_mode: Optional[str] = None,
    ) -> Optional[str]:
        """Send a text message to target channel or user."""
        channel = self._resolve_target(target)
        if not channel:
            log.warning(f"[discord] send_text: no channel for target {target!r}")
            return None
        last_msg_id: Optional[str] = None
        for chunk in _chunk_text(text):
            try:
                msg = await channel.send(chunk)
                if msg:
                    last_msg_id = str(msg.id)
            except discord.DiscordException as e:
                log.warning(f"[discord] send_text error: {e}")
                return None
        return last_msg_id

    async def send_typing(self, target: str, active: bool = True) -> None:
        """Show/hide typing indicator — no-op for Discord bot API."""
        if ChannelCapability.TYPING_INDICATOR not in self.capabilities:
            raise NotImplementedError(f"{self.name} does not support typing indicators")

    async def send_document(
        self,
        target: str,
        path: str,
        *,
        caption: Optional[str] = None,
        filename: Optional[str] = None,
    ) -> Optional[str]:
        """Send a file to target channel."""
        if not os.path.exists(path):
            log.warning(f"[discord] send_document: file not found {path}")
            return None
        channel = self._resolve_target(target)
        if not channel:
            log.warning(f"[discord] send_document: no channel for target {target!r}")
            return None
        try:
            kwargs: dict = {"content": caption} if caption else {}
            msg = await channel.send(file=discord.File(path), **kwargs)
            log.info(f"[discord] sent document {path} to channel")
            return str(msg.id) if msg else None
        except discord.DiscordException as e:
            log.warning(f"[discord] send_document error: {e}")
            return None

    async def send_photo(
        self,
        target: str,
        path: str,
        *,
        caption: Optional[str] = None,
    ) -> Optional[str]:
        """Send an image to target channel."""
        if not os.path.exists(path):
            log.warning(f"[discord] send_photo: file not found {path}")
            return None
        channel = self._resolve_target(target)
        if not channel:
            log.warning(f"[discord] send_photo: no channel for target {target!r}")
            return None
        try:
            kwargs: dict = {"content": caption} if caption else {}
            msg = await channel.send(file=discord.File(path), **kwargs)
            log.info(f"[discord] sent photo {path} to channel")
            return str(msg.id) if msg else None
        except discord.DiscordException as e:
            log.warning(f"[discord] send_photo error: {e}")
            return None

    async def edit_text(
        self,
        target: str,
        message_id: str,
        text: str,
        *,
        parse_mode: Optional[str] = None,
    ) -> bool:
        """Edit an existing message."""
        if ChannelCapability.EDIT_MESSAGE not in self.capabilities:
            raise NotImplementedError(f"{self.name} does not support editing messages")
        channel = self._resolve_target(target)
        if not channel:
            return False
        try:
            message = await channel.fetch_message(int(message_id))
            if message and message.author == self._client.user:
                await message.edit(content=text)
                return True
        except discord.DiscordException as e:
            log.warning(f"[discord] edit_text error: {e}")
        return False

    async def delete_message(self, target: str, message_id: str) -> bool:
        """Delete a message."""
        if ChannelCapability.DELETE_MESSAGE not in self.capabilities:
            raise NotImplementedError(f"{self.name} does not support deleting messages")
        channel = self._resolve_target(target)
        if not channel:
            return False
        try:
            message = await channel.fetch_message(int(message_id))
            if message and message.author == self._client.user:
                await message.delete()
                return True
        except discord.DiscordException as e:
            log.warning(f"[discord] delete_message error: {e}")
        return False
