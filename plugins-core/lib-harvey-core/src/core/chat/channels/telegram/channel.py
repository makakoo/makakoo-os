"""
Telegram channel for HarveyChat.

Uses python-telegram-bot library in polling mode (no webhook server needed).
Supports text messages, voice transcription placeholders, and typing indicators.
"""

import asyncio
import logging
import os
import time
from typing import Callable, Awaitable, Optional

from telegram import Update
from telegram.error import RetryAfter
from telegram.ext import (
    Application,
    CommandHandler,
    MessageHandler,
    ContextTypes,
    filters,
)

from core.chat.channels.base import BaseChannel
from core.chat.config import TelegramConfig

log = logging.getLogger("harveychat.telegram")


class TelegramChannel(BaseChannel):
    """Telegram bot channel using long-polling."""

    def __init__(self, config: TelegramConfig):
        self.config = config
        self._app: Optional[Application] = None
        self._on_message: Optional[Callable] = None
        # Track where each user last messaged from, so file/photo replies
        # land in the same chat (group chat_id ≠ user.id).
        self._reply_chat: dict[str, int] = {}
        # Track forum topic/thread per user so replies land in the correct thread
        self._reply_thread: dict[str, Optional[int]] = {}
        # Watchdog: updated on every successful message/poll interaction
        self.last_poll_time: float = 0.0

    @staticmethod
    def _safe_cancel(task: Optional[asyncio.Task]) -> None:
        """Cancel an asyncio task without raising."""
        if task is not None:
            try:
                task.cancel()
            except Exception:
                pass

    def _remember_reply_target(
        self, user_id: str, chat_id: Optional[int], thread_id: Optional[int] = None
    ) -> None:
        if chat_id is not None:
            self._reply_chat[user_id] = chat_id
        self._reply_thread[user_id] = thread_id

    def _resolve_chat_id(self, user_id: str) -> int:
        return self._reply_chat.get(user_id, int(user_id))

    def _resolve_thread_id(self, user_id: str) -> Optional[int]:
        return self._reply_thread.get(user_id)

    @property
    def name(self) -> str:
        return "telegram"

    def is_configured(self) -> bool:
        return bool(self.config.bot_token)

    async def start(self, on_message: Callable[..., Awaitable[str]]):
        """Start Telegram bot polling."""
        if not self.is_configured():
            log.error("Telegram bot token not configured")
            return

        self._on_message = on_message

        self._app = Application.builder().token(self.config.bot_token).build()

        # Register handlers
        self._app.add_handler(CommandHandler("start", self._cmd_start))
        self._app.add_handler(CommandHandler("status", self._cmd_status))
        self._app.add_handler(CommandHandler("clear", self._cmd_clear))
        self._app.add_handler(
            MessageHandler(filters.TEXT & ~filters.COMMAND, self._handle_message)
        )
        self._app.add_handler(
            MessageHandler(filters.VOICE | filters.AUDIO, self._handle_voice)
        )
        self._app.add_handler(MessageHandler(filters.PHOTO, self._handle_photo))
        self._app.add_handler(
            MessageHandler(filters.Document.ALL, self._handle_document)
        )
        self._app.add_handler(MessageHandler(filters.VIDEO, self._handle_video))

        log.info("Telegram bot starting (polling mode)...")
        await self._app.initialize()
        await self._app.start()
        await self._app.updater.start_polling(
            poll_interval=1.0,
            timeout=self.config.polling_timeout,
            drop_pending_updates=True,
        )
        self.last_poll_time = time.time()
        # Heartbeat: update last_poll_time every 15s so the watchdog
        # doesn't false-positive during quiet periods with no messages.
        self._heartbeat_active = True

        async def _poll_heartbeat():
            while self._heartbeat_active:
                self.last_poll_time = time.time()
                await asyncio.sleep(15)

        self._heartbeat_task = asyncio.create_task(_poll_heartbeat())
        log.info("Telegram bot is live.")

    async def stop(self):
        """Stop polling gracefully."""
        self._heartbeat_active = False
        if hasattr(self, '_heartbeat_task'):
            self._heartbeat_task.cancel()
        if self._app:
            log.info("Stopping Telegram bot...")
            await self._app.updater.stop()
            await self._app.stop()
            await self._app.shutdown()
            log.info("Telegram bot stopped.")

    async def _send_with_rate_limit(
        self, chat_id: int, text: str, thread_id: Optional[int] = None
    ):
        """Send a single message with Telegram 429 backoff and forum thread support."""
        kwargs = {"chat_id": chat_id, "text": text, "parse_mode": "HTML"}
        if thread_id is not None:
            kwargs["message_thread_id"] = thread_id
        for attempt in range(3):
            try:
                await self._app.bot.send_message(**kwargs)
                return
            except RetryAfter as e:
                retry_after = e.retry_after or 5
                log.warning(
                    f"Telegram rate limit hit — backing off {retry_after}s "
                    f"(attempt {attempt + 1}/3)"
                )
                await asyncio.sleep(retry_after)
                continue
            except Exception as e:
                # Not a rate limit — try plain text fallback
                try:
                    import re as _re
                    clean = _re.sub(r"<[^>]+>", "", text)
                    plain_kwargs = {"chat_id": chat_id, "text": clean}
                    if thread_id is not None:
                        plain_kwargs["message_thread_id"] = thread_id
                    await self._app.bot.send_message(**plain_kwargs)
                    return
                except Exception:
                    log.warning(f"Failed to send message even as plain text: {e}")
                    return

    async def send(self, user_id: str, text: str, audio_path: str = None):
        """Send a message to a Telegram user. Optionally send audio with text."""
        if self._app:
            chat_id = self._resolve_chat_id(user_id)
            thread_id = self._resolve_thread_id(user_id)

            # If audio is provided, send as voice note
            if audio_path and os.path.exists(audio_path):
                try:
                    kwargs = {
                        "chat_id": chat_id,
                        "voice": open(audio_path, "rb"),
                        "caption": text[:1024] if text else None,
                    }
                    if thread_id is not None:
                        kwargs["message_thread_id"] = thread_id
                    await self._app.bot.send_voice(**kwargs)
                    log.info(f"Sent voice note to {user_id}")
                    return
                except Exception as e:
                    log.warning(f"Failed to send voice: {e}")

            # Split long messages (Telegram limit: 4096 chars)
            chunks = _split_message(text, 4000)
            for i, chunk in enumerate(chunks):
                await self._send_with_rate_limit(chat_id, chunk, thread_id)
                # Small delay between chunks to avoid rate limits
                if i < len(chunks) - 1:
                    await asyncio.sleep(0.3)

    async def send_document(self, user_id: str, file_path: str, caption: str = ""):
        """Send a file (PDF, etc.) to a Telegram user."""
        if not self._app or not os.path.exists(file_path):
            log.warning(f"Cannot send document: file not found {file_path}")
            return False

        try:
            chat_id = self._resolve_chat_id(user_id)
            thread_id = self._resolve_thread_id(user_id)
            kwargs = {
                "chat_id": chat_id,
                "document": open(file_path, "rb"),
                "caption": caption[:1024] if caption else None,
            }
            if thread_id is not None:
                kwargs["message_thread_id"] = thread_id
            await self._app.bot.send_document(**kwargs)
            log.info(f"Sent document {file_path} to chat {chat_id} (user {user_id})")
            return True
        except Exception as e:
            log.error(f"Failed to send document: {e}")
            return False

    async def send_photo(self, user_id: str, file_path: str, caption: str = ""):
        """Send an image to a Telegram user."""
        if not self._app or not os.path.exists(file_path):
            log.warning(f"Cannot send photo: file not found {file_path}")
            return False

        try:
            chat_id = self._resolve_chat_id(user_id)
            thread_id = self._resolve_thread_id(user_id)
            kwargs = {
                "chat_id": chat_id,
                "photo": open(file_path, "rb"),
                "caption": caption[:1024] if caption else None,
            }
            if thread_id is not None:
                kwargs["message_thread_id"] = thread_id
            await self._app.bot.send_photo(**kwargs)
            log.info(f"Sent photo {file_path} to chat {chat_id} (user {user_id})")
            return True
        except Exception as e:
            log.error(f"Failed to send photo: {e}")
            return False

    def _is_allowed(self, user_id: int, chat_id: Optional[int] = None) -> bool:
        """
        Check if a message should be accepted.

        Rules:
          - If both allowed_user_ids AND allowed_chat_ids are empty →
            permissive (allow anyone).
          - Otherwise accept if EITHER the user is explicitly allowed
            OR the chat (group/channel) is explicitly allowed.

        This lets operators express three common shapes:
          1. "only me" — set allowed_user_ids=[my_id], empty chats
          2. "only this group" — empty users, set allowed_chat_ids=[-100…]
          3. "me anywhere + this group" — both lists populated
        """
        user_ids = self.config.allowed_user_ids or []
        chat_ids = self.config.allowed_chat_ids or []

        if not user_ids and not chat_ids:
            return True

        if user_id in user_ids:
            return True
        if chat_id is not None and chat_id in chat_ids:
            return True
        return False

    async def _handle_message(self, update: Update, context: ContextTypes.DEFAULT_TYPE):
        """Handle incoming text message."""
        if not update.message or not update.message.text:
            return

        user = update.effective_user
        if user and user.is_bot and self.config.ignore_bots:
            return

        chat_id = update.effective_chat.id if update.effective_chat else None
        if not self._is_allowed(user.id, chat_id):
            # In groups, stay silent to avoid spamming every message.
            # In 1:1 chats, tell the user they're not authorized.
            is_private = (
                update.effective_chat and update.effective_chat.type == "private"
            )
            if is_private:
                await update.message.reply_text(
                    "Access denied. You are not authorized to talk to Harvey."
                )
            log.warning(
                f"Unauthorized access attempt from user {user.id} "
                f"(@{user.username}) in chat {chat_id}"
            )
            return

        text = update.message.text.strip()
        if not text:
            return

        user_id = str(user.id)
        username = user.username or user.first_name or user_id
        # Capture forum topic thread_id so replies land in the correct thread
        thread_id = update.message.message_thread_id
        self._remember_reply_target(user_id, chat_id, thread_id)

        self.last_poll_time = time.time()
        log.info(
            f"Message from @{username} ({user_id}) in chat {chat_id}"
            f"{f' thread {thread_id}' if thread_id else ''}: {text[:80]}..."
        )

        # Show typing indicator — pass thread_id for forum groups
        typing_kwargs = {"action": "typing"}
        if thread_id is not None:
            typing_kwargs["message_thread_id"] = thread_id
        try:
            await self._app.bot.send_chat_action(
                chat_id=chat_id, **typing_kwargs
            )
        except Exception as e:
            log.debug(f"initial typing action failed: {e}")

        typing_active = True

        async def keep_typing():
            while typing_active:
                await asyncio.sleep(4)
                if not typing_active:
                    break
                try:
                    await self._app.bot.send_chat_action(
                        chat_id=chat_id, **typing_kwargs
                    )
                except RetryAfter as e:
                    await asyncio.sleep(min(e.retry_after, 30))
                except Exception:
                    await asyncio.sleep(5)

        typing_task = asyncio.create_task(keep_typing())

        try:
            response = await self._on_message(
                channel="telegram",
                user_id=user_id,
                username=username,
                text=text,
            )

            # Stop typing indicator
            typing_active = False
            self._safe_cancel(typing_task)

            # Check if we should generate voice response
            voice_mode = False
            history = (
                self.store.get_history("telegram", user_id, limit=1)
                if hasattr(self, "store")
                else []
            )
            if history:
                # Check last user message for voice keyword
                voice_mode = False  # Disabled for now

            # Send response via rate-limited sender (thread-aware)
            chat_id = self._resolve_chat_id(user_id)
            thread_id = self._resolve_thread_id(user_id)
            chunks = _split_message(response, 4000)
            for i, chunk in enumerate(chunks):
                await self._send_with_rate_limit(chat_id, chunk, thread_id)
                if i < len(chunks) - 1:
                    await asyncio.sleep(0.3)

        except Exception as e:
            typing_active = False
            self._safe_cancel(typing_task)
            log.error(f"Error handling message: {e}", exc_info=True)
            await update.message.reply_text(
                "Something broke on my end. Check the CLI for details."
            )

    async def _handle_voice(self, update: Update, context: ContextTypes.DEFAULT_TYPE):
        """Handle voice messages — download, transcribe, and process."""
        if not update.message or not update.message.voice:
            return

        user = update.effective_user
        if user and user.is_bot and self.config.ignore_bots:
            return
        chat_id = update.effective_chat.id if update.effective_chat else None
        if not self._is_allowed(user.id, chat_id):
            return

        user_id = str(user.id)
        username = user.username or user.first_name or user_id
        thread_id = update.message.message_thread_id
        self._remember_reply_target(user_id, chat_id, thread_id)

        voice = update.message.voice
        file_id = voice.file_id

        log.info(
            f"Voice message from @{username} ({user_id}), duration={voice.duration}s"
        )

        # Show typing while we process — thread-aware for forum groups
        typing_kwargs = {"action": "typing"}
        if thread_id is not None:
            typing_kwargs["message_thread_id"] = thread_id
        try:
            await self._app.bot.send_chat_action(chat_id=chat_id, **typing_kwargs)
        except Exception as e:
            log.debug(f"initial typing action failed: {e}")

        typing_active = True

        async def keep_typing():
            while typing_active:
                await asyncio.sleep(0.5)
                if not typing_active:
                    break
                try:
                    await self._app.bot.send_chat_action(chat_id=chat_id, **typing_kwargs)
                except Exception:
                    pass

        typing_task = asyncio.create_task(keep_typing())

        try:
            from core.chat.audio import download_telegram_voice, transcribe_audio

            # Download and convert voice
            wav_path = await download_telegram_voice(file_id, self.config.bot_token)
            if not wav_path:
                await update.message.reply_text(
                    "Couldn't process your voice message. Try again or send text."
                )
                return

            # Transcribe
            text = await asyncio.get_event_loop().run_in_executor(
                None, transcribe_audio, wav_path
            )

            # Clean up temp file
            try:
                os.unlink(wav_path)
            except Exception:
                pass

            if not text:
                await update.message.reply_text(
                    "I couldn't hear anything. Try again or send text."
                )
                return

            log.info(f"Voice transcribed: {text[:100]}...")

            # Stop typing and acknowledge transcription
            typing_active = False
            self._safe_cancel(typing_task)
            await update.message.chat.send_action("typing")

            # Process transcription as a text message
            response = await self._on_message(
                channel="telegram",
                user_id=user_id,
                username=username,
                text=text,
            )

            typing_active = False
            self._safe_cancel(typing_task)

            # Send text response (with optional audio)
            chunks = _split_message(response, 4000)
            for chunk in chunks:
                try:
                    await update.message.reply_text(chunk, parse_mode="HTML")
                except Exception:
                    await update.message.reply_text(chunk)

        except Exception as e:
            typing_active = False
            self._safe_cancel(typing_task)
            log.error(f"Voice processing error: {e}", exc_info=True)
            await update.message.reply_text(
                "Voice processing failed. Send me text for now."
            )

    async def _handle_photo(self, update: Update, context: ContextTypes.DEFAULT_TYPE):
        """Handle photo messages — download and analyze with vision."""
        if not update.message or not update.message.photo:
            return

        user = update.effective_user
        if user and user.is_bot and self.config.ignore_bots:
            return
        chat_id = update.effective_chat.id if update.effective_chat else None
        if not self._is_allowed(user.id, chat_id):
            return

        user_id = str(user.id)
        username = user.username or user.first_name or user_id
        thread_id = update.message.message_thread_id
        self._remember_reply_target(user_id, chat_id, thread_id)

        photo = update.message.photo[-1]  # Get largest photo
        log.info(
            f"Photo from @{username} ({user_id}), size={photo.width}x{photo.height}"
        )

        typing_active = True
        typing_kwargs = {"action": "typing"}
        if thread_id is not None:
            typing_kwargs["message_thread_id"] = thread_id

        async def keep_typing():
            while typing_active:
                try:
                    await self._app.bot.send_chat_action(chat_id=chat_id, **typing_kwargs)
                except RetryAfter as e:
                    await asyncio.sleep(min(e.retry_after, 30))
                except Exception:
                    await asyncio.sleep(5)
                await asyncio.sleep(4)

        typing_task = asyncio.create_task(keep_typing())

        try:
            from core.chat.media import download_telegram_file, describe_image

            # Download photo
            file_path = await download_telegram_file(
                photo.file_id, self.config.bot_token
            )
            if not file_path:
                await update.message.reply_text(
                    "Couldn't download your photo. Try again."
                )
                return

            # Analyze with vision
            description = await asyncio.get_event_loop().run_in_executor(
                None, describe_image, file_path
            )

            # Clean up
            try:
                os.unlink(file_path)
            except Exception:
                pass

            typing_active = False
            self._safe_cancel(typing_task)

            if not description:
                description = "(I couldn't see anything in the image)"

            # Ask the LLM about this image
            response = await self._on_message(
                channel="telegram",
                user_id=user_id,
                username=username,
                text=f"[User sent an image]\nImage description: {description}\n\nWhat is this about?",
            )

            chunks = _split_message(response, 4000)
            for chunk in chunks:
                try:
                    await update.message.reply_text(chunk, parse_mode="HTML")
                except Exception:
                    await update.message.reply_text(chunk)

        except Exception as e:
            typing_active = False
            self._safe_cancel(typing_task)
            log.error(f"Photo processing error: {e}", exc_info=True)
            await update.message.reply_text(
                "Photo processing failed. Send again or describe it."
            )

    async def _handle_document(
        self, update: Update, context: ContextTypes.DEFAULT_TYPE
    ):
        """Handle document/PDF uploads — download, extract text, and summarize."""
        if not update.message or not update.message.document:
            return

        user = update.effective_user
        if user and user.is_bot and self.config.ignore_bots:
            return
        chat_id = update.effective_chat.id if update.effective_chat else None
        if not self._is_allowed(user.id, chat_id):
            return

        user_id = str(user.id)
        username = user.username or user.first_name or user_id
        thread_id = update.message.message_thread_id
        self._remember_reply_target(user_id, chat_id, thread_id)

        doc = update.message.document
        file_name = doc.file_name or "attachment"
        log.info(
            f"Document from @{username} ({user_id}): {file_name} ({doc.mime_type})"
        )

        typing_active = True
        typing_kwargs = {"action": "typing"}
        if thread_id is not None:
            typing_kwargs["message_thread_id"] = thread_id

        async def keep_typing():
            while typing_active:
                try:
                    await self._app.bot.send_chat_action(chat_id=chat_id, **typing_kwargs)
                except RetryAfter as e:
                    await asyncio.sleep(min(e.retry_after, 30))
                except Exception:
                    await asyncio.sleep(5)
                await asyncio.sleep(4)

        typing_task = asyncio.create_task(keep_typing())

        try:
            from core.chat.media import download_telegram_file, extract_text_from_file

            file_path = await download_telegram_file(doc.file_id, self.config.bot_token)
            if not file_path:
                await update.message.reply_text(
                    "Couldn't download your document. Try again."
                )
                return

            text = await asyncio.get_event_loop().run_in_executor(
                None, extract_text_from_file, file_path, doc.mime_type or ""
            )

            try:
                os.unlink(file_path)
            except Exception:
                pass

            typing_active = False
            self._safe_cancel(typing_task)

            if not text:
                response = await self._on_message(
                    channel="telegram",
                    user_id=user_id,
                    username=username,
                    text=f"[User sent a file: {file_name}]",
                )
            else:
                # Truncate very long text
                preview = text[:3000] + ("..." if len(text) > 3000 else "")
                response = await self._on_message(
                    channel="telegram",
                    user_id=user_id,
                    username=username,
                    text=f"[User sent a file: {file_name}]\n\nFile content:\n{preview}",
                )

            chunks = _split_message(response, 4000)
            for chunk in chunks:
                try:
                    await update.message.reply_text(chunk, parse_mode="HTML")
                except Exception:
                    await update.message.reply_text(chunk)

        except Exception as e:
            typing_active = False
            self._safe_cancel(typing_task)
            log.error(f"Document processing error: {e}", exc_info=True)
            await update.message.reply_text(
                "Document processing failed. Try again or describe it."
            )

    async def _handle_video(self, update: Update, context: ContextTypes.DEFAULT_TYPE):
        """Handle video messages — download and summarize with omni video."""
        if not update.message or not update.message.video:
            return

        user = update.effective_user
        if user and user.is_bot and self.config.ignore_bots:
            return
        chat_id = update.effective_chat.id if update.effective_chat else None
        if not self._is_allowed(user.id, chat_id):
            return

        user_id = str(user.id)
        username = user.username or user.first_name or user_id
        thread_id = update.message.message_thread_id
        self._remember_reply_target(user_id, chat_id, thread_id)

        video = update.message.video
        log.info(f"Video from @{username} ({user_id}), duration={video.duration}s")

        typing_active = True
        typing_kwargs = {"action": "typing"}
        if thread_id is not None:
            typing_kwargs["message_thread_id"] = thread_id

        async def keep_typing():
            while typing_active:
                try:
                    await self._app.bot.send_chat_action(chat_id=chat_id, **typing_kwargs)
                except RetryAfter as e:
                    await asyncio.sleep(min(e.retry_after, 30))
                except Exception:
                    await asyncio.sleep(5)
                await asyncio.sleep(4)

        typing_task = asyncio.create_task(keep_typing())
        try:
            from core.chat.media import download_telegram_file, describe_video

            file_path = await download_telegram_file(video.file_id, self.config.bot_token)
            if not file_path:
                await update.message.reply_text("Couldn't download your video. Try again.")
                return

            description = await asyncio.get_event_loop().run_in_executor(
                None, describe_video, file_path
            )
            try:
                os.unlink(file_path)
            except Exception:
                pass

            typing_active = False
            self._safe_cancel(typing_task)

            if not description:
                description = "(I couldn't understand the video content.)"

            response = await self._on_message(
                channel="telegram",
                user_id=user_id,
                username=username,
                text=f"[User sent a video]\nVideo description: {description}\n\nWhat is this about?",
            )
            chunks = _split_message(response, 4000)
            for chunk in chunks:
                try:
                    await update.message.reply_text(chunk, parse_mode="HTML")
                except Exception:
                    await update.message.reply_text(chunk)
        except Exception as e:
            typing_active = False
            self._safe_cancel(typing_task)
            log.error(f"Video processing error: {e}", exc_info=True)
            await update.message.reply_text("Video processing failed. Send a screenshot or try again.")

    async def _cmd_start(self, update: Update, context: ContextTypes.DEFAULT_TYPE):
        """Handle /start command."""
        user = update.effective_user
        chat_id = update.effective_chat.id if update.effective_chat else None
        if not self._is_allowed(user.id, chat_id):
            await update.message.reply_text("Access denied.")
            return

        await update.message.reply_text(
            "Harvey online. Talk to me like you would in the CLI.\n\n"
            "Commands:\n"
            "/status — Check my status\n"
            "/clear — Clear conversation history"
        )
        log.info(f"New user started: @{user.username} ({user.id})")

    async def _cmd_status(self, update: Update, context: ContextTypes.DEFAULT_TYPE):
        """Handle /status command."""
        chat_id = update.effective_chat.id if update.effective_chat else None
        if not self._is_allowed(update.effective_user.id, chat_id):
            return

        # This will be overridden by the gateway to include real stats
        if self._on_message:
            response = await self._on_message(
                channel="telegram",
                user_id=str(update.effective_user.id),
                username=update.effective_user.username or "unknown",
                text="/status",
            )
            await update.message.reply_text(response)

    async def _cmd_clear(self, update: Update, context: ContextTypes.DEFAULT_TYPE):
        """Handle /clear command."""
        chat_id = update.effective_chat.id if update.effective_chat else None
        if not self._is_allowed(update.effective_user.id, chat_id):
            return

        if self._on_message:
            response = await self._on_message(
                channel="telegram",
                user_id=str(update.effective_user.id),
                username=update.effective_user.username or "unknown",
                text="/clear",
            )
            await update.message.reply_text(response)


def _split_message(text: str, max_len: int = 4000) -> list:
    """Split a long message into chunks respecting paragraph boundaries."""
    if len(text) <= max_len:
        return [text]

    chunks = []
    while text:
        if len(text) <= max_len:
            chunks.append(text)
            break

        # Try to split at paragraph boundary
        split_at = text.rfind("\n\n", 0, max_len)
        if split_at == -1:
            split_at = text.rfind("\n", 0, max_len)
        if split_at == -1:
            split_at = text.rfind(" ", 0, max_len)
        if split_at == -1:
            split_at = max_len

        chunks.append(text[:split_at])
        text = text[split_at:].lstrip()

    return chunks
