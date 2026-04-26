//! Telegram impls of the four channel-ops trait families.
//!
//! Phase 6 / Q6. Wraps an existing `Arc<TelegramAdapter>` to reuse
//! the resolved bot token, `reqwest::Client`, and `api_base` override
//! hook (so wiremock can intercept HTTP in tests).
//!
//! Telegram's per-bot API is intentionally narrow — bots can't
//! enumerate all chats they're in or look up arbitrary users. The
//! impls return [`ChannelOpError::Unsupported`] for ops the API
//! doesn't expose, with a human-readable `reason` so the LLM (and
//! the user) sees a clear explanation rather than a crash.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::time::timeout;

use crate::channel_ops::approval::{
    ApprovalCenter, ApprovalDecision, ApprovalKey, ChannelApprovalAdapter,
};
use crate::channel_ops::directory::{
    ChannelDirectoryAdapter, ChannelKind, ChannelOpError, ChannelSummary, UserSummary,
};
use crate::channel_ops::messaging::{
    BroadcastResult, ChannelMessagingAdapter, MessageRef,
};
use crate::channel_ops::threading::{
    ChannelThreadingAdapter, ThreadParent, ThreadSummary,
};
use crate::transport::telegram::TelegramAdapter;

const KIND: &str = "telegram";

/// Helper: chat_id parsing. Telegram's chat ids are signed 64-bit
/// integers — supergroups are negative.
fn parse_chat_id(s: &str) -> Result<i64, ChannelOpError> {
    s.parse::<i64>().map_err(|_| {
        ChannelOpError::InvalidInput(format!(
            "telegram chat_id '{s}' is not a numeric id"
        ))
    })
}

fn telegram_url(adapter: &TelegramAdapter, method: &str) -> String {
    format!(
        "{}/bot{}/{}",
        adapter.api_base, adapter.bot_token, method
    )
}

#[derive(Deserialize)]
struct TgEnvelope<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

// ────────────────────────────────────────────────────────────────────
// Directory
// ────────────────────────────────────────────────────────────────────

pub struct TelegramDirectory {
    inner: Arc<TelegramAdapter>,
}

impl TelegramDirectory {
    pub fn new(inner: Arc<TelegramAdapter>) -> Self {
        Self { inner }
    }
}

#[derive(Deserialize)]
struct TgChat {
    id: i64,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

#[async_trait]
impl ChannelDirectoryAdapter for TelegramDirectory {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "telegram"
    }

    async fn list_channels(&self) -> Result<Vec<ChannelSummary>, ChannelOpError> {
        // Telegram bots cannot list "all chats they're in". We
        // enrich the configured allowlist via `getChat` — the
        // resulting list is the actual surface the bot can address.
        let mut ids: Vec<&String> = self
            .inner
            .config
            .allowed_chat_ids
            .iter()
            .chain(self.inner.config.allowed_group_ids.iter())
            .collect();
        ids.sort();
        ids.dedup();

        let mut out = Vec::with_capacity(ids.len());
        for id_str in ids {
            let chat_id = parse_chat_id(id_str)?;
            let url = telegram_url(&self.inner, "getChat");
            let resp = self
                .inner
                .http
                .get(&url)
                .query(&[("chat_id", chat_id.to_string())])
                .send()
                .await
                .map_err(|e| ChannelOpError::Http(e.to_string()))?;
            let env: TgEnvelope<TgChat> = resp
                .json()
                .await
                .map_err(|e| ChannelOpError::Http(e.to_string()))?;
            if !env.ok {
                return Err(ChannelOpError::Remote(
                    env.description.unwrap_or_else(|| "unknown".into()),
                ));
            }
            let chat = env
                .result
                .ok_or_else(|| ChannelOpError::Remote("getChat ok=true but no result".into()))?;
            let kind = match chat.kind.as_str() {
                "private" => ChannelKind::Dm,
                "group" | "supergroup" => ChannelKind::Group,
                "channel" => ChannelKind::Channel,
                _ => ChannelKind::Channel,
            };
            out.push(ChannelSummary {
                id: chat.id.to_string(),
                name: chat.title.or(chat.username),
                kind,
                is_member: true, // we're configured to receive from it
            });
        }
        Ok(out)
    }

