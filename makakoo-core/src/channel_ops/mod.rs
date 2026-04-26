//! Channel-ops adapter family — Phase 6 OpenClaw-parity layer.
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS-V2.0-MEGA Phase 6:
//!
//! Four trait families abstract per-transport channel operations so
//! the LLM (via MCP) can list/lookup, request approval, route DM-vs-
//! channel, and manage threads without knowing whether the underlying
//! transport is Telegram, Slack, Discord, etc:
//!
//! 1. [`directory::ChannelDirectoryAdapter`] — list channels/users + lookup
//! 2. [`approval::ChannelApprovalAdapter`]   — yes/no/timeout approvals
//! 3. [`messaging::ChannelMessagingAdapter`] — DM / channel / broadcast
//! 4. [`threading::ChannelThreadingAdapter`] — create/list/follow threads
//!
//! Per-transport impls live in `channel_ops/telegram.rs` and
//! `channel_ops/slack.rs` (Discord lands in Phase 7). Each impl wraps
//! the existing transport adapter (`Arc<TelegramAdapter>`,
//! `Arc<SlackAdapter>`) so it can reuse the resolved bot token,
//! `reqwest::Client`, and `api_base` override hook.
//!
//! [`ChannelOpsRegistry`] holds `(slot_id, transport_id) → adapter`
//! maps for each of the four trait families. Slot isolation is the
//! core safety property: a slot's lookup MUST NEVER return another
//! slot's adapter (verified by isolation tests).

pub mod approval;
pub mod directory;
pub mod messaging;
pub mod slack;
pub mod telegram;
pub mod threading;

pub use approval::{
    parse_decision_text, ApprovalCenter, ApprovalDecision, ApprovalKey, ChannelApprovalAdapter,
};
pub use directory::{
    ChannelDirectoryAdapter, ChannelKind, ChannelOpError, ChannelSummary, UserSummary,
};
pub use messaging::{BroadcastResult, ChannelMessagingAdapter, MessageRef};
pub use threading::{ChannelThreadingAdapter, ThreadParent, ThreadSummary};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct OpKey {
    slot_id: String,
    transport_id: String,
}

impl OpKey {
    fn new(slot_id: &str, transport_id: &str) -> Self {
        Self {
            slot_id: slot_id.to_string(),
            transport_id: transport_id.to_string(),
        }
    }
}

/// Registry holding the four trait-family maps, keyed by
/// `(slot_id, transport_id)`. One instance per running supervisor.
pub struct ChannelOpsRegistry {
    directory: RwLock<HashMap<OpKey, Arc<dyn ChannelDirectoryAdapter>>>,
    messaging: RwLock<HashMap<OpKey, Arc<dyn ChannelMessagingAdapter>>>,
    approval: RwLock<HashMap<OpKey, Arc<dyn ChannelApprovalAdapter>>>,
    threading: RwLock<HashMap<OpKey, Arc<dyn ChannelThreadingAdapter>>>,
}

