//! Discord impls of the four channel-ops trait families.
//!
//! Phase 7 / Q6. Wraps an `Arc<DiscordAdapter>` to reuse the resolved
//! bot token, `reqwest::Client`, and `api_base` override hook.
//!
//! Discord's REST surface is rich enough to map all four families
//! (`/users/@me/guilds`, `/guilds/{id}/channels`, `/users/{id}`,
//! `/channels/{id}/messages`, `/channels/{id}/threads`).

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
use crate::transport::discord::DiscordAdapter;

fn auth_header(adapter: &DiscordAdapter) -> String {
    format!("Bot {}", adapter.bot_token)
}

fn url(adapter: &DiscordAdapter, path: &str) -> String {
    format!("{}{}", adapter.api_base, path)
}

#[derive(Deserialize)]
struct DcChannel {
    id: String,
    #[serde(default, rename = "type")]
    kind: Option<u8>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct DcUser {
    id: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    global_name: Option<String>,
    #[serde(default)]
    bot: bool,
}

#[derive(Deserialize)]
struct DcMessage {
    id: String,
    channel_id: String,
}

#[derive(Deserialize)]
struct DcThread {
    id: String,
    #[serde(default)]
    name: Option<String>,
    parent_id: Option<String>,
    #[serde(default)]
    message_count: Option<u32>,
}

#[derive(Deserialize)]
struct DcThreadsResp {
    threads: Vec<DcThread>,
}

fn discord_channel_kind(t: Option<u8>) -> ChannelKind {
    // Discord channel types per
    // https://discord.com/developers/docs/resources/channel#channel-object-channel-types
    match t.unwrap_or(0) {
        0 => ChannelKind::Channel,            // GUILD_TEXT
        1 => ChannelKind::Dm,                 // DM
        2 => ChannelKind::Channel,            // GUILD_VOICE — treat as channel
        3 => ChannelKind::Group,              // GROUP_DM
        4 => ChannelKind::Channel,            // GUILD_CATEGORY
        10 | 11 | 12 => ChannelKind::Thread, // PUBLIC_THREAD / PRIVATE_THREAD / NEWS_THREAD
        _ => ChannelKind::Channel,
    }
}

// ────────────────────────────────────────────────────────────────────
// Directory
// ────────────────────────────────────────────────────────────────────

pub struct DiscordDirectory {
    inner: Arc<DiscordAdapter>,
}

impl DiscordDirectory {
    pub fn new(inner: Arc<DiscordAdapter>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ChannelDirectoryAdapter for DiscordDirectory {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "discord"
    }

    async fn list_channels(&self) -> Result<Vec<ChannelSummary>, ChannelOpError> {
        // For each allowlisted guild (or every guild the bot is in if
        // the allowlist is empty), GET /guilds/{id}/channels.
        let guild_ids: Vec<String> = if self.inner.config.guild_ids.is_empty() {
            // No allowlist — call /users/@me/guilds and use that set.
            let resp = self
                .inner
                .http
                .get(url(&self.inner, "/users/@me/guilds"))
                .header("Authorization", auth_header(&self.inner))
                .send()
                .await
                .map_err(|e| ChannelOpError::Http(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(ChannelOpError::Remote(format!(
                    "/users/@me/guilds HTTP {}",
                    resp.status()
                )));
            }
            #[derive(Deserialize)]
            struct G {
                id: String,
            }
            let gs: Vec<G> = resp
                .json()
                .await
                .map_err(|e| ChannelOpError::Http(e.to_string()))?;
            gs.into_iter().map(|g| g.id).collect()
        } else {
            self.inner.config.guild_ids.iter().map(|g| g.to_string()).collect()
        };

        let mut out: Vec<ChannelSummary> = Vec::new();
        for gid in guild_ids {
            let resp = self
                .inner
                .http
                .get(url(&self.inner, &format!("/guilds/{gid}/channels")))
                .header("Authorization", auth_header(&self.inner))
                .send()
                .await
                .map_err(|e| ChannelOpError::Http(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(ChannelOpError::Remote(format!(
                    "/guilds/{gid}/channels HTTP {}",
                    resp.status()
                )));
            }
            let chans: Vec<DcChannel> = resp
                .json()
                .await
                .map_err(|e| ChannelOpError::Http(e.to_string()))?;
            for c in chans {
                out.push(ChannelSummary {
                    id: c.id,
                    name: c.name,
                    kind: discord_channel_kind(c.kind),
                    is_member: true,
                });
            }
        }
        Ok(out)
    }

    async fn list_users(&self) -> Result<Vec<UserSummary>, ChannelOpError> {
        // Discord's GUILD_MEMBERS list endpoint requires the privileged
        // GUILD_MEMBERS intent + the `guilds.members.read` scope. The
        // adapter doesn't request that intent today, so we report
        // Unsupported with a clear remediation hint.
        Err(ChannelOpError::Unsupported {
            kind: "discord",
            op: "list_users",
            reason: "Discord guild member listing requires the privileged \
                     GUILD_MEMBERS intent — opt in via the developer portal \
                     and rebuild the adapter to request it"
                .into(),
        })
    }

    async fn lookup_user(
        &self,
        query: &str,
    ) -> Result<Option<UserSummary>, ChannelOpError> {
        // Discord user_ids are snowflake numeric strings. We don't
        // attempt to lookup by username since there's no public
        // `users.lookupByName` API.
        if !query.chars().all(|c| c.is_ascii_digit()) {
            return Err(ChannelOpError::InvalidInput(format!(
                "discord lookup_user query '{query}' is not a numeric snowflake"
            )));
        }
        let resp = self
            .inner
            .http
            .get(url(&self.inner, &format!("/users/{query}")))
            .header("Authorization", auth_header(&self.inner))
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(ChannelOpError::Remote(format!(
                "/users/{query} HTTP {status}"
            )));
        }
        let u: DcUser = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        Ok(Some(UserSummary {
            id: u.id,
            display_name: u.global_name,
            handle: u.username,
            is_bot: u.bot,
        }))
    }
}

// ────────────────────────────────────────────────────────────────────
// Messaging
// ────────────────────────────────────────────────────────────────────

pub struct DiscordMessaging {
    inner: Arc<DiscordAdapter>,
}

impl DiscordMessaging {
    pub fn new(inner: Arc<DiscordAdapter>) -> Self {
        Self { inner }
    }

    async fn open_dm(&self, user_id: &str) -> Result<String, ChannelOpError> {
        let resp = self
            .inner
            .http
            .post(url(&self.inner, "/users/@me/channels"))
            .header("Authorization", auth_header(&self.inner))
            .json(&serde_json::json!({ "recipient_id": user_id }))
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ChannelOpError::Remote(format!(
                "/users/@me/channels HTTP {}",
                resp.status()
            )));
        }
        let chan: DcChannel = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        Ok(chan.id)
    }

