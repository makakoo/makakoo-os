//! Slack impls of the four channel-ops trait families.
//!
//! Phase 6 / Q6. Wraps an `Arc<SlackAdapter>` to reuse the resolved
//! bot token, `reqwest::Client`, and `api_base` override hook.
//!
//! Slack's Web API is rich enough to map all four families natively
//! (`conversations.list`, `users.list`, `users.info`,
//! `chat.postMessage`, `conversations.replies`).

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
use crate::transport::slack::SlackAdapter;

#[derive(Deserialize)]
struct SlackEnvelope {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    channels: Vec<SlackChannel>,
    #[serde(default)]
    members: Vec<SlackUser>,
    #[serde(default)]
    user: Option<SlackUser>,
    #[serde(default)]
    channel: Option<SlackChannel>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    messages: Vec<SlackThreadMessage>,
}

#[derive(Deserialize)]
struct SlackChannel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    is_im: bool,
    #[serde(default)]
    is_group: bool,
    #[serde(default)]
    is_channel: bool,
    #[serde(default)]
    is_member: bool,
}

impl SlackChannel {
    fn into_summary(self) -> ChannelSummary {
        let kind = if self.is_im {
            ChannelKind::Dm
        } else if self.is_group {
            ChannelKind::Group
        } else if self.is_channel {
            ChannelKind::Channel
        } else {
            ChannelKind::Channel
        };
        ChannelSummary {
            id: self.id,
            name: self.name,
            kind,
            is_member: self.is_member,
        }
    }
}

#[derive(Deserialize)]
struct SlackUser {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    real_name: Option<String>,
    #[serde(default)]
    is_bot: bool,
}

impl SlackUser {
    fn into_summary(self) -> UserSummary {
        UserSummary {
            id: self.id,
            display_name: self.real_name,
            handle: self.name,
            is_bot: self.is_bot,
        }
    }
}

#[derive(Deserialize)]
struct SlackThreadMessage {
    #[serde(default)]
    thread_ts: Option<String>,
    ts: String,
    #[serde(default)]
    reply_count: Option<u32>,
}

fn slack_url(adapter: &SlackAdapter, method: &str) -> String {
    format!("{}/{}", adapter.api_base, method)
}

// ────────────────────────────────────────────────────────────────────
// Directory
// ────────────────────────────────────────────────────────────────────

pub struct SlackDirectory {
    inner: Arc<SlackAdapter>,
}

impl SlackDirectory {
    pub fn new(inner: Arc<SlackAdapter>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ChannelDirectoryAdapter for SlackDirectory {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "slack"
    }

    async fn list_channels(&self) -> Result<Vec<ChannelSummary>, ChannelOpError> {
        let resp = self
            .inner
            .http
            .get(slack_url(&self.inner, "conversations.list"))
            .bearer_auth(&self.inner.bot_token)
            .query(&[
                ("types", "public_channel,private_channel,im,mpim"),
                ("limit", "200"),
            ])
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let env: SlackEnvelope = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            return Err(ChannelOpError::Remote(
                env.error.unwrap_or_else(|| "unknown".into()),
            ));
        }
        Ok(env.channels.into_iter().map(|c| c.into_summary()).collect())
    }

    async fn list_users(&self) -> Result<Vec<UserSummary>, ChannelOpError> {
        let resp = self
            .inner
            .http
            .get(slack_url(&self.inner, "users.list"))
            .bearer_auth(&self.inner.bot_token)
            .query(&[("limit", "200")])
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let env: SlackEnvelope = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            return Err(ChannelOpError::Remote(
                env.error.unwrap_or_else(|| "unknown".into()),
            ));
        }
        Ok(env.members.into_iter().map(|u| u.into_summary()).collect())
    }

    async fn lookup_user(
        &self,
        query: &str,
    ) -> Result<Option<UserSummary>, ChannelOpError> {
        // Two-step: if query starts with "U" treat as user id and call
        // users.info; otherwise treat as email and call
        // users.lookupByEmail.
        let (endpoint, key, val) = if query.starts_with('U') || query.starts_with('W') {
            ("users.info", "user", query)
        } else if query.contains('@') {
            ("users.lookupByEmail", "email", query)
        } else {
            return Err(ChannelOpError::InvalidInput(format!(
                "slack lookup_user query '{query}' is neither a Slack user_id (U…) nor an email"
            )));
        };
        let resp = self
            .inner
            .http
            .get(slack_url(&self.inner, endpoint))
            .bearer_auth(&self.inner.bot_token)
            .query(&[(key, val)])
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let env: SlackEnvelope = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            // Slack returns "users_not_found" for missing users —
            // map to Ok(None).
            let err = env.error.unwrap_or_default();
            if err == "users_not_found" || err == "user_not_found" {
                return Ok(None);
            }
            return Err(ChannelOpError::Remote(err));
        }
        Ok(env.user.map(|u| u.into_summary()))
    }
}