    async fn list_users(&self) -> Result<Vec<UserSummary>, ChannelOpError> {
        Err(ChannelOpError::Unsupported {
            kind: KIND,
            op: "list_users",
            reason: "Telegram bots cannot enumerate users; use lookup_user with \
                     a known chat_id:user_id pair via getChatMember"
                .into(),
        })
    }

    async fn lookup_user(
        &self,
        query: &str,
    ) -> Result<Option<UserSummary>, ChannelOpError> {
        // Format: "<chat_id>:<user_id>" — both numeric. Anything else
        // is rejected because Telegram has no global user lookup API.
        let (chat_str, user_str) = query.split_once(':').ok_or_else(|| {
            ChannelOpError::Unsupported {
                kind: KIND,
                op: "lookup_user",
                reason: "telegram lookup_user requires query in the form \
                         '<chat_id>:<user_id>' — both numeric"
                    .into(),
            }
        })?;
        let chat_id = parse_chat_id(chat_str)?;
        let user_id = user_str.parse::<i64>().map_err(|_| {
            ChannelOpError::InvalidInput(format!(
                "telegram lookup_user user_id '{user_str}' is not numeric"
            ))
        })?;
        let url = telegram_url(&self.inner, "getChatMember");
        let resp = self
            .inner
            .http
            .get(&url)
            .query(&[
                ("chat_id", chat_id.to_string()),
                ("user_id", user_id.to_string()),
            ])
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;

        #[derive(Deserialize)]
        struct TgChatMember {
            #[serde(default)]
            user: Option<TgChatMemberUser>,
        }
        #[derive(Deserialize)]
        struct TgChatMemberUser {
            id: i64,
            #[serde(default)]
            username: Option<String>,
            #[serde(default)]
            first_name: Option<String>,
            #[serde(default)]
            is_bot: bool,
        }

        let env: TgEnvelope<TgChatMember> = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            // Telegram returns `Bad Request: user not found` — map
            // to Ok(None) instead of error.
            let desc = env.description.unwrap_or_default();
            if desc.to_ascii_lowercase().contains("not found") {
                return Ok(None);
            }
            return Err(ChannelOpError::Remote(desc));
        }
        let user = env.result.and_then(|m| m.user);
        Ok(user.map(|u| UserSummary {
            id: u.id.to_string(),
            display_name: u.first_name.clone(),
            handle: u.username,
            is_bot: u.is_bot,
        }))
    }
}

// ────────────────────────────────────────────────────────────────────
// Messaging
// ────────────────────────────────────────────────────────────────────

pub struct TelegramMessaging {
    inner: Arc<TelegramAdapter>,
}

impl TelegramMessaging {
    pub fn new(inner: Arc<TelegramAdapter>) -> Self {
        Self { inner }
    }

    async fn raw_send(
        &self,
        chat_id_str: &str,
        text: &str,
    ) -> Result<MessageRef, ChannelOpError> {
        let chat_id = parse_chat_id(chat_id_str)?;
        let url = telegram_url(&self.inner, "sendMessage");
        let resp = self
            .inner
            .http
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": text,
            }))
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;

        #[derive(Deserialize)]
        struct TgSentMessage {
            message_id: i64,
            chat: TgSentChat,
        }
        #[derive(Deserialize)]
        struct TgSentChat {
            id: i64,
        }

        let env: TgEnvelope<TgSentMessage> = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            return Err(ChannelOpError::Remote(
                env.description.unwrap_or_else(|| "unknown".into()),
            ));
        }
        let m = env
            .result
            .ok_or_else(|| ChannelOpError::Remote("sendMessage ok=true but no result".into()))?;
        Ok(MessageRef {
            channel_id: m.chat.id.to_string(),
            message_id: m.message_id.to_string(),
        })
    }
}