impl ChannelOpsRegistry {
    pub fn new() -> Self {
        Self {
            directory: RwLock::new(HashMap::new()),
            messaging: RwLock::new(HashMap::new()),
            approval: RwLock::new(HashMap::new()),
            threading: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register_directory(
        &self,
        slot_id: &str,
        adapter: Arc<dyn ChannelDirectoryAdapter>,
    ) {
        let key = OpKey::new(slot_id, adapter.transport_id());
        self.directory.write().await.insert(key, adapter);
    }

    pub async fn register_messaging(
        &self,
        slot_id: &str,
        adapter: Arc<dyn ChannelMessagingAdapter>,
    ) {
        let key = OpKey::new(slot_id, adapter.transport_id());
        self.messaging.write().await.insert(key, adapter);
    }

    pub async fn register_approval(
        &self,
        slot_id: &str,
        adapter: Arc<dyn ChannelApprovalAdapter>,
    ) {
        let key = OpKey::new(slot_id, adapter.transport_id());
        self.approval.write().await.insert(key, adapter);
    }

    pub async fn register_threading(
        &self,
        slot_id: &str,
        adapter: Arc<dyn ChannelThreadingAdapter>,
    ) {
        let key = OpKey::new(slot_id, adapter.transport_id());
        self.threading.write().await.insert(key, adapter);
    }

    pub async fn lookup_directory(
        &self,
        slot_id: &str,
        transport_id: &str,
    ) -> Option<Arc<dyn ChannelDirectoryAdapter>> {
        self.directory
            .read()
            .await
            .get(&OpKey::new(slot_id, transport_id))
            .cloned()
    }

    pub async fn lookup_messaging(
        &self,
        slot_id: &str,
        transport_id: &str,
    ) -> Option<Arc<dyn ChannelMessagingAdapter>> {
        self.messaging
            .read()
            .await
            .get(&OpKey::new(slot_id, transport_id))
            .cloned()
    }

    pub async fn lookup_approval(
        &self,
        slot_id: &str,
        transport_id: &str,
    ) -> Option<Arc<dyn ChannelApprovalAdapter>> {
        self.approval
            .read()
            .await
            .get(&OpKey::new(slot_id, transport_id))
            .cloned()
    }

    pub async fn lookup_threading(
        &self,
        slot_id: &str,
        transport_id: &str,
    ) -> Option<Arc<dyn ChannelThreadingAdapter>> {
        self.threading
            .read()
            .await
            .get(&OpKey::new(slot_id, transport_id))
            .cloned()
    }

    /// List all `(transport_id, kind)` pairs registered for a slot in
    /// the directory family. Used by the MCP `channel_directory.list`
    /// surface to enumerate available transports for a slot without
    /// leaking other slots' transports.
    pub async fn list_slot_directories(&self, slot_id: &str) -> Vec<(String, &'static str)> {
        let map = self.directory.read().await;
        let mut out: Vec<(String, &'static str)> = map
            .iter()
            .filter(|(k, _)| k.slot_id == slot_id)
            .map(|(k, a)| (k.transport_id.clone(), a.transport_kind()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Drop every adapter registered against `slot_id` across all
    /// four families. Called by the supervisor on slot stop.
    pub async fn drop_slot(&self, slot_id: &str) {
        self.directory
            .write()
            .await
            .retain(|k, _| k.slot_id != slot_id);
        self.messaging
            .write()
            .await
            .retain(|k, _| k.slot_id != slot_id);
        self.approval
            .write()
            .await
            .retain(|k, _| k.slot_id != slot_id);
        self.threading
            .write()
            .await
            .retain(|k, _| k.slot_id != slot_id);
    }
}

impl Default for ChannelOpsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::time::Duration;

    // ── mock adapters covering all four trait families ────────

    struct FakeDir {
        kind: &'static str,
        transport_id: String,
        channels: Vec<ChannelSummary>,
    }

    #[async_trait]
    impl ChannelDirectoryAdapter for FakeDir {
        fn transport_id(&self) -> &str {
            &self.transport_id
        }
        fn transport_kind(&self) -> &'static str {
            self.kind
        }
        async fn list_channels(&self) -> Result<Vec<ChannelSummary>, ChannelOpError> {
            Ok(self.channels.clone())
        }
        async fn list_users(&self) -> Result<Vec<UserSummary>, ChannelOpError> {
            Ok(vec![])
        }
        async fn lookup_user(
            &self,
            _query: &str,
        ) -> Result<Option<UserSummary>, ChannelOpError> {
            Ok(None)
        }
    }

    struct FakeMsg {
        kind: &'static str,
        transport_id: String,
    }

    #[async_trait]
    impl ChannelMessagingAdapter for FakeMsg {
        fn transport_id(&self) -> &str {
            &self.transport_id
        }
        fn transport_kind(&self) -> &'static str {
            self.kind
        }
        async fn send_dm(&self, user_id: &str, _text: &str) -> Result<MessageRef, ChannelOpError> {
            Ok(MessageRef {
                channel_id: format!("DM-{user_id}"),
                message_id: "1".into(),
            })
        }
        async fn send_channel(
            &self,
            channel_id: &str,
            _text: &str,
        ) -> Result<MessageRef, ChannelOpError> {
            Ok(MessageRef {
                channel_id: channel_id.into(),
                message_id: "1".into(),
            })
        }
        async fn broadcast(
            &self,
            channel_ids: &[String],
            _text: &str,
        ) -> Vec<BroadcastResult> {
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

    struct FakeApprove {
        kind: &'static str,
        transport_id: String,
        canned: ApprovalDecision,
    }

    #[async_trait]
    impl ChannelApprovalAdapter for FakeApprove {
        fn transport_id(&self) -> &str {
            &self.transport_id
        }
        fn transport_kind(&self) -> &'static str {
            self.kind
        }
        async fn request_approval(
            &self,
            _channel_id: &str,
            _prompt: &str,
            _timeout: Duration,
        ) -> Result<ApprovalDecision, ChannelOpError> {
            Ok(self.canned.clone())
        }
    }

    struct FakeThread {
        kind: &'static str,
        transport_id: String,
    }

    #[async_trait]
    impl ChannelThreadingAdapter for FakeThread {
        fn transport_id(&self) -> &str {
            &self.transport_id
        }
        fn transport_kind(&self) -> &'static str {
            self.kind
        }
        async fn create_thread(
            &self,
            _parent: &ThreadParent,
            _title: Option<&str>,
        ) -> Result<String, ChannelOpError> {
            Ok("THREAD-1".into())
        }
        async fn list_threads(
            &self,
            _channel_id: &str,
        ) -> Result<Vec<ThreadSummary>, ChannelOpError> {
            Ok(vec![])
        }
        async fn follow_thread(&self, _thread_id: &str) -> Result<(), ChannelOpError> {
            Ok(())
        }
    }

    fn fake(kind: &'static str, transport_id: &str) -> Arc<dyn ChannelDirectoryAdapter> {
        Arc::new(FakeDir {
            kind,
            transport_id: transport_id.into(),
            channels: vec![],
        })
    }

    fn fake_with_channels(
        kind: &'static str,
        transport_id: &str,
        channels: Vec<ChannelSummary>,
    ) -> Arc<dyn ChannelDirectoryAdapter> {
        Arc::new(FakeDir {
            kind,
            transport_id: transport_id.into(),
            channels,
        })
    }

    fn fake_msg(kind: &'static str, transport_id: &str) -> Arc<dyn ChannelMessagingAdapter> {
        Arc::new(FakeMsg {
            kind,
            transport_id: transport_id.into(),
        })
    }

    fn fake_thread(kind: &'static str, transport_id: &str) -> Arc<dyn ChannelThreadingAdapter> {
        Arc::new(FakeThread {
            kind,
            transport_id: transport_id.into(),
        })
    }

    fn fake_approve(
        kind: &'static str,
        transport_id: &str,
        canned: ApprovalDecision,
    ) -> Arc<dyn ChannelApprovalAdapter> {
        Arc::new(FakeApprove {
            kind,
            transport_id: transport_id.into(),
            canned,
        })
    }

    // ── registry mechanics ────────────────────────────────────

    #[tokio::test]
    async fn registry_lookup_finds_registered_adapter() {
        let r = ChannelOpsRegistry::new();
        r.register_directory("secretary", fake("telegram", "telegram-main"))
            .await;
        let got = r.lookup_directory("secretary", "telegram-main").await;
        assert!(got.is_some());
    }

    #[tokio::test]
    async fn registry_isolation_rejects_cross_slot_lookup() {
        let r = ChannelOpsRegistry::new();
        r.register_directory("secretary", fake("telegram", "telegram-main"))
            .await;
        // career slot tries to access secretary's adapter via the same
        // transport_id — must MISS, even though transport_id matches.
        let leaked = r.lookup_directory("career", "telegram-main").await;
        assert!(
            leaked.is_none(),
            "cross-slot lookup leaked another slot's adapter"
        );
    }

    #[tokio::test]
    async fn list_slot_directories_omits_other_slots() {
        let r = ChannelOpsRegistry::new();
        r.register_directory("secretary", fake("telegram", "telegram-main"))
            .await;
        r.register_directory("career", fake("slack", "slack-main"))
            .await;
        let secs = r.list_slot_directories("secretary").await;
        assert_eq!(secs, vec![("telegram-main".into(), "telegram")]);
        let car = r.list_slot_directories("career").await;
        assert_eq!(car, vec![("slack-main".into(), "slack")]);
    }

    #[tokio::test]
    async fn drop_slot_removes_only_target_slot() {
        let r = ChannelOpsRegistry::new();
        r.register_directory("a", fake("telegram", "telegram-main"))
            .await;
        r.register_directory("b", fake("slack", "slack-main")).await;
        r.drop_slot("a").await;
        assert!(r.lookup_directory("a", "telegram-main").await.is_none());
        assert!(r.lookup_directory("b", "slack-main").await.is_some());
    }

    // ── integration tests ─────────────────────────────────────

    #[tokio::test]
    async fn integration_register_all_four_families_for_one_slot() {
        let r = ChannelOpsRegistry::new();
        let slot = "secretary";
        let tid = "telegram-main";
        r.register_directory(slot, fake("telegram", tid)).await;
        r.register_messaging(slot, fake_msg("telegram", tid)).await;
        r.register_threading(slot, fake_thread("telegram", tid)).await;
        r.register_approval(
            slot,
            fake_approve(
                "telegram",
                tid,
                ApprovalDecision::Approved {
                    actor_id: "U001".into(),
                    at: std::time::SystemTime::now(),
                },
            ),
        )
        .await;

        // All four lookups MUST hit on the (slot, transport) pair.
        assert!(r.lookup_directory(slot, tid).await.is_some());
        assert!(r.lookup_messaging(slot, tid).await.is_some());
        assert!(r.lookup_threading(slot, tid).await.is_some());
        assert!(r.lookup_approval(slot, tid).await.is_some());

        // And calling through them works end-to-end.
        let dir = r.lookup_directory(slot, tid).await.unwrap();
        let _ = dir.list_channels().await.unwrap();

        let msg = r.lookup_messaging(slot, tid).await.unwrap();
        let mref = msg.send_channel("C-1", "hello").await.unwrap();
        assert_eq!(mref.channel_id, "C-1");

        let thread = r.lookup_threading(slot, tid).await.unwrap();
        let tid_out = thread
            .create_thread(&ThreadParent::Channel("C-1".into()), Some("topic"))
            .await
            .unwrap();
        assert_eq!(tid_out, "THREAD-1");

        let appr = r.lookup_approval(slot, tid).await.unwrap();
        let dec = appr
            .request_approval("C-1", "ok?", Duration::from_millis(10))
            .await
            .unwrap();
        assert!(matches!(dec, ApprovalDecision::Approved { .. }));
    }

    #[tokio::test]
    async fn integration_two_slots_two_transports_no_crosstalk() {
        let r = ChannelOpsRegistry::new();
        r.register_directory("secretary", fake("telegram", "telegram-main"))
            .await;
        r.register_messaging("secretary", fake_msg("telegram", "telegram-main"))
            .await;
        r.register_directory("career", fake("slack", "slack-main"))
            .await;
        r.register_messaging("career", fake_msg("slack", "slack-main"))
            .await;

        // Each slot only sees its own adapters.
        assert!(r.lookup_directory("secretary", "telegram-main").await.is_some());
        assert!(r.lookup_messaging("secretary", "slack-main").await.is_none());
        assert!(r.lookup_directory("career", "slack-main").await.is_some());
        assert!(r.lookup_messaging("career", "telegram-main").await.is_none());
    }

    #[tokio::test]
    async fn integration_directory_list_returns_canned_payload_through_registry() {
        let r = ChannelOpsRegistry::new();
        let canned = vec![
            ChannelSummary {
                id: "C1".into(),
                name: Some("general".into()),
                kind: ChannelKind::Channel,
                is_member: true,
            },
            ChannelSummary {
                id: "D1".into(),
                name: None,
                kind: ChannelKind::Dm,
                is_member: true,
            },
        ];
        r.register_directory(
            "secretary",
            fake_with_channels("slack", "slack-main", canned.clone()),
        )
        .await;
        let dir = r
            .lookup_directory("secretary", "slack-main")
            .await
            .expect("directory adapter registered");
        let live = dir.list_channels().await.unwrap();
        assert_eq!(live, canned);
    }

    #[tokio::test]
    async fn integration_drop_slot_clears_all_four_families() {
        let r = ChannelOpsRegistry::new();
        let slot = "ephemeral";
        r.register_directory(slot, fake("telegram", "telegram-main"))
            .await;
        r.register_messaging(slot, fake_msg("telegram", "telegram-main"))
            .await;
        r.register_threading(slot, fake_thread("telegram", "telegram-main"))
            .await;
        r.register_approval(
            slot,
            fake_approve("telegram", "telegram-main", ApprovalDecision::Timeout),
        )
        .await;

        r.drop_slot(slot).await;

        assert!(r.lookup_directory(slot, "telegram-main").await.is_none());
        assert!(r.lookup_messaging(slot, "telegram-main").await.is_none());
        assert!(r.lookup_threading(slot, "telegram-main").await.is_none());
        assert!(r.lookup_approval(slot, "telegram-main").await.is_none());
    }
}
