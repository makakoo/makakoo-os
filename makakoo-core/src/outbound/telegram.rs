//! Telegram Bot API adapter.
//!
//! Consumes approved drafts from [`OutboundQueue`] and sends them via the
//! Bot API. The adapter keeps the HARD RULE: nothing is sent unless the
//! draft's status is already `Approved` (the queue enforces this on
//! `mark_sent`, we enforce it pre-flight to give a cleaner error).
//!
//! Chat-id normalization mirrors the Python `telegram_utils.py` behaviour:
//! a bare positive 9+ digit supergroup peer id gets `-100` prepended so the
//! Bot API accepts it.

use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{MakakooError, Result};
use crate::outbound::{Draft, DraftStatus, OutboundQueue};

/// Default Telegram API base. Overridable for tests via [`TelegramAdapter::with_base`].
pub const DEFAULT_TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Shape of a successful `sendMessage` response, just the fields we care about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramSendResult {
    pub ok: bool,
    #[serde(default)]
    pub result: serde_json::Value,
}

/// The adapter proper. Carries the bot token + HTTP client + API base.
pub struct TelegramAdapter {
    token: String,
    http: Client,
    base: String,
}

impl TelegramAdapter {
    /// Build an adapter from an explicit bot token. Prefer `from_env()` in
    /// production — this constructor is for tests and scripted callers.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            http: Client::new(),
            base: DEFAULT_TELEGRAM_API_BASE.to_string(),
        }
    }

    /// Read the bot token from `TELEGRAM_BOT_TOKEN`. Errors with a clear
    /// message if unset — callers surface this upward as "outbound not
    /// configured" rather than swallowing it.
    pub fn from_env() -> Result<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN").map_err(|_| {
            MakakooError::internal(
                "outbound/telegram: TELEGRAM_BOT_TOKEN not set. Adapter disabled.",
            )
        })?;
        Ok(Self::new(token))
    }

    /// Override the API base URL — only useful for integration tests that
    /// point at a wiremock instance.
    #[must_use]
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// Send an approved Draft. Fails loudly if `status != Approved` and
    /// calls `queue.mark_sent(draft.id)` on success.
    ///
    /// This is the ONLY legitimate path from Approved → Sent for the
    /// telegram channel. The queue itself refuses `mark_sent` on anything
    /// not already approved, so both guards hold.
    pub async fn send_approved(&self, queue: &OutboundQueue, draft: &Draft) -> Result<()> {
        if draft.status != DraftStatus::Approved {
            return Err(MakakooError::InvalidInput(format!(
                "telegram: draft {} not approved (status={:?}); refusing to send",
                draft.id, draft.status,
            )));
        }
        if draft.channel != "telegram" {
            return Err(MakakooError::InvalidInput(format!(
                "telegram: draft {} is channel={:?}, not 'telegram'",
                draft.id, draft.channel,
            )));
        }
        let chat_id = normalize_chat_id(&draft.recipient)?;
        self.send_raw(chat_id, &draft.body).await?;
        queue.mark_sent(draft.id)?;
        Ok(())
    }

    /// Low-level send. Exposed for callers that want to send a message
    /// without going through the draft queue (e.g. SANCHO notifications).
    /// Returns `MakakooError::internal` on API errors with the Telegram
    /// `description` field when present.
    pub async fn send_raw(&self, chat_id: i64, text: &str) -> Result<TelegramSendResult> {
        let url = format!("{}/bot{}/sendMessage", self.base, self.token);
        let body = json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown",
        });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| MakakooError::internal(format!("telegram post failed: {e}")))?;
        let status = resp.status();
        let payload: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| MakakooError::internal(format!("telegram body decode: {e}")))?;
        let ok = payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !status.is_success() || !ok {
            let desc = payload
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("no description");
            return Err(MakakooError::internal(format!(
                "telegram sendMessage failed: http={status} desc={desc}"
            )));
        }
        Ok(TelegramSendResult {
            ok,
            result: payload.get("result").cloned().unwrap_or(json!(null)),
        })
    }
}

/// Shared handle useful when multiple MCP handlers want to reuse one
/// adapter + token.
pub type SharedTelegramAdapter = Arc<TelegramAdapter>;

