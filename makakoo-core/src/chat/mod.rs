//! Chat subsystem: persistent chat store, intent router, TTS, Telegram bot.
//!
//! Ports the Python `core.chat.*` stack onto the same unified SQLite schema
//! (`chat_messages`, `chat_sessions`, `chat_tasks`, `chat_cooldowns`) that
//! `makakoo_core::db` already provisions. Entry points:
//!
//! * [`ChatStore`] — per-conversation persistence with history + search.
//! * [`IntelligentRouter`] — keyword-heuristic message triage.
//! * [`TelegramBot`] — teloxide driver; replies only to incoming messages
//!   from allow-listed chats, no unsolicited outbound.
//! * [`speak`] — cross-platform text-to-speech subprocess wrapper.
//!
//! Voice ingestion (whisper + ffmpeg) stays on the skill layer and is out of
//! scope for this module.

pub mod router;
pub mod store;
pub mod telegram;
pub mod tts;

pub use router::{IntelligentRouter, RouteDecision};
pub use store::{ChatMessage, ChatStats, ChatStore, Conversation};
pub use telegram::TelegramBot;
pub use tts::speak;