#[async_trait]
impl ChannelMessagingAdapter for TelegramMessaging {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "telegram"
    }

    async fn send_dm(&self, user_id: &str, text: &str) -> Result<MessageRef, ChannelOpError> {
        // In Telegram, a DM is just a `sendMessage` to a user_id (which
        // is also the chat_id for private chats).
        self.raw_send(user_id, text).await
    }

    async fn send_channel(
        &self,
        channel_id: &str,
        text: &str,
    ) -> Result<MessageRef, ChannelOpError> {
        self.raw_send(channel_id, text).await
    }

    async fn broadcast(
        &self,
        channel_ids: &[String],
        text: &str,
    ) -> Vec<BroadcastResult> {
        let mut out = Vec::with_capacity(channel_ids.len());
        for cid in channel_ids {
            let outcome = self
                .raw_send(cid, text)
                .await
                .map_err(|e| e.to_string());
            out.push(BroadcastResult {
                channel_id: cid.clone(),
                outcome,
            });
        }
        out
    }
}

// ────────────────────────────────────────────────────────────────────
// Threading
// ────────────────────────────────────────────────────────────────────

pub struct TelegramThreading {
    inner: Arc<TelegramAdapter>,
}

impl TelegramThreading {
    pub fn new(inner: Arc<TelegramAdapter>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ChannelThreadingAdapter for TelegramThreading {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "telegram"
    }

    async fn create_thread(
        &self,
        parent: &ThreadParent,
        title: Option<&str>,
    ) -> Result<String, ChannelOpError> {
        // Telegram supports forum topics on supergroups via
        // `createForumTopic`. Anchored-on-message threads (Slack-style)
        // are not a Telegram concept.
        let chat = match parent {
            ThreadParent::Channel(c) => c,
            ThreadParent::Message { .. } => {
                return Err(ChannelOpError::Unsupported {
                    kind: KIND,
                    op: "create_thread",
                    reason: "Telegram does not support threads anchored to a \
                             message — use ThreadParent::Channel pointing at a \
                             forum supergroup"
                        .into(),
                });
            }
        };
        let chat_id = parse_chat_id(chat)?;
        let title = title.unwrap_or("Topic");
        let url = telegram_url(&self.inner, "createForumTopic");
        let resp = self
            .inner
            .http
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "name": title,
            }))
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;

        #[derive(Deserialize)]
        struct TgForumTopic {
            message_thread_id: i64,
        }

        let env: TgEnvelope<TgForumTopic> = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            return Err(ChannelOpError::Remote(
                env.description.unwrap_or_else(|| "unknown".into()),
            ));
        }
        Ok(env
            .result
            .ok_or_else(|| ChannelOpError::Remote("createForumTopic ok=true but no result".into()))?
            .message_thread_id
            .to_string())
    }

    async fn list_threads(
        &self,
        _channel_id: &str,
    ) -> Result<Vec<ThreadSummary>, ChannelOpError> {
        Err(ChannelOpError::Unsupported {
            kind: KIND,
            op: "list_threads",
            reason: "Telegram Bot API does not expose a forum-topic listing \
                     endpoint — track topics in your bot's local state instead"
                .into(),
        })
    }

    async fn follow_thread(&self, _thread_id: &str) -> Result<(), ChannelOpError> {
        // Telegram threads have no follow concept — a bot receives
        // updates for any topic in a chat it's a member of. Treat
        // follow as a local no-op.
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────
// Approval
// ────────────────────────────────────────────────────────────────────

pub struct TelegramApproval {
    inner: Arc<TelegramAdapter>,
    center: Arc<ApprovalCenter>,
    slot_id: String,
}

impl TelegramApproval {
    pub fn new(
        inner: Arc<TelegramAdapter>,
        center: Arc<ApprovalCenter>,
        slot_id: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            center,
            slot_id: slot_id.into(),
        }
    }
}

