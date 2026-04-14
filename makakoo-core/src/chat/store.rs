//! `ChatStore` — SQLite-backed conversation + message persistence.
//!
//! Port of `harvey-os/core/chat/store.py`. The underlying tables
//! (`chat_messages`, `chat_sessions`) are already provisioned by
//! `makakoo_core::db::run_migrations`, so this module only opens the
//! connection and issues queries — no schema DDL lives here.
//!
//! Semantics that match the Python source of truth:
//!
//! * A "conversation" is a `(channel, channel_user_id)` tuple. Messages are
//!   appended in order; `chat_sessions` tracks a rolling session window.
//! * `get_or_create_conversation` rolls a session forward if the last message
//!   is within 3600s, otherwise it starts a new session — matching
//!   `ChatStore.add_message` in Python.
//! * `recent_messages` returns newest-first-then-reversed, so the caller
//!   always sees messages in chronological order.
//! * `search_messages` is a plain substring match (`LIKE ?`) rather than
//!   FTS5 because the canonical chat table is not FTS-indexed — the Python
//!   side also searches by substring.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{open_db, run_migrations};
use crate::error::{MakakooError, Result};

/// A conversation is the `(channel, user_id)` pair; its `id` is the
/// `chat_sessions.id` of the most recent active session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: i64,
    pub channel: String,
    pub user_id: String,
    pub user_display: String,
    pub started_at: DateTime<Utc>,
    pub last_message_at: DateTime<Utc>,
}

/// One message stored in `chat_messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub ts: DateTime<Utc>,
    pub tokens: Option<u32>,
    pub metadata: Value,
}

/// Aggregate counters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ChatStats {
    pub conversations: usize,
    pub messages: usize,
    pub active_today: usize,
}

/// SQLite-backed chat store. Shared across threads via `Arc<Mutex<_>>`.
#[derive(Clone)]
pub struct ChatStore {
    conn: Arc<Mutex<Connection>>,
}