// ────────────────────────────────────────────────────────────────────
// Messaging
// ────────────────────────────────────────────────────────────────────

pub struct SlackMessaging {
    inner: Arc<SlackAdapter>,
}

impl SlackMessaging {
    pub fn new(inner: Arc<SlackAdapter>) -> Self {
        Self { inner }
    }

    async fn open_dm(&self, user_id: &str) -> Result<String, ChannelOpError> {
        let resp = self
            .inner
            .http
            .post(slack_url(&self.inner, "conversations.open"))
            .bearer_auth(&self.inner.bot_token)
            .json(&serde_json::json!({ "users": user_id }))
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let env: SlackEnvelope = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            return Err(ChannelOpError::Remote(
                env.error.unwrap_or_else(|| "unknown".into()),
            ));
        }
        env.channel
            .map(|c| c.id)
            .ok_or_else(|| ChannelOpError::Remote("conversations.open missing channel".into()))
    }

    async fn post_message(
        &self,
        channel_id: &str,
        text: &str,
    ) -> Result<MessageRef, ChannelOpError> {
        let resp = self
            .inner
            .http
            .post(slack_url(&self.inner, "chat.postMessage"))
            .bearer_auth(&self.inner.bot_token)
            .json(&serde_json::json!({
                "channel": channel_id,
                "text": text,
            }))
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let env: SlackEnvelope = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            return Err(ChannelOpError::Remote(
                env.error.unwrap_or_else(|| "unknown".into()),
            ));
        }
        let ts = env
            .ts
            .ok_or_else(|| ChannelOpError::Remote("chat.postMessage missing ts".into()))?;
        Ok(MessageRef {
            channel_id: channel_id.to_string(),
            message_id: ts,
        })
    }
}