    async fn post_message(
        &self,
        channel_id: &str,
        text: &str,
    ) -> Result<MessageRef, ChannelOpError> {
        let resp = self
            .inner
            .http
            .post(url(
                &self.inner,
                &format!("/channels/{channel_id}/messages"),
            ))
            .header("Authorization", auth_header(&self.inner))
            .json(&serde_json::json!({ "content": text }))
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ChannelOpError::Remote(format!(
                "/channels/{channel_id}/messages HTTP {status}"
            )));
        }
        let msg: DcMessage = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        Ok(MessageRef {
            channel_id: msg.channel_id,
            message_id: msg.id,
        })
    }
}

#[async_trait]
impl ChannelMessagingAdapter for DiscordMessaging {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "discord"
    }

    async fn send_dm(&self, user_id: &str, text: &str) -> Result<MessageRef, ChannelOpError> {
        let dm = self.open_dm(user_id).await?;
        self.post_message(&dm, text).await
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

pub struct DiscordThreading {
    inner: Arc<DiscordAdapter>,
}

impl DiscordThreading {
    pub fn new(inner: Arc<DiscordAdapter>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ChannelThreadingAdapter for DiscordThreading {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "discord"
    }

    async fn create_thread(
        &self,
        parent: &ThreadParent,
        title: Option<&str>,
    ) -> Result<String, ChannelOpError> {
        // Discord supports two thread-creation endpoints:
        // - POST /channels/{id}/threads               (no parent message)
        // - POST /channels/{id}/messages/{mid}/threads (anchor on a message)
        let path = match parent {
            ThreadParent::Channel(c) => format!("/channels/{c}/threads"),
            ThreadParent::Message {
                channel_id,
                message_id,
            } => format!("/channels/{channel_id}/messages/{message_id}/threads"),
        };
        let body = serde_json::json!({
            "name": title.unwrap_or("thread"),
            // type=11 = PUBLIC_THREAD; safe default for both endpoints.
            "type": 11,
        });
        let resp = self
            .inner
            .http
            .post(url(&self.inner, &path))
            .header("Authorization", auth_header(&self.inner))
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ChannelOpError::Remote(format!("{path} HTTP {status}")));
        }
        let chan: DcChannel = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        Ok(chan.id)
    }

    async fn list_threads(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ThreadSummary>, ChannelOpError> {
        // GET /guilds/{guild_id}/threads/active is the documented
        // endpoint, but it requires guild_id. Without one, we fall
        // back to GET /channels/{id}/threads/archived/public which
        // returns archived threads anchored on the channel.
        let path = format!("/channels/{channel_id}/threads/archived/public");
        let resp = self
            .inner
            .http
            .get(url(&self.inner, &path))
            .header("Authorization", auth_header(&self.inner))
            .send()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ChannelOpError::Remote(format!("{path} HTTP {status}")));
        }
        let env: DcThreadsResp = resp
            .json()
            .await
            .map_err(|e| ChannelOpError::Http(e.to_string()))?;
        Ok(env
            .threads
            .into_iter()
            .map(|t| ThreadSummary {
                id: t.id,
                channel_id: t.parent_id.unwrap_or_else(|| channel_id.to_string()),
                title: t.name,
                message_count: t.message_count.unwrap_or(0),
            })
            .collect())
    }

    async fn follow_thread(&self, _thread_id: &str) -> Result<(), ChannelOpError> {
        // Discord auto-delivers thread events once the bot is in the
        // parent channel — local marker only.
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────
// Approval
// ────────────────────────────────────────────────────────────────────

pub struct DiscordApproval {
    inner: Arc<DiscordAdapter>,
    center: Arc<ApprovalCenter>,
    slot_id: String,
}

impl DiscordApproval {
    pub fn new(
        inner: Arc<DiscordAdapter>,
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
impl ChannelApprovalAdapter for DiscordApproval {
    fn transport_id(&self) -> &str {
        &self.inner.ctx.transport_id
    }
    fn transport_kind(&self) -> &'static str {
        "discord"
    }

    async fn request_approval(
        &self,
        channel_id: &str,
        prompt: &str,
        timeout_dur: Duration,
    ) -> Result<ApprovalDecision, ChannelOpError> {
        let key = ApprovalKey::new(&self.slot_id, self.transport_id(), channel_id);
        let rx = self.center.register(key.clone());

        let send = DiscordMessaging::new(self.inner.clone())
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
    use crate::transport::config::DiscordConfig;
    use crate::transport::TransportContext;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx() -> TransportContext {
        TransportContext {
            slot_id: "secretary".into(),
            transport_id: "discord-main".into(),
        }
    }

    fn cfg_with(guild_ids: Vec<u64>) -> DiscordConfig {
        DiscordConfig {
            message_content: false,
            guild_ids,
            channels: vec![],
            support_thread: false,
        }
    }

    fn adapter(api_base: String, guild_ids: Vec<u64>) -> Arc<DiscordAdapter> {
        Arc::new(DiscordAdapter::with_endpoints(
            ctx(),
            cfg_with(guild_ids),
            "BOTTOK".into(),
            vec!["U001".into()],
            api_base,
            "ws://unused".into(),
        ))
    }

    // ── Directory ─────────────────────────────────────────────

    #[tokio::test]
    async fn directory_list_channels_walks_allowlisted_guilds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/guilds/42/channels"))
            .and(header("authorization", "Bot BOTTOK"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": "C1", "type": 0, "name": "general" },
                { "id": "C2", "type": 11, "name": "thread-a" }
            ])))
            .mount(&server)
            .await;
        let dir = DiscordDirectory::new(adapter(server.uri(), vec![42]));
        let chans = dir.list_channels().await.unwrap();
        assert_eq!(chans.len(), 2);
        assert_eq!(chans[0].kind, ChannelKind::Channel);
        assert_eq!(chans[1].kind, ChannelKind::Thread);
    }

    #[tokio::test]
    async fn directory_list_channels_propagates_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/guilds/42/channels"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Missing Permissions"))
            .mount(&server)
            .await;
        let dir = DiscordDirectory::new(adapter(server.uri(), vec![42]));
        let err = dir.list_channels().await.unwrap_err();
        assert!(format!("{err}").contains("403"));
    }

    #[tokio::test]
    async fn directory_list_users_is_unsupported() {
        let dir = DiscordDirectory::new(adapter("http://unused".into(), vec![]));
        let err = dir.list_users().await.unwrap_err();
        assert!(matches!(err, ChannelOpError::Unsupported { op: "list_users", .. }));
    }

    #[tokio::test]
    async fn directory_lookup_user_returns_user_object() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/9000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "9000",
                "username": "alice",
                "global_name": "Alice"
            })))
            .mount(&server)
            .await;
        let dir = DiscordDirectory::new(adapter(server.uri(), vec![]));
        let u = dir.lookup_user("9000").await.unwrap().unwrap();
        assert_eq!(u.id, "9000");
        assert_eq!(u.handle.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn directory_lookup_user_returns_none_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/9999"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let dir = DiscordDirectory::new(adapter(server.uri(), vec![]));
        let u = dir.lookup_user("9999").await.unwrap();
        assert!(u.is_none());
    }

    #[tokio::test]
    async fn directory_lookup_user_rejects_non_numeric_query() {
        let dir = DiscordDirectory::new(adapter("http://unused".into(), vec![]));
        let err = dir.lookup_user("alice").await.unwrap_err();
        assert!(matches!(err, ChannelOpError::InvalidInput(_)));
    }

    // ── Messaging ─────────────────────────────────────────────

    #[tokio::test]
    async fn messaging_send_channel_returns_message_ref() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/C1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "M99",
                "channel_id": "C1"
            })))
            .mount(&server)
            .await;
        let m = DiscordMessaging::new(adapter(server.uri(), vec![]));
        let r = m.send_channel("C1", "hi").await.unwrap();
        assert_eq!(r.message_id, "M99");
        assert_eq!(r.channel_id, "C1");
    }

    #[tokio::test]
    async fn messaging_send_dm_opens_then_posts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/users/@me/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "DM-99", "type": 1
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/channels/DM-99/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "M1", "channel_id": "DM-99"
            })))
            .mount(&server)
            .await;
        let m = DiscordMessaging::new(adapter(server.uri(), vec![]));
        let r = m.send_dm("U001", "hi").await.unwrap();
        assert_eq!(r.channel_id, "DM-99");
        assert_eq!(r.message_id, "M1");
    }

    #[tokio::test]
    async fn messaging_send_channel_propagates_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/Cnope/messages"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Missing Access"))
            .mount(&server)
            .await;
        let m = DiscordMessaging::new(adapter(server.uri(), vec![]));
        let err = m.send_channel("Cnope", "x").await.unwrap_err();
        assert!(format!("{err}").contains("403"));
    }

    // ── Threading ─────────────────────────────────────────────

    #[tokio::test]
    async fn threading_create_thread_anchors_in_channel() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/C1/threads"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "T-1", "type": 11, "name": "topic"
            })))
            .mount(&server)
            .await;
        let t = DiscordThreading::new(adapter(server.uri(), vec![]));
        let id = t
            .create_thread(&ThreadParent::Channel("C1".into()), Some("topic"))
            .await
            .unwrap();
        assert_eq!(id, "T-1");
    }

    #[tokio::test]
    async fn threading_create_thread_anchors_on_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/C1/messages/M1/threads"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "T-2", "type": 11, "name": "from-msg"
            })))
            .mount(&server)
            .await;
        let t = DiscordThreading::new(adapter(server.uri(), vec![]));
        let id = t
            .create_thread(
                &ThreadParent::Message {
                    channel_id: "C1".into(),
                    message_id: "M1".into(),
                },
                Some("from-msg"),
            )
            .await
            .unwrap();
        assert_eq!(id, "T-2");
    }

    #[tokio::test]
    async fn threading_list_threads_decodes_archived_public_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/channels/C1/threads/archived/public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "threads": [
                    { "id": "T-1", "name": "a", "parent_id": "C1", "message_count": 5 },
                    { "id": "T-2", "name": null, "parent_id": "C1", "message_count": 0 }
                ]
            })))
            .mount(&server)
            .await;
        let t = DiscordThreading::new(adapter(server.uri(), vec![]));
        let threads = t.list_threads("C1").await.unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].message_count, 5);
        assert_eq!(threads[1].title, None);
    }

    #[tokio::test]
    async fn threading_create_thread_propagates_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/C1/threads"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Missing Permissions"))
            .mount(&server)
            .await;
        let t = DiscordThreading::new(adapter(server.uri(), vec![]));
        let err = t
            .create_thread(&ThreadParent::Channel("C1".into()), Some("x"))
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("403"));
    }

    // ── Approval ──────────────────────────────────────────────

    #[tokio::test]
    async fn approval_resolves_when_inbound_completes_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/C1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "M1", "channel_id": "C1"
            })))
            .mount(&server)
            .await;
        let center = Arc::new(ApprovalCenter::new());
        let approval = DiscordApproval::new(adapter(server.uri(), vec![]), center.clone(), "secretary");

        let key = ApprovalKey::new("secretary", "discord-main", "C1");
        let center_clone = center.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            center_clone.try_resolve(
                &key,
                ApprovalDecision::Approved {
                    actor_id: "U001".into(),
                    at: std::time::SystemTime::now(),
                },
            );
        });

        let decision = approval
            .request_approval("C1", "ok?", Duration::from_secs(2))
            .await
            .unwrap();
        assert!(matches!(decision, ApprovalDecision::Approved { .. }));
    }

    #[tokio::test]
    async fn approval_returns_error_when_send_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/Cnope/messages"))
            .respond_with(ResponseTemplate::new(403).set_body_string("denied"))
            .mount(&server)
            .await;
        let center = Arc::new(ApprovalCenter::new());
        let approval = DiscordApproval::new(adapter(server.uri(), vec![]), center.clone(), "secretary");
        let err = approval
            .request_approval("Cnope", "x", Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("403"));
        assert_eq!(center.pending_len(), 0);
    }
}