impl ChatStore {
    /// Open or create a store at `path`. Runs core migrations on every call
    /// so the `chat_*` tables exist before any query fires.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = open_db(path)?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| MakakooError::internal("chat store mutex poisoned"))
    }

    /// Return the active session for `(channel, user_id)`, or create a new
    /// one if none exists or the previous one has been idle for >3600s.
    ///
    /// `user_display` is stored into `chat_sessions.metadata.display` so
    /// the caller can recover a human-readable handle later without
    /// piggy-backing on a separate profile table.
    pub fn get_or_create_conversation(
        &self,
        channel: &str,
        user_id: &str,
        user_display: &str,
    ) -> Result<Conversation> {
        let conn = self.lock()?;
        let now = unix_now();

        let row: Option<(i64, f64, f64, String)> = conn
            .query_row(
                "SELECT id, started_at, last_active, metadata FROM chat_sessions
                 WHERE channel = ?1 AND channel_user_id = ?2
                 ORDER BY last_active DESC LIMIT 1",
                params![channel, user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;

        if let Some((id, started, last_active, _metadata)) = row {
            if now - last_active < 3600.0 {
                // Same session still warm.
                return Ok(Conversation {
                    id,
                    channel: channel.to_string(),
                    user_id: user_id.to_string(),
                    user_display: user_display.to_string(),
                    started_at: ts_from_secs(started),
                    last_message_at: ts_from_secs(last_active),
                });
            }
        }

        // Start a fresh session.
        let metadata = serde_json::json!({ "display": user_display }).to_string();
        conn.execute(
            "INSERT INTO chat_sessions (channel, channel_user_id, started_at, last_active, message_count, metadata)
             VALUES (?1, ?2, ?3, ?3, 0, ?4)",
            params![channel, user_id, now, metadata],
        )?;
        let id = conn.last_insert_rowid();
        Ok(Conversation {
            id,
            channel: channel.to_string(),
            user_id: user_id.to_string(),
            user_display: user_display.to_string(),
            started_at: ts_from_secs(now),
            last_message_at: ts_from_secs(now),
        })
    }

    /// Append a message to a conversation and bump the session's counters.
    /// Returns the new `chat_messages.id`.
    pub fn append_message(
        &self,
        conv_id: i64,
        role: &str,
        content: &str,
        tokens: Option<u32>,
    ) -> Result<i64> {
        let conn = self.lock()?;
        let now = unix_now();

        // Load the conversation's (channel, user_id) so chat_messages carries
        // the same routing keys the Python schema uses.
        let (channel, user_id): (String, String) = conn.query_row(
            "SELECT channel, channel_user_id FROM chat_sessions WHERE id = ?1",
            params![conv_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let metadata = match tokens {
            Some(t) => serde_json::json!({ "conversation_id": conv_id, "tokens": t }),
            None => serde_json::json!({ "conversation_id": conv_id }),
        }
        .to_string();

        conn.execute(
            "INSERT INTO chat_messages (channel, channel_user_id, role, content, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![channel, user_id, role, content, metadata, now],
        )?;
        let id = conn.last_insert_rowid();

        conn.execute(
            "UPDATE chat_sessions SET last_active = ?1, message_count = message_count + 1 WHERE id = ?2",
            params![now, conv_id],
        )?;
        Ok(id)
    }

    /// Return the `limit` most recent messages for a conversation in
    /// chronological order (oldest first).
    pub fn recent_messages(&self, conv_id: i64, limit: usize) -> Result<Vec<ChatMessage>> {
        let conn = self.lock()?;
        let (channel, user_id): (String, String) = conn.query_row(
            "SELECT channel, channel_user_id FROM chat_sessions WHERE id = ?1",
            params![conv_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, role, content, metadata, created_at FROM chat_messages
             WHERE channel = ?1 AND channel_user_id = ?2
             ORDER BY created_at DESC, id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![channel, user_id, limit as i64],
            |row| {
                let id: i64 = row.get(0)?;
                let role: String = row.get(1)?;
                let content: String = row.get(2)?;
                let metadata_raw: String = row.get(3)?;
                let created_at: f64 = row.get(4)?;
                Ok((id, role, content, metadata_raw, created_at))
            },
        )?;

        let mut out: Vec<ChatMessage> = Vec::new();
        for r in rows {
            let (id, role, content, metadata_raw, created_at) = r?;
            let metadata: Value =
                serde_json::from_str(&metadata_raw).unwrap_or(Value::Null);
            let tokens = metadata
                .get("tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            out.push(ChatMessage {
                id,
                conversation_id: conv_id,
                role,
                content,
                ts: ts_from_secs(created_at),
                tokens,
                metadata,
            });
        }
        out.reverse();
        Ok(out)
    }

    /// List the most recent conversations, optionally filtered by channel.
    pub fn list_conversations(
        &self,
        channel: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Conversation>> {
        let conn = self.lock()?;
        let (sql, has_filter) = match channel {
            Some(_) => (
                "SELECT id, channel, channel_user_id, started_at, last_active, metadata
                 FROM chat_sessions WHERE channel = ?1 ORDER BY last_active DESC LIMIT ?2",
                true,
            ),
            None => (
                "SELECT id, channel, channel_user_id, started_at, last_active, metadata
                 FROM chat_sessions ORDER BY last_active DESC LIMIT ?1",
                false,
            ),
        };
        let mut stmt = conn.prepare(sql)?;

        let mapper = |row: &rusqlite::Row| {
            let id: i64 = row.get(0)?;
            let channel: String = row.get(1)?;
            let user_id: String = row.get(2)?;
            let started_at: f64 = row.get(3)?;
            let last_active: f64 = row.get(4)?;
            let metadata_raw: String = row.get(5)?;
            Ok((
                id,
                channel,
                user_id,
                started_at,
                last_active,
                metadata_raw,
            ))
        };

        let rows: Vec<(i64, String, String, f64, f64, String)> = if has_filter {
            let ch = channel.unwrap();
            stmt.query_map(params![ch, limit as i64], mapper)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![limit as i64], mapper)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        let out = rows
            .into_iter()
            .map(
                |(id, channel, user_id, started_at, last_active, metadata_raw)| {
                    let metadata: Value =
                        serde_json::from_str(&metadata_raw).unwrap_or(Value::Null);
                    let display = metadata
                        .get("display")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Conversation {
                        id,
                        channel,
                        user_id,
                        user_display: display,
                        started_at: ts_from_secs(started_at),
                        last_message_at: ts_from_secs(last_active),
                    }
                },
            )
            .collect();
        Ok(out)
    }

    pub fn message_count(&self) -> Result<usize> {
        let conn = self.lock()?;
        let n: i64 =
            conn.query_row("SELECT COUNT(*) FROM chat_messages", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    pub fn conversation_count(&self) -> Result<usize> {
        let conn = self.lock()?;
        let n: i64 =
            conn.query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    pub fn stats(&self) -> Result<ChatStats> {
        let conn = self.lock()?;
        let messages: i64 =
            conn.query_row("SELECT COUNT(*) FROM chat_messages", [], |row| row.get(0))?;
        let conversations: i64 =
            conn.query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))?;
        let since = unix_now() - 86400.0;
        let active_today: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chat_sessions WHERE last_active >= ?1",
            params![since],
            |row| row.get(0),
        )?;
        Ok(ChatStats {
            conversations: conversations as usize,
            messages: messages as usize,
            active_today: active_today as usize,
        })
    }

    /// Substring search over message content. Returns newest-first.
    pub fn search_messages(&self, query: &str, limit: usize) -> Result<Vec<ChatMessage>> {
        let conn = self.lock()?;
        let pattern = format!("%{}%", query.replace('%', r"\%").replace('_', r"\_"));
        let mut stmt = conn.prepare(
            "SELECT id, role, content, metadata, created_at FROM chat_messages
             WHERE content LIKE ?1 ESCAPE '\\'
             ORDER BY created_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
            let id: i64 = row.get(0)?;
            let role: String = row.get(1)?;
            let content: String = row.get(2)?;
            let metadata_raw: String = row.get(3)?;
            let created_at: f64 = row.get(4)?;
            Ok((id, role, content, metadata_raw, created_at))
        })?;

        let mut out = Vec::new();
        for r in rows {
            let (id, role, content, metadata_raw, created_at) = r?;
            let metadata: Value =
                serde_json::from_str(&metadata_raw).unwrap_or(Value::Null);
            let tokens = metadata
                .get("tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let conversation_id = metadata
                .get("conversation_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            out.push(ChatMessage {
                id,
                conversation_id,
                role,
                content,
                ts: ts_from_secs(created_at),
                tokens,
                metadata,
            });
        }
        Ok(out)
    }
}

fn unix_now() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn ts_from_secs(secs: f64) -> DateTime<Utc> {
    let whole = secs.trunc() as i64;
    let nanos = ((secs - secs.trunc()) * 1e9) as u32;
    Utc.timestamp_opt(whole, nanos)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (tempfile::TempDir, ChatStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");
        let store = ChatStore::open(&path).unwrap();
        (dir, store)
    }

    #[test]
    fn open_creates_tables() {
        let (_dir, store) = tmp_store();
        assert_eq!(store.message_count().unwrap(), 0);
        assert_eq!(store.conversation_count().unwrap(), 0);
    }

    #[test]
    fn get_or_create_is_idempotent_within_session_window() {
        let (_dir, store) = tmp_store();
        let a = store
            .get_or_create_conversation("telegram", "42", "alice")
            .unwrap();
        let b = store
            .get_or_create_conversation("telegram", "42", "alice")
            .unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(store.conversation_count().unwrap(), 1);
    }

    #[test]
    fn different_users_get_different_conversations() {
        let (_dir, store) = tmp_store();
        let a = store
            .get_or_create_conversation("telegram", "1", "alice")
            .unwrap();
        let b = store
            .get_or_create_conversation("telegram", "2", "bob")
            .unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(store.conversation_count().unwrap(), 2);
    }

    #[test]
    fn append_and_recent_return_chronological_order() {
        let (_dir, store) = tmp_store();
        let conv = store
            .get_or_create_conversation("cli", "u1", "alice")
            .unwrap();
        for i in 0..5 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            store
                .append_message(conv.id, role, &format!("msg-{i}"), None)
                .unwrap();
        }
        let recent = store.recent_messages(conv.id, 10).unwrap();
        assert_eq!(recent.len(), 5);
        for (i, m) in recent.iter().enumerate() {
            assert_eq!(m.content, format!("msg-{i}"));
        }
    }

    #[test]
    fn recent_respects_limit() {
        let (_dir, store) = tmp_store();
        let conv = store
            .get_or_create_conversation("cli", "u1", "alice")
            .unwrap();
        for i in 0..10 {
            store
                .append_message(conv.id, "user", &format!("m{i}"), None)
                .unwrap();
        }
        let recent = store.recent_messages(conv.id, 3).unwrap();
        assert_eq!(recent.len(), 3);
        // Most recent three in chronological order: m7, m8, m9.
        assert_eq!(recent[0].content, "m7");
        assert_eq!(recent[2].content, "m9");
    }

    #[test]
    fn message_count_tracks_appends() {
        let (_dir, store) = tmp_store();
        let conv = store
            .get_or_create_conversation("cli", "u1", "alice")
            .unwrap();
        assert_eq!(store.message_count().unwrap(), 0);
        store.append_message(conv.id, "user", "hi", None).unwrap();
        store
            .append_message(conv.id, "assistant", "hello", Some(12))
            .unwrap();
        assert_eq!(store.message_count().unwrap(), 2);
    }

    #[test]
    fn list_conversations_filters_by_channel() {
        let (_dir, store) = tmp_store();
        store
            .get_or_create_conversation("telegram", "1", "alice")
            .unwrap();
        store
            .get_or_create_conversation("cli", "1", "alice")
            .unwrap();
        store
            .get_or_create_conversation("mcp", "1", "alice")
            .unwrap();
        let all = store.list_conversations(None, 10).unwrap();
        assert_eq!(all.len(), 3);
        let tg = store.list_conversations(Some("telegram"), 10).unwrap();
        assert_eq!(tg.len(), 1);
        assert_eq!(tg[0].channel, "telegram");
    }

    #[test]
    fn search_messages_finds_substring() {
        let (_dir, store) = tmp_store();
        let conv = store
            .get_or_create_conversation("cli", "u1", "alice")
            .unwrap();
        store
            .append_message(conv.id, "user", "please find polymarket", None)
            .unwrap();
        store
            .append_message(conv.id, "assistant", "done", None)
            .unwrap();
        store
            .append_message(conv.id, "user", "also remember quantum", None)
            .unwrap();
        let hits = store.search_messages("polymarket", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("polymarket"));
    }

    #[test]
    fn search_respects_limit() {
        let (_dir, store) = tmp_store();
        let conv = store
            .get_or_create_conversation("cli", "u1", "alice")
            .unwrap();
        for i in 0..5 {
            store
                .append_message(conv.id, "user", &format!("needle-{i}"), None)
                .unwrap();
        }
        let hits = store.search_messages("needle", 2).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn stats_reports_counts() {
        let (_dir, store) = tmp_store();
        let conv = store
            .get_or_create_conversation("cli", "u1", "alice")
            .unwrap();
        store.append_message(conv.id, "user", "hi", None).unwrap();
        store
            .append_message(conv.id, "assistant", "hi back", None)
            .unwrap();
        let s = store.stats().unwrap();
        assert_eq!(s.conversations, 1);
        assert_eq!(s.messages, 2);
        assert_eq!(s.active_today, 1);
    }

    #[test]
    fn tokens_metadata_roundtrips() {
        let (_dir, store) = tmp_store();
        let conv = store
            .get_or_create_conversation("cli", "u1", "alice")
            .unwrap();
        store
            .append_message(conv.id, "assistant", "word", Some(7))
            .unwrap();
        let recent = store.recent_messages(conv.id, 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].tokens, Some(7));
    }
}