#[async_trait]
impl ChannelMessagingAdapter for SlackMessaging {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "slack"
    }

    async fn send_dm(&self, user_id: &str, text: &str) -> Result<MessageRef, ChannelOpError> {
        let dm_channel = self.open_dm(user_id).await?;
        self.post_message(&dm_channel, text).await
    }

    async fn send_channel(
        &self,
        channel_id: &str,
        text: &str,
    ) -> Result<MessageRef, ChannelOpError> {
        self.post_message(channel_id, text).await
    }

    async fn broadcast(
        &self,
        channel_ids: &[String],
        text: &str,
    ) -> Vec<BroadcastResult> {
        let mut out = Vec::with_capacity(channel_ids.len());
        for cid in channel_ids {
            let outcome = self
                .post_message(cid, text)
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

pub struct SlackThreading {
    inner: Arc<SlackAdapter>,
}

impl SlackThreading {
    pub fn new(inner: Arc<SlackAdapter>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ChannelThreadingAdapter for SlackThreading {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "slack"
    }

    async fn create_thread(
        &self,
        parent: &ThreadParent,
        title: Option<&str>,
    ) -> Result<String, ChannelOpError> {
        // Slack threads are anchored to a parent message. There's no
        // separate "create thread" call — we post the parent and return
        // its `ts` (which becomes the future thread_ts).
        let (channel, parent_ts) = match parent {
            ThreadParent::Message {
                channel_id,
                message_id,
            } => (channel_id.clone(), Some(message_id.clone())),
            ThreadParent::Channel(c) => (c.clone(), None),
        };
        if let Some(ts) = parent_ts {
            // Caller already has a parent message — just return the ts;
            // Slack will treat any reply with `thread_ts = ts` as a
            // thread on that message.
            return Ok(ts);
        }
        // Anchor with a stub parent message titled accordingly.
        let body = serde_json::json!({
            "channel": channel,
            "text": title.unwrap_or("(thread)"),
        });
        let resp = self
            .inner
            .http
            .post(slack_url(&self.inner, "chat.postMessage"))
            .bearer_auth(&self.inner.bot_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let env: SlackEnvelope = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            return Err(ChannelOpError::Remote(
                env.error.unwrap_or_else(|| "unknown".into()),
            ));
        }
        env.ts
            .ok_or_else(|| ChannelOpError::Remote("chat.postMessage missing ts".into()))
    }

    async fn list_threads(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ThreadSummary>, ChannelOpError> {
        // Slack has no top-level "list threads in channel" API. Best
        // we can do without scanning history page-by-page is read the
        // channel's recent history, then filter for messages whose
        // `thread_ts == ts` (parents) with `reply_count > 0`. That
        // requires `conversations.history` — implemented as a single
        // page (Slack default limit 100) for cost reasons.
        let resp = self
            .inner
            .http
            .get(slack_url(&self.inner, "conversations.history"))
            .bearer_auth(&self.inner.bot_token)
            .query(&[("channel", channel_id), ("limit", "100")])
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let env: SlackEnvelope = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !env.ok {
            return Err(ChannelOpError::Remote(
                env.error.unwrap_or_else(|| "unknown".into()),
            ));
        }
        let out = env
            .messages
            .into_iter()
            .filter_map(|m| {
                let count = m.reply_count.unwrap_or(0);
                let parent = m.thread_ts.as_deref() == Some(m.ts.as_str());
                if parent && count > 0 {
                    Some(ThreadSummary {
                        id: m.ts.clone(),
                        channel_id: channel_id.to_string(),
                        title: None,
                        message_count: count,
                    })
                } else {
                    None
                }
            })
            .collect();
        Ok(out)
    }

    async fn follow_thread(&self, _thread_id: &str) -> Result<(), ChannelOpError> {
        // Slack has no programmatic "follow thread" primitive — bots
        // receive replies via the events stream once they're posted in
        // a watched channel. Treat as a local no-op.
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────
// Approval
// ────────────────────────────────────────────────────────────────────

pub struct SlackApproval {
    inner: Arc<SlackAdapter>,
    center: Arc<ApprovalCenter>,
    slot_id: String,
}

impl SlackApproval {
    pub fn new(
        inner: Arc<SlackAdapter>,
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
impl ChannelApprovalAdapter for SlackApproval {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "slack"
    }

    async fn request_approval(
        &self,
        channel_id: &str,
        prompt: &str,
        timeout_dur: Duration,
    ) -> Result<ApprovalDecision, ChannelOpError> {
        let key = ApprovalKey::new(&self.slot_id, self.transport_id(), channel_id);
        let rx = self.center.register(key.clone());

        let send = SlackMessaging::new(self.inner.clone())
            .send_channel(channel_id, &format!("{prompt}\n\nReply: yes / no"))
            .await;
        if let Err(e) = send {
            self.center.drop_pending(&key);
            return Err(e);
        }

        match timeout(timeout_dur, rx).await {
            Ok(Ok(decision)) => Ok(decision),
            Ok(Err(_)) => {
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
    use crate::transport::config::SlackConfig;
    use crate::transport::TransportContext;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "slack-main".into(),
        }
    }

    fn cfg() -> SlackConfig {
        SlackConfig {
            team_id: "T0123".into(),
            mode: "socket".into(),
            dm_only: false,
            channels: vec!["C1".into()],
            support_thread: false,
        }
    }

    fn adapter(api_base: String) -> Arc<SlackAdapter> {
        Arc::new(SlackAdapter::with_api_base(
            ctx(),
            cfg(),
            "xoxb-bot".into(),
            "xapp-1".into(),
            vec!["U001".into()],
            api_base,
        ))
    }

    // ── Directory ─────────────────────────────────────────────

    #[tokio::test]
    async fn directory_list_channels_decodes_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "channels": [
                    { "id": "C1", "name": "general", "is_channel": true, "is_member": true },
                    { "id": "D1", "is_im": true, "is_member": true }
                ]
            })))
            .mount(&server)
            .await;
        let dir = SlackDirectory::new(adapter(server.uri()));
        let chans = dir.list_channels().await.unwrap();
        assert_eq!(chans.len(), 2);
        assert_eq!(chans[0].kind, ChannelKind::Channel);
        assert_eq!(chans[1].kind, ChannelKind::Dm);
    }

    #[tokio::test]
    async fn directory_list_channels_propagates_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "missing_scope"
            })))
            .mount(&server)
            .await;
        let dir = SlackDirectory::new(adapter(server.uri()));
        let err = dir.list_channels().await.unwrap_err();
        assert!(format!("{err}").contains("missing_scope"));
    }

    #[tokio::test]
    async fn directory_list_users_decodes_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "members": [
                    { "id": "U001", "name": "alice", "real_name": "Alice", "is_bot": false }
                ]
            })))
            .mount(&server)
            .await;
        let dir = SlackDirectory::new(adapter(server.uri()));
        let users = dir.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].handle.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn directory_lookup_user_returns_none_on_users_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.lookupByEmail"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "users_not_found"
            })))
            .mount(&server)
            .await;
        let dir = SlackDirectory::new(adapter(server.uri()));
        let u = dir.lookup_user("nope@x.com").await.unwrap();
        assert!(u.is_none());
    }

    // ── Messaging ─────────────────────────────────────────────

    #[tokio::test]
    async fn messaging_send_channel_returns_message_ref() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "ts": "1700000000.000100",
            })))
            .mount(&server)
            .await;
        let m = SlackMessaging::new(adapter(server.uri()));
        let r = m.send_channel("C1", "hi").await.unwrap();
        assert_eq!(r.message_id, "1700000000.000100");
        assert_eq!(r.channel_id, "C1");
    }

    #[tokio::test]
    async fn messaging_send_dm_opens_then_posts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "channel": { "id": "D9" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "ts": "1700000000.000200"
            })))
            .mount(&server)
            .await;
        let m = SlackMessaging::new(adapter(server.uri()));
        let r = m.send_dm("U001", "hi").await.unwrap();
        assert_eq!(r.channel_id, "D9");
        assert_eq!(r.message_id, "1700000000.000200");
    }

    #[tokio::test]
    async fn messaging_send_channel_propagates_channel_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "channel_not_found"
            })))
            .mount(&server)
            .await;
        let m = SlackMessaging::new(adapter(server.uri()));
        let err = m.send_channel("Cnope", "x").await.unwrap_err();
        assert!(format!("{err}").contains("channel_not_found"));
    }

    // ── Threading ─────────────────────────────────────────────

    #[tokio::test]
    async fn threading_create_thread_from_message_returns_parent_ts() {
        let t = SlackThreading::new(adapter("http://unused.invalid".into()));
        let id = t
            .create_thread(
                &ThreadParent::Message {
                    channel_id: "C1".into(),
                    message_id: "1700000000.000100".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(id, "1700000000.000100");
    }

    #[tokio::test]
    async fn threading_create_thread_anchors_parent_in_channel() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "ts": "1700000000.000300",
            })))
            .mount(&server)
            .await;
        let t = SlackThreading::new(adapter(server.uri()));
        let id = t
            .create_thread(&ThreadParent::Channel("C1".into()), Some("planning"))
            .await
            .unwrap();
        assert_eq!(id, "1700000000.000300");
    }

    #[tokio::test]
    async fn threading_list_threads_filters_to_parents_with_replies() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.history"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "messages": [
                    {
                        "ts": "1.1",
                        "thread_ts": "1.1",
                        "reply_count": 3
                    },
                    {
                        "ts": "1.2",
                        "thread_ts": null,
                        "reply_count": 0
                    }
                ]
            })))
            .mount(&server)
            .await;
        let t = SlackThreading::new(adapter(server.uri()));
        let threads = t.list_threads("C1").await.unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "1.1");
        assert_eq!(threads[0].message_count, 3);
    }

    #[tokio::test]
    async fn threading_create_thread_propagates_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "not_in_channel"
            })))
            .mount(&server)
            .await;
        let t = SlackThreading::new(adapter(server.uri()));
        let err = t
            .create_thread(&ThreadParent::Channel("Cnope".into()), None)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("not_in_channel"));
    }

    // ── Approval ──────────────────────────────────────────────

    #[tokio::test]
    async fn approval_resolves_when_inbound_completes_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "ts": "1.1"
            })))
            .mount(&server)
            .await;
        let center = Arc::new(ApprovalCenter::new());
        let approval =
            SlackApproval::new(adapter(server.uri()), center.clone(), "secretary");

        let key = ApprovalKey::new("secretary", "slack-main", "C1");
        let center_clone = center.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            center_clone.try_resolve(
                &key,
                ApprovalDecision::Denied {
                    actor_id: "U001".into(),
                    at: std::time::SystemTime::now(),
                    reason: Some("not yet".into()),
                },
            );
        });

        let decision = approval
            .request_approval("C1", "ok?", Duration::from_secs(2))
            .await
            .unwrap();
        match decision {
            ApprovalDecision::Denied { reason, .. } => {
                assert_eq!(reason.as_deref(), Some("not yet"));
            }
            d => panic!("expected Denied, got {d:?}"),
        }
    }

    #[tokio::test]
    async fn approval_returns_timeout_error_propagation_path() {
        // If sending the prompt fails, the entry should be dropped
        // and the error returned (not silently swallowed).
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "channel_not_found"
            })))
            .mount(&server)
            .await;
        let center = Arc::new(ApprovalCenter::new());
        let approval = SlackApproval::new(adapter(server.uri()), center.clone(), "secretary");
        let err = approval
            .request_approval("Cnope", "x", Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("channel_not_found"));
        // Pending entry must be cleaned up after the failed send.
        assert_eq!(center.pending_len(), 0);
    }
}