/// Chat-id normalization. Mirrors `telegram_utils.normalize_chat_id` on
/// the Python side.
///
///   * negative numbers pass through (groups / channels / supergroups)
///   * positive < 1_000_000_000 (≤ 9 digits) are user DMs, pass through
///   * positive ≥ 1_000_000_000 (≥ 10 digits) are bare supergroup peer
///     IDs — prepend `-100` via `-1000000000000 - raw` so Telegram
///     sees the correct MTProto→Bot API translation.
pub fn normalize_chat_id(raw: &str) -> Result<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(MakakooError::InvalidInput(
            "telegram: chat id is empty".into(),
        ));
    }
    let n: i64 = s.parse().map_err(|_| {
        MakakooError::InvalidInput(format!("telegram: chat id '{s}' is not an integer"))
    })?;
    if n < 0 {
        return Ok(n);
    }
    if n >= 1_000_000_000 {
        // `-100` prefix is the MTProto→Bot API supergroup translation.
        // Build via arithmetic rather than string manipulation to stay
        // i64-safe.
        return Ok(-(1_000_000_000_000 + n));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{body_json_string, method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn open_queue() -> (tempfile::TempDir, OutboundQueue) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(&dir.path().join("t.db")).unwrap();
        run_migrations(&conn).unwrap();
        let shared = Arc::new(Mutex::new(conn));
        let q = OutboundQueue::open(shared).unwrap();
        (dir, q)
    }

    #[test]
    fn normalize_chat_id_passes_through_negatives() {
        assert_eq!(normalize_chat_id("-12345").unwrap(), -12345);
        assert_eq!(normalize_chat_id("-1003746642416").unwrap(), -1003746642416);
    }

    #[test]
    fn normalize_chat_id_passes_through_small_user_ids() {
        assert_eq!(normalize_chat_id("42").unwrap(), 42);
        assert_eq!(normalize_chat_id("999999999").unwrap(), 999_999_999);
    }

    #[test]
    fn normalize_chat_id_adds_minus100_prefix_for_bare_supergroup_ids() {
        // `3746642416` → `-1003746642416`
        assert_eq!(
            normalize_chat_id("3746642416").unwrap(),
            -1_003_746_642_416,
        );
    }

    #[test]
    fn normalize_chat_id_rejects_garbage() {
        assert!(normalize_chat_id("").is_err());
        assert!(normalize_chat_id("abc").is_err());
        assert!(normalize_chat_id("  ").is_err());
    }

    #[tokio::test]
    async fn send_approved_hits_bot_api_and_marks_sent() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/bot[^/]+/sendMessage$"))
            .and(body_json_string(
                r#"{"chat_id":-1003746642416,"parse_mode":"Markdown","text":"hello group"}"#,
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {"message_id": 1}
            })))
            .mount(&mock)
            .await;

        let (_dir, queue) = open_queue();
        let id = queue
            .draft("telegram", "3746642416", None, "hello group")
            .unwrap();
        queue.approve(id).unwrap();
        let draft = queue.get(id).unwrap().unwrap();

        let adapter = TelegramAdapter::new("FAKE-TOKEN").with_base(mock.uri());
        adapter.send_approved(&queue, &draft).await.unwrap();

        let after = queue.get(id).unwrap().unwrap();
        assert_eq!(after.status, DraftStatus::Sent);
        assert!(after.sent_at.is_some());
    }

    #[tokio::test]
    async fn send_approved_refuses_unapproved_draft() {
        let (_dir, queue) = open_queue();
        let id = queue
            .draft("telegram", "-100123", None, "nope")
            .unwrap();
        let draft = queue.get(id).unwrap().unwrap();
        let adapter = TelegramAdapter::new("FAKE");
        let err = adapter.send_approved(&queue, &draft).await.unwrap_err();
        assert!(matches!(err, MakakooError::InvalidInput(_)));
        // Queue should be untouched.
        let after = queue.get(id).unwrap().unwrap();
        assert_eq!(after.status, DraftStatus::Pending);
    }

    #[tokio::test]
    async fn send_raw_surfaces_api_error_description() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/bot[^/]+/sendMessage$"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "ok": false,
                "description": "chat not found",
            })))
            .mount(&mock)
            .await;
        let adapter = TelegramAdapter::new("FAKE").with_base(mock.uri());
        let err = adapter.send_raw(42, "hey").await.unwrap_err();
        let s = err.to_string();
        assert!(s.contains("chat not found"), "missing desc: {s}");
    }

    #[test]
    fn from_env_errors_when_token_missing() {
        // Clear the var for this test; cargo test is single-process so
        // we restore it at the end.
        let prior = std::env::var("TELEGRAM_BOT_TOKEN").ok();
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        let r = TelegramAdapter::from_env();
        assert!(r.is_err());
        if let Some(v) = prior {
            std::env::set_var("TELEGRAM_BOT_TOKEN", v);
        }
    }
}
