//! Tier-B channel-ops handlers.
//!
//! Phase 6 / Q6 — MCP tool surface over the four trait families in
//! `makakoo_core::channel_ops`. Tools accept `slot_id` + `transport_id`
//! plus per-op params; the slot's allowlist is enforced by the
//! [`ChannelOpsRegistry`] (cross-slot lookup returns None, which we
//! map to `RpcError::invalid_params` so the LLM gets a clear "this
//! slot has no such transport" rather than a silent miss).
//!
//! Tools shipped (MCP naming convention requires lowercase
//! alphanumeric + underscores — no dots):
//! - `channel_directory_list_channels`
//! - `channel_directory_list_users`
//! - `channel_directory_lookup_user`
//! - `channel_messaging_send_dm`
//! - `channel_messaging_send_channel`
//! - `channel_messaging_broadcast`
//! - `channel_threading_create_thread`
//! - `channel_threading_list_threads`
//! - `channel_threading_follow_thread`
//! - `channel_approval_request`

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

use makakoo_core::channel_ops::{
    ApprovalDecision, ChannelKind, ChannelOpError, ChannelOpsRegistry, ChannelSummary,
    MessageRef, ThreadParent, ThreadSummary, UserSummary,
};

const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 300;

// ────────────────────────────────────────────────────────────────────
// shared helpers
// ────────────────────────────────────────────────────────────────────

fn registry(ctx: &ToolContext) -> Result<&Arc<ChannelOpsRegistry>, RpcError> {
    ctx.channel_ops
        .as_ref()
        .ok_or_else(|| RpcError::internal("channel_ops registry not wired"))
}

fn require_str<'a>(p: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params(format!("missing '{key}'")))
}

fn require_str_array(p: &Value, key: &str) -> Result<Vec<String>, RpcError> {
    let arr = p
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::invalid_params(format!("missing '{key}' (array)")))?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let s = v.as_str().ok_or_else(|| {
            RpcError::invalid_params(format!("'{key}' must contain only strings"))
        })?;
        out.push(s.to_string());
    }
    Ok(out)
}

fn op_err_to_rpc(e: ChannelOpError) -> RpcError {
    match e {
        ChannelOpError::InvalidInput(m) => RpcError::invalid_params(m),
        other => RpcError::internal(other.to_string()),
    }
}

fn channel_kind_str(k: &ChannelKind) -> &'static str {
    match k {
        ChannelKind::Dm => "dm",
        ChannelKind::Channel => "channel",
        ChannelKind::Group => "group",
        ChannelKind::Thread => "thread",
    }
}

fn channel_summary_json(c: &ChannelSummary) -> Value {
    json!({
        "id": c.id,
        "name": c.name,
        "kind": channel_kind_str(&c.kind),
        "is_member": c.is_member,
    })
}

fn user_summary_json(u: &UserSummary) -> Value {
    json!({
        "id": u.id,
        "display_name": u.display_name,
        "handle": u.handle,
        "is_bot": u.is_bot,
    })
}

fn message_ref_json(m: &MessageRef) -> Value {
    json!({
        "channel_id": m.channel_id,
        "message_id": m.message_id,
    })
}

fn thread_summary_json(t: &ThreadSummary) -> Value {
    json!({
        "id": t.id,
        "channel_id": t.channel_id,
        "title": t.title,
        "message_count": t.message_count,
    })
}

fn approval_decision_json(d: &ApprovalDecision) -> Value {
    match d {
        ApprovalDecision::Approved { actor_id, .. } => {
            json!({ "outcome": "approved", "actor_id": actor_id })
        }
        ApprovalDecision::Denied {
            actor_id, reason, ..
        } => json!({
            "outcome": "denied",
            "actor_id": actor_id,
            "reason": reason,
        }),
        ApprovalDecision::Timeout => json!({ "outcome": "timeout" }),
    }
}