#[async_trait]
impl ChannelApprovalAdapter for TelegramApproval {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "telegram"
    }

    async fn request_approval(
        &self,
        channel_id: &str,
        prompt: &str,
        timeout_dur: Duration,
    ) -> Result<ApprovalDecision, ChannelOpError> {
        let key = ApprovalKey::new(&self.slot_id, self.transport_id(), channel_id);
        let rx = self.center.register(key.clone());

        // Send the prompt with a clear yes/no instruction. Inline
        // keyboards would be nicer; text fallback works on every
        // Telegram client without bot-specific button handling
        // (Q6: text-fallback yes/no).
        let send = TelegramMessaging::new(self.inner.clone())
            .send_channel(channel_id, &format!("{prompt}\n\nReply: yes / no"))
            .await;
        if let Err(e) = send {
            self.center.drop_pending(&key);
            return Err(e);
        }

        match timeout(timeout_dur, rx).await {
            Ok(Ok(decision)) => Ok(decision),
            Ok(Err(_recv_err)) => {
                // Sender dropped without resolving — treat as timeout.
                self.center.drop_pending(&key);
                Ok(ApprovalDecision::Timeout)
            }
            Err(_) => {
                self.center.drop_pending(&key);
                Ok(ApprovalDecision::Timeout)
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::config::TelegramConfig;
    use crate::transport::TransportContext;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "telegram-main".into(),
        }
    }

    fn cfg() -> TelegramConfig {
        TelegramConfig {
            polling_timeout_seconds: 30,
            allowed_chat_ids: vec!["123".into()],
            allowed_group_ids: vec!["-1001".into()],
            support_thread: false,
        }
    }

    fn adapter(api_base: String) -> Arc<TelegramAdapter> {
        Arc::new(TelegramAdapter::with_api_base(
            ctx(),
            cfg(),
            "T:abc".into(),
            vec!["123".into()],
            api_base,
        ))
    }

    // ── Directory ─────────────────────────────────────────────

    #[tokio::test]
    async fn directory_list_channels_returns_allowlist_enriched() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botT:abc/getChat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "id": 123, "type": "private", "username": "harvey_user" }
            })))
            .mount(&server)
            .await;
        let dir = TelegramDirectory::new(adapter(server.uri()));
        let chans = dir.list_channels().await.unwrap();
        // 2 deduped allowlist entries (123 + -1001), but both responses
        // are mocked to return the same chat envelope; that's fine —
        // the test asserts shape, not unique enrichment.
        assert_eq!(chans.len(), 2);
        assert_eq!(chans[0].kind, ChannelKind::Dm);
        assert!(chans[0].is_member);
    }

    #[tokio::test]
    async fn directory_list_channels_propagates_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botT:abc/getChat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "description": "Forbidden: bot was kicked"
            })))
            .mount(&server)
            .await;
        let dir = TelegramDirectory::new(adapter(server.uri()));
        let err = dir.list_channels().await.unwrap_err();
        assert!(format!("{err}").contains("Forbidden"));
    }

    #[tokio::test]
    async fn directory_list_users_is_unsupported() {
        let dir = TelegramDirectory::new(adapter("http://unused.invalid".into()));
        let err = dir.list_users().await.unwrap_err();
        assert!(matches!(err, ChannelOpError::Unsupported { op: "list_users", .. }));
    }

    #[tokio::test]
    async fn directory_lookup_user_with_bad_query_is_unsupported() {
        let dir = TelegramDirectory::new(adapter("http://unused.invalid".into()));
        let err = dir.lookup_user("not-a-pair").await.unwrap_err();
        assert!(matches!(err, ChannelOpError::Unsupported { .. }));
    }

    #[tokio::test]
    async fn directory_lookup_user_with_chat_user_pair() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botT:abc/getChatMember"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "user": { "id": 9, "username": "alice", "first_name": "Alice", "is_bot": false } }
            })))
            .mount(&server)
            .await;
        let dir = TelegramDirectory::new(adapter(server.uri()));
        let user = dir.lookup_user("123:9").await.unwrap().unwrap();
        assert_eq!(user.id, "9");
        assert_eq!(user.handle.as_deref(), Some("alice"));
        assert!(!user.is_bot);
    }

    // ── Messaging ─────────────────────────────────────────────

    #[tokio::test]
    async fn messaging_send_channel_returns_message_ref() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/botT:abc/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "message_id": 42, "chat": { "id": 123 } }
            })))
            .mount(&server)
            .await;
        let m = TelegramMessaging::new(adapter(server.uri()));
        let r = m.send_channel("123", "hi").await.unwrap();
        assert_eq!(r.message_id, "42");
        assert_eq!(r.channel_id, "123");
    }

    #[tokio::test]
    async fn messaging_send_invalid_chat_id_is_invalid_input() {
        let m = TelegramMessaging::new(adapter("http://unused.invalid".into()));
        let err = m.send_channel("not-a-number", "x").await.unwrap_err();
        assert!(matches!(err, ChannelOpError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn messaging_broadcast_collects_per_channel_outcomes() {
        let server = MockServer::start().await;
        // Always return a successful sendMessage envelope. The
        // broadcast helper should produce one Ok per input channel.
        Mock::given(method("POST"))
            .and(path("/botT:abc/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "message_id": 1, "chat": { "id": 0 } }
            })))
            .mount(&server)
            .await;
        let m = TelegramMessaging::new(adapter(server.uri()));
        let out = m
            .broadcast(&["123".into(), "456".into()], "hello all")
            .await;
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|r| r.outcome.is_ok()));
    }

    // ── Threading ─────────────────────────────────────────────

    #[tokio::test]
    async fn threading_create_thread_uses_create_forum_topic() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/botT:abc/createForumTopic"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "message_thread_id": 7, "name": "topic" }
            })))
            .mount(&server)
            .await;
        let t = TelegramThreading::new(adapter(server.uri()));
        let id = t
            .create_thread(&ThreadParent::Channel("-1001".into()), Some("topic"))
            .await
            .unwrap();
        assert_eq!(id, "7");
    }

    #[tokio::test]
    async fn threading_create_thread_anchored_to_message_is_unsupported() {
        let t = TelegramThreading::new(adapter("http://unused.invalid".into()));
        let err = t
            .create_thread(
                &ThreadParent::Message {
                    channel_id: "123".into(),
                    message_id: "1".into(),
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelOpError::Unsupported { .. }));
    }

    #[tokio::test]
    async fn threading_list_threads_is_unsupported() {
        let t = TelegramThreading::new(adapter("http://unused.invalid".into()));
        let err = t.list_threads("123").await.unwrap_err();
        assert!(matches!(err, ChannelOpError::Unsupported { .. }));
    }

    #[tokio::test]
    async fn threading_follow_thread_is_local_no_op() {
        let t = TelegramThreading::new(adapter("http://unused.invalid".into()));
        // Should succeed without a network call.
        t.follow_thread("anything").await.unwrap();
    }

    // ── Approval ──────────────────────────────────────────────

    #[tokio::test]
    async fn approval_resolves_when_inbound_completes_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/botT:abc/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "message_id": 1, "chat": { "id": 123 } }
            })))
            .mount(&server)
            .await;
        let center = Arc::new(ApprovalCenter::new());
        let approval =
            TelegramApproval::new(adapter(server.uri()), center.clone(), "secretary");

        let key = ApprovalKey::new("secretary", "telegram-main", "123");
        let center_clone = center.clone();
        let resolver = tokio::spawn(async move {
            // Wait briefly so the awaiter has registered.
            tokio::time::sleep(Duration::from_millis(50)).await;
            center_clone.try_resolve(
                &key,
                ApprovalDecision::Approved {
                    actor_id: "U001".into(),
                    at: std::time::SystemTime::now(),
                },
            )
        });

        let decision = approval
            .request_approval("123", "ok to proceed?", Duration::from_secs(2))
            .await
            .unwrap();
        assert!(matches!(decision, ApprovalDecision::Approved { .. }));
        assert!(resolver.await.unwrap());
    }

    #[tokio::test]
    async fn approval_times_out_when_no_reply_arrives() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/botT:abc/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "message_id": 1, "chat": { "id": 123 } }
            })))
            .mount(&server)
            .await;
        let center = Arc::new(ApprovalCenter::new());
        let approval = TelegramApproval::new(adapter(server.uri()), center, "secretary");
        let decision = approval
            .request_approval("123", "ok?", Duration::from_millis(50))
            .await
            .unwrap();
        assert_eq!(decision, ApprovalDecision::Timeout);
    }
}