fn slot_target_schema(extra: Value) -> Value {
    let mut props = json!({
        "slot_id": { "type": "string" },
        "transport_id": { "type": "string" },
    });
    if let (Some(props_obj), Some(extra_obj)) = (props.as_object_mut(), extra.as_object()) {
        for (k, v) in extra_obj {
            props_obj.insert(k.clone(), v.clone());
        }
    }
    json!({
        "type": "object",
        "properties": props,
        "required": ["slot_id", "transport_id"],
    })
}

fn unknown_transport(slot: &str, transport: &str, family: &str) -> RpcError {
    RpcError::invalid_params(format!(
        "no '{family}' adapter registered for slot='{slot}' transport='{transport}'"
    ))
}

// ────────────────────────────────────────────────────────────────────
// channel_directory.list_channels
// ────────────────────────────────────────────────────────────────────

pub struct ChannelDirectoryListChannelsHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelDirectoryListChannelsHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelDirectoryListChannelsHandler {
    fn name(&self) -> &str {
        "channel_directory_list_channels"
    }
    fn description(&self) -> &str {
        "List channels visible to a slot's transport. Telegram returns the configured \
         allowlist enriched via getChat; Slack returns conversations.list."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({}))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let adapter = reg
            .lookup_directory(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "directory"))?;
        let chans = adapter.list_channels().await.map_err(op_err_to_rpc)?;
        Ok(json!({
            "channels": chans.iter().map(channel_summary_json).collect::<Vec<_>>(),
        }))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_directory.list_users
// ────────────────────────────────────────────────────────────────────

pub struct ChannelDirectoryListUsersHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelDirectoryListUsersHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelDirectoryListUsersHandler {
    fn name(&self) -> &str {
        "channel_directory_list_users"
    }
    fn description(&self) -> &str {
        "List users visible to a slot's transport. Telegram returns Unsupported \
         (bots cannot enumerate users); Slack returns users.list."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({}))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let adapter = reg
            .lookup_directory(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "directory"))?;
        let users = adapter.list_users().await.map_err(op_err_to_rpc)?;
        Ok(json!({
            "users": users.iter().map(user_summary_json).collect::<Vec<_>>(),
        }))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_directory.lookup_user
// ────────────────────────────────────────────────────────────────────

pub struct ChannelDirectoryLookupUserHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelDirectoryLookupUserHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelDirectoryLookupUserHandler {
    fn name(&self) -> &str {
        "channel_directory_lookup_user"
    }
    fn description(&self) -> &str {
        "Resolve a single user by id, handle, or email. Returns null when the \
         user does not exist."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({
            "query": { "type": "string" },
        }))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let query = require_str(&params, "query")?;
        let adapter = reg
            .lookup_directory(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "directory"))?;
        let user = adapter.lookup_user(query).await.map_err(op_err_to_rpc)?;
        Ok(json!({
            "user": user.as_ref().map(user_summary_json),
        }))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_messaging.send_dm
// ────────────────────────────────────────────────────────────────────

pub struct ChannelMessagingSendDmHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelMessagingSendDmHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelMessagingSendDmHandler {
    fn name(&self) -> &str {
        "channel_messaging_send_dm"
    }
    fn description(&self) -> &str {
        "Send a direct message to a user via the slot's transport."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({
            "user_id": { "type": "string" },
            "text": { "type": "string" },
        }))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let user_id = require_str(&params, "user_id")?;
        let text = require_str(&params, "text")?;
        let adapter = reg
            .lookup_messaging(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "messaging"))?;
        let r = adapter
            .send_dm(user_id, text)
            .await
            .map_err(op_err_to_rpc)?;
        Ok(message_ref_json(&r))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_messaging.send_channel
// ────────────────────────────────────────────────────────────────────

pub struct ChannelMessagingSendChannelHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelMessagingSendChannelHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelMessagingSendChannelHandler {
    fn name(&self) -> &str {
        "channel_messaging_send_channel"
    }
    fn description(&self) -> &str {
        "Send a message to a channel (public/private/group) via the slot's transport."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({
            "channel_id": { "type": "string" },
            "text": { "type": "string" },
        }))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let channel_id = require_str(&params, "channel_id")?;
        let text = require_str(&params, "text")?;
        let adapter = reg
            .lookup_messaging(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "messaging"))?;
        let r = adapter
            .send_channel(channel_id, text)
            .await
            .map_err(op_err_to_rpc)?;
        Ok(message_ref_json(&r))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_messaging.broadcast
// ────────────────────────────────────────────────────────────────────

pub struct ChannelMessagingBroadcastHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelMessagingBroadcastHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelMessagingBroadcastHandler {
    fn name(&self) -> &str {
        "channel_messaging_broadcast"
    }
    fn description(&self) -> &str {
        "Send the same message to multiple channels. Returns one outcome per channel; \
         partial failures are captured per-entry rather than aborting the call."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({
            "channel_ids": { "type": "array", "items": { "type": "string" } },
            "text": { "type": "string" },
        }))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let channel_ids = require_str_array(&params, "channel_ids")?;
        let text = require_str(&params, "text")?;
        let adapter = reg
            .lookup_messaging(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "messaging"))?;
        let outcomes = adapter.broadcast(&channel_ids, text).await;
        let arr: Vec<Value> = outcomes
            .iter()
            .map(|r| match &r.outcome {
                Ok(m) => json!({
                    "channel_id": r.channel_id,
                    "ok": true,
                    "message": message_ref_json(m),
                }),
                Err(e) => json!({
                    "channel_id": r.channel_id,
                    "ok": false,
                    "error": e,
                }),
            })
            .collect();
        Ok(json!({ "results": arr }))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_threading.create_thread
// ────────────────────────────────────────────────────────────────────

pub struct ChannelThreadingCreateThreadHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelThreadingCreateThreadHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

fn parse_thread_parent(p: &Value) -> Result<ThreadParent, RpcError> {
    let parent = p
        .get("parent")
        .ok_or_else(|| RpcError::invalid_params("missing 'parent'"))?;
    let kind = parent
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("'parent.kind' must be 'channel' or 'message'"))?;
    match kind {
        "channel" => {
            let cid = parent
                .get("channel_id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing 'parent.channel_id'"))?;
            Ok(ThreadParent::Channel(cid.into()))
        }
        "message" => {
            let cid = parent
                .get("channel_id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing 'parent.channel_id'"))?;
            let mid = parent
                .get("message_id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing 'parent.message_id'"))?;
            Ok(ThreadParent::Message {
                channel_id: cid.into(),
                message_id: mid.into(),
            })
        }
        other => Err(RpcError::invalid_params(format!(
            "unknown parent.kind '{other}' (expected 'channel' or 'message')"
        ))),
    }
}

#[async_trait]
impl ToolHandler for ChannelThreadingCreateThreadHandler {
    fn name(&self) -> &str {
        "channel_threading_create_thread"
    }
    fn description(&self) -> &str {
        "Create a thread anchored to a channel or message. Returns the new thread id."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({
            "parent": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["channel", "message"] },
                    "channel_id": { "type": "string" },
                    "message_id": { "type": "string" }
                },
                "required": ["kind", "channel_id"]
            },
            "title": { "type": "string" }
        }))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let parent = parse_thread_parent(&params)?;
        let title = params.get("title").and_then(Value::as_str);
        let adapter = reg
            .lookup_threading(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "threading"))?;
        let id = adapter
            .create_thread(&parent, title)
            .await
            .map_err(op_err_to_rpc)?;
        Ok(json!({ "thread_id": id }))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_threading.list_threads
// ────────────────────────────────────────────────────────────────────

pub struct ChannelThreadingListThreadsHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelThreadingListThreadsHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelThreadingListThreadsHandler {
    fn name(&self) -> &str {
        "channel_threading_list_threads"
    }
    fn description(&self) -> &str {
        "List threads anchored on the given channel. May return Unsupported \
         on transports without a listing API."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({
            "channel_id": { "type": "string" },
        }))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let channel_id = require_str(&params, "channel_id")?;
        let adapter = reg
            .lookup_threading(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "threading"))?;
        let threads = adapter
            .list_threads(channel_id)
            .await
            .map_err(op_err_to_rpc)?;
        Ok(json!({
            "threads": threads.iter().map(thread_summary_json).collect::<Vec<_>>(),
        }))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_threading.follow_thread
// ────────────────────────────────────────────────────────────────────

pub struct ChannelThreadingFollowThreadHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelThreadingFollowThreadHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelThreadingFollowThreadHandler {
    fn name(&self) -> &str {
        "channel_threading_follow_thread"
    }
    fn description(&self) -> &str {
        "Mark a thread as actively watched. For transports without a remote \
         subscription concept this is a local marker."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({
            "thread_id": { "type": "string" },
        }))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let thread_id = require_str(&params, "thread_id")?;
        let adapter = reg
            .lookup_threading(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "threading"))?;
        adapter
            .follow_thread(thread_id)
            .await
            .map_err(op_err_to_rpc)?;
        Ok(json!({ "ok": true }))
    }
}

// ────────────────────────────────────────────────────────────────────
// channel_approval.request
// ────────────────────────────────────────────────────────────────────

pub struct ChannelApprovalRequestHandler {
    ctx: Arc<ToolContext>,
}

impl ChannelApprovalRequestHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ChannelApprovalRequestHandler {
    fn name(&self) -> &str {
        "channel_approval_request"
    }
    fn description(&self) -> &str {
        "Send a yes/no approval prompt to a channel and block until the user \
         replies or the timeout elapses."
    }
    fn input_schema(&self) -> Value {
        slot_target_schema(json!({
            "channel_id": { "type": "string" },
            "prompt": { "type": "string" },
            "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 86400 }
        }))
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let reg = registry(&self.ctx)?;
        let slot = require_str(&params, "slot_id")?;
        let transport = require_str(&params, "transport_id")?;
        let channel_id = require_str(&params, "channel_id")?;
        let prompt = require_str(&params, "prompt")?;
        let timeout_secs = params
            .get("timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_APPROVAL_TIMEOUT_SECS);
        let adapter = reg
            .lookup_approval(slot, transport)
            .await
            .ok_or_else(|| unknown_transport(slot, transport, "approval"))?;
        let dec = adapter
            .request_approval(channel_id, prompt, Duration::from_secs(timeout_secs))
            .await
            .map_err(op_err_to_rpc)?;
        Ok(approval_decision_json(&dec))
    }
}

// ────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use makakoo_core::channel_ops::{
        ApprovalDecision, ApprovalKey, BroadcastResult, ChannelApprovalAdapter,
        ChannelDirectoryAdapter, ChannelKind, ChannelMessagingAdapter,
        ChannelOpError, ChannelSummary, MessageRef, ThreadParent, ThreadSummary,
        ChannelThreadingAdapter, UserSummary,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    struct CannedDir {
        transport_id: String,
        channels: Vec<ChannelSummary>,
    }
    #[async_trait]
    impl ChannelDirectoryAdapter for CannedDir {
        fn transport_id(&self) -> &str {
            &self.transport_id
        }
        fn transport_kind(&self) -> &'static str {
            "telegram"
        }
        async fn list_channels(&self) -> Result<Vec<ChannelSummary>, ChannelOpError> {
            Ok(self.channels.clone())
        }
        async fn list_users(&self) -> Result<Vec<UserSummary>, ChannelOpError> {
            Ok(vec![UserSummary {
                id: "U1".into(),
                display_name: Some("Alice".into()),
                handle: Some("alice".into()),
                is_bot: false,
            }])
        }
        async fn lookup_user(&self, query: &str) -> Result<Option<UserSummary>, ChannelOpError> {
            if query == "U1" {
                Ok(Some(UserSummary {
                    id: "U1".into(),
                    display_name: Some("Alice".into()),
                    handle: Some("alice".into()),
                    is_bot: false,
                }))
            } else {
                Ok(None)
            }
        }
    }

    struct CannedMsg(&'static str, String);
    #[async_trait]
    impl ChannelMessagingAdapter for CannedMsg {
        fn transport_id(&self) -> &str {
            &self.1
        }
        fn transport_kind(&self) -> &'static str {
            self.0
        }
        async fn send_dm(&self, user_id: &str, _: &str) -> Result<MessageRef, ChannelOpError> {
            Ok(MessageRef {
                channel_id: format!("DM-{user_id}"),
                message_id: "1".into(),
            })
        }
        async fn send_channel(
            &self,
            channel_id: &str,
            _: &str,
        ) -> Result<MessageRef, ChannelOpError> {
            Ok(MessageRef {
                channel_id: channel_id.into(),
                message_id: "2".into(),
            })
        }
        async fn broadcast(&self, channel_ids: &[String], _: &str) -> Vec<BroadcastResult> {
            channel_ids
                .iter()
                .map(|c| BroadcastResult {
                    channel_id: c.clone(),
                    outcome: Ok(MessageRef {
                        channel_id: c.clone(),
                        message_id: "1".into(),
                    }),
                })
                .collect()
        }
    }

    struct CannedThread(&'static str, String);
    #[async_trait]
    impl ChannelThreadingAdapter for CannedThread {
        fn transport_id(&self) -> &str {
            &self.1
        }
        fn transport_kind(&self) -> &'static str {
            self.0
        }
        async fn create_thread(
            &self,
            _parent: &ThreadParent,
            _title: Option<&str>,
        ) -> Result<String, ChannelOpError> {
            Ok("T-99".into())
        }
        async fn list_threads(&self, _: &str) -> Result<Vec<ThreadSummary>, ChannelOpError> {
            Ok(vec![])
        }
        async fn follow_thread(&self, _: &str) -> Result<(), ChannelOpError> {
            Ok(())
        }
    }

    struct CannedApprove(String, ApprovalDecision);
    #[async_trait]
    impl ChannelApprovalAdapter for CannedApprove {
        fn transport_id(&self) -> &str {
            &self.0
        }
        fn transport_kind(&self) -> &'static str {
            "telegram"
        }
        async fn request_approval(
            &self,
            _channel_id: &str,
            _prompt: &str,
            _timeout: Duration,
        ) -> Result<ApprovalDecision, ChannelOpError> {
            Ok(self.1.clone())
        }
    }

    async fn build_ctx() -> Arc<ToolContext> {
        let reg = Arc::new(ChannelOpsRegistry::new());
        reg.register_directory(
            "secretary",
            Arc::new(CannedDir {
                transport_id: "telegram-main".into(),
                channels: vec![ChannelSummary {
                    id: "123".into(),
                    name: Some("alice".into()),
                    kind: ChannelKind::Dm,
                    is_member: true,
                }],
            }),
        )
        .await;
        reg.register_messaging(
            "secretary",
            Arc::new(CannedMsg("telegram", "telegram-main".into())),
        )
        .await;
        reg.register_threading(
            "secretary",
            Arc::new(CannedThread("telegram", "telegram-main".into())),
        )
        .await;
        reg.register_approval(
            "secretary",
            Arc::new(CannedApprove(
                "telegram-main".into(),
                ApprovalDecision::Approved {
                    actor_id: "U001".into(),
                    at: std::time::SystemTime::now(),
                },
            )),
        )
        .await;
        Arc::new(ToolContext::empty(PathBuf::from("/tmp")).with_channel_ops(reg))
    }

    #[tokio::test]
    async fn list_channels_returns_canned_payload() {
        let ctx = build_ctx().await;
        let h = ChannelDirectoryListChannelsHandler::new(ctx);
        let v = h
            .call(json!({ "slot_id": "secretary", "transport_id": "telegram-main" }))
            .await
            .unwrap();
        let chans = v["channels"].as_array().unwrap();
        assert_eq!(chans.len(), 1);
        assert_eq!(chans[0]["id"], "123");
        assert_eq!(chans[0]["kind"], "dm");
    }

    #[tokio::test]
    async fn list_channels_unknown_transport_is_invalid_params() {
        let ctx = build_ctx().await;
        let h = ChannelDirectoryListChannelsHandler::new(ctx);
        let err = h
            .call(json!({ "slot_id": "career", "transport_id": "telegram-main" }))
            .await
            .unwrap_err();
        let s = format!("{err:?}");
        assert!(s.contains("no 'directory' adapter"), "{s}");
    }

    #[tokio::test]
    async fn lookup_user_returns_user_object() {
        let ctx = build_ctx().await;
        let h = ChannelDirectoryLookupUserHandler::new(ctx);
        let v = h
            .call(json!({
                "slot_id": "secretary",
                "transport_id": "telegram-main",
                "query": "U1"
            }))
            .await
            .unwrap();
        assert_eq!(v["user"]["id"], "U1");
    }

    #[tokio::test]
    async fn lookup_user_returns_null_when_missing() {
        let ctx = build_ctx().await;
        let h = ChannelDirectoryLookupUserHandler::new(ctx);
        let v = h
            .call(json!({
                "slot_id": "secretary",
                "transport_id": "telegram-main",
                "query": "absent"
            }))
            .await
            .unwrap();
        assert!(v["user"].is_null());
    }

    #[tokio::test]
    async fn send_channel_returns_message_ref() {
        let ctx = build_ctx().await;
        let h = ChannelMessagingSendChannelHandler::new(ctx);
        let v = h
            .call(json!({
                "slot_id": "secretary",
                "transport_id": "telegram-main",
                "channel_id": "C1",
                "text": "hi"
            }))
            .await
            .unwrap();
        assert_eq!(v["channel_id"], "C1");
        assert_eq!(v["message_id"], "2");
    }

    #[tokio::test]
    async fn broadcast_returns_results_array() {
        let ctx = build_ctx().await;
        let h = ChannelMessagingBroadcastHandler::new(ctx);
        let v = h
            .call(json!({
                "slot_id": "secretary",
                "transport_id": "telegram-main",
                "channel_ids": ["C1", "C2"],
                "text": "hi"
            }))
            .await
            .unwrap();
        let arr = v["results"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(arr.iter().all(|r| r["ok"] == true));
    }

    #[tokio::test]
    async fn create_thread_returns_thread_id() {
        let ctx = build_ctx().await;
        let h = ChannelThreadingCreateThreadHandler::new(ctx);
        let v = h
            .call(json!({
                "slot_id": "secretary",
                "transport_id": "telegram-main",
                "parent": { "kind": "channel", "channel_id": "C1" }
            }))
            .await
            .unwrap();
        assert_eq!(v["thread_id"], "T-99");
    }

    #[tokio::test]
    async fn approval_request_returns_approved_outcome() {
        let ctx = build_ctx().await;
        let h = ChannelApprovalRequestHandler::new(ctx);
        let v = h
            .call(json!({
                "slot_id": "secretary",
                "transport_id": "telegram-main",
                "channel_id": "C1",
                "prompt": "ok?",
                "timeout_seconds": 1
            }))
            .await
            .unwrap();
        assert_eq!(v["outcome"], "approved");
    }

    // Suppress unused-import warning for ApprovalKey in this test
    // module — it's intentionally exposed for downstream callers.
    #[allow(dead_code)]
    fn _unused_apk() -> ApprovalKey {
        ApprovalKey::new("a", "b", "c")
    }
}
