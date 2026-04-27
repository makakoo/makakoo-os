//! Transport router — `(slot_id, transport_id) → Transport adapter`.
//!
//! Phase 1 keeps this in-memory only. The registry (Phase 2) will
//! populate the router from `~/MAKAKOO/config/agents/<slot>.toml`
//! on agent start.
//!
//! Routing rules (Q9 + concurrency model):
//! - PRIMARY key for outbound is `(slot_id, transport_id)`. The
//!   triple `(transport_kind, account_id, tenant_id)` is auxiliary
//!   diagnostic only.
//! - Cross-transport reply is FORBIDDEN in v1: an outbound frame
//!   whose `transport_id` doesn't match a registered adapter on the
//!   same `slot_id` is rejected without invoking the adapter.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::RwLock;

use crate::transport::frame::MakakooOutboundFrame;
use crate::transport::Transport;
use crate::Result;

/// Per-slot map: `transport_id` → adapter.
type SlotMap = HashMap<String, Arc<dyn Transport>>;

/// Errors raised by the router; bubble up to the IPC layer where
/// they are translated into structured WARN log lines and a
/// negative-ack response on the outbound socket.
#[derive(Debug, Error)]
pub enum RouterError {
    #[error("no slot '{slot_id}' is registered")]
    UnknownSlot { slot_id: String },

    #[error(
        "outbound transport_id '{transport_id}' has no matching transport on slot '{slot_id}' \
         — cross-transport reply is forbidden in v1"
    )]
    UnknownTransport {
        slot_id: String,
        transport_id: String,
    },

    #[error("transport adapter error: {0}")]
    AdapterError(#[from] crate::MakakooError),
}

/// `TransportRouter` owns the live transport adapter instances for
/// every slot the host process is supervising.  Adapters are
/// `Arc<dyn Transport>` so the router can hand out clones to the
/// inbound listener task without taking the lock for the duration
/// of a `send` call.
pub struct TransportRouter {
    slots: RwLock<HashMap<String, SlotMap>>,
}

impl TransportRouter {
    pub fn new() -> Self {
        Self {
            slots: RwLock::new(HashMap::new()),
        }
    }

    /// Register a transport for a slot. Replaces any existing
    /// adapter with the same `(slot_id, transport_id)` pair.
    pub async fn register(&self, slot_id: &str, transport: Arc<dyn Transport>) {
        let transport_id = transport.transport_id().to_string();
        let mut slots = self.slots.write().await;
        slots
            .entry(slot_id.to_string())
            .or_default()
            .insert(transport_id, transport);
    }

    /// Drop a slot's transport.  Returns true iff something was
    /// removed.
    pub async fn deregister(&self, slot_id: &str, transport_id: &str) -> bool {
        let mut slots = self.slots.write().await;
        match slots.get_mut(slot_id) {
            Some(map) => map.remove(transport_id).is_some(),
            None => false,
        }
    }

    /// Look up an adapter by `(slot_id, transport_id)`.
    pub async fn lookup(&self, slot_id: &str, transport_id: &str) -> Option<Arc<dyn Transport>> {
        let slots = self.slots.read().await;
        slots.get(slot_id).and_then(|m| m.get(transport_id)).cloned()
    }

    /// List the registered transports for a slot.  Returns
    /// `(transport_id, kind)` tuples, sorted by transport_id for
    /// deterministic output.
    pub async fn list_slot(&self, slot_id: &str) -> Vec<(String, &'static str)> {
        let slots = self.slots.read().await;
        let Some(m) = slots.get(slot_id) else {
            return vec![];
        };
        let mut out: Vec<(String, &'static str)> =
            m.iter().map(|(id, t)| (id.clone(), t.kind())).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Dispatch an outbound frame.
    ///
    /// Performs the cross-transport-reply guard and adapter
    /// selection, then calls `Transport::send`.
    pub async fn dispatch_outbound(
        &self,
        slot_id: &str,
        frame: &MakakooOutboundFrame,
    ) -> std::result::Result<(), RouterError> {
        let adapter = {
            let slots = self.slots.read().await;
            let map = slots.get(slot_id).ok_or_else(|| RouterError::UnknownSlot {
                slot_id: slot_id.to_string(),
            })?;
            map.get(&frame.transport_id)
                .cloned()
                .ok_or_else(|| RouterError::UnknownTransport {
                    slot_id: slot_id.to_string(),
                    transport_id: frame.transport_id.clone(),
                })?
        };
        adapter
            .send(frame)
            .await
            .map_err(RouterError::AdapterError)
    }
}

impl Default for TransportRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Verify no two adapters in the same `(slot_id, kind, tenant_id)`
/// triple resolve to the same `account_id`. Phase 2's
/// `agent create` calls this after every `verify_credentials` step
/// to catch duplicate-bot misconfigurations (Q11 same-kind
/// multi-transport network-level guard).
///
/// Inputs are already-resolved identities — the caller is expected
/// to have run `Transport::verify_credentials` for each adapter.
pub fn verify_no_duplicate_identities<'a>(
    identities: impl IntoIterator<
        Item = (
            &'a str, // transport_id
            &'a str, // kind
            &'a str, // account_id
            Option<&'a str>, // tenant_id (Slack: team_id)
        ),
    >,
) -> Result<()> {
    use std::collections::HashSet;
    let mut seen: HashSet<(String, String, Option<String>)> = HashSet::new();
    for (transport_id, kind, account_id, tenant_id) in identities {
        let key = (
            kind.to_string(),
            account_id.to_string(),
            tenant_id.map(|s| s.to_string()),
        );
        if !seen.insert(key.clone()) {
            return Err(crate::MakakooError::InvalidInput(format!(
                "two transports of kind '{}' resolve to the same identity (account_id='{}', tenant={:?}); \
                 transport '{}' is the duplicate. Same-kind multi-transport requires distinct bot identities.",
                kind, account_id, tenant_id, transport_id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::frame::MakakooOutboundFrame;
    use crate::transport::VerifiedIdentity;
    use async_trait::async_trait;

    /// Test double for a transport adapter — records sends.
    struct FakeAdapter {
        kind: &'static str,
        transport_id: String,
        sent: tokio::sync::Mutex<Vec<MakakooOutboundFrame>>,
    }

    #[async_trait]
    impl Transport for FakeAdapter {
        fn kind(&self) -> &'static str {
            self.kind
        }
        fn transport_id(&self) -> &str {
            &self.transport_id
        }
        async fn verify_credentials(&self) -> crate::Result<VerifiedIdentity> {
            Ok(VerifiedIdentity {
                account_id: "acc-x".into(),
                tenant_id: None,
                display_name: None,
            })
        }
        async fn send(&self, frame: &MakakooOutboundFrame) -> crate::Result<()> {
            self.sent.lock().await.push(frame.clone());
            Ok(())
        }
    }

    fn fake(kind: &'static str, id: &str) -> Arc<FakeAdapter> {
        Arc::new(FakeAdapter {
            kind,
            transport_id: id.into(),
            sent: tokio::sync::Mutex::new(vec![]),
        })
    }

    #[tokio::test]
    async fn lookup_finds_registered_adapter() {
        let router = TransportRouter::new();
        let adapter = fake("telegram", "telegram-main");
        router.register("secretary", adapter).await;
        let found = router.lookup("secretary", "telegram-main").await;
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn dispatch_routes_to_correct_adapter() {
        let router = TransportRouter::new();
        let tg = fake("telegram", "telegram-main");
        let sl = fake("slack", "slack-main");
        router.register("secretary", tg.clone()).await;
        router.register("secretary", sl.clone()).await;

        let frame = MakakooOutboundFrame {
            transport_id: "slack-main".into(),
            transport_kind: "slack".into(),
            conversation_id: "D0123ABCD".into(),
            thread_id: None,
            thread_kind: None,
            text: "hi".into(),
            reply_to_message_id: None,
        };
        router.dispatch_outbound("secretary", &frame).await.unwrap();
        assert_eq!(sl.sent.lock().await.len(), 1);
        assert_eq!(tg.sent.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn dispatch_rejects_cross_transport() {
        let router = TransportRouter::new();
        router
            .register("secretary", fake("telegram", "telegram-main"))
            .await;
        // Outbound says transport_id="slack-main" but no Slack adapter
        // is registered on this slot.
        let frame = MakakooOutboundFrame {
            transport_id: "slack-main".into(),
            transport_kind: "slack".into(),
            conversation_id: "D0123ABCD".into(),
            thread_id: None,
            thread_kind: None,
            text: "hi".into(),
            reply_to_message_id: None,
        };
        let err = router.dispatch_outbound("secretary", &frame).await.unwrap_err();
        assert!(matches!(err, RouterError::UnknownTransport { .. }));
    }

    #[tokio::test]
    async fn dispatch_rejects_unknown_slot() {
        let router = TransportRouter::new();
        let frame = MakakooOutboundFrame {
            transport_id: "telegram-main".into(),
            transport_kind: "telegram".into(),
            conversation_id: "1".into(),
            thread_id: None,
            thread_kind: None,
            text: "hi".into(),
            reply_to_message_id: None,
        };
        let err = router.dispatch_outbound("nope", &frame).await.unwrap_err();
        assert!(matches!(err, RouterError::UnknownSlot { .. }));
    }

    #[tokio::test]
    async fn list_slot_returns_sorted() {
        let router = TransportRouter::new();
        router
            .register("secretary", fake("slack", "z-slack"))
            .await;
        router
            .register("secretary", fake("telegram", "a-telegram"))
            .await;
        let listed = router.list_slot("secretary").await;
        assert_eq!(
            listed,
            vec![
                ("a-telegram".into(), "telegram"),
                ("z-slack".into(), "slack"),
            ]
        );
    }

    #[test]
    fn duplicate_identity_check_rejects_same_telegram_bot() {
        let identities = [
            ("tg-a", "telegram", "12345678", None),
            ("tg-b", "telegram", "12345678", None),
        ];
        let err = verify_no_duplicate_identities(identities.iter().map(|t| (t.0, t.1, t.2, t.3)))
            .unwrap_err();
        assert!(format!("{err}").contains("same identity"));
    }

    #[test]
    fn duplicate_identity_check_rejects_same_slack_in_same_team() {
        let identities = [
            ("slack-a", "slack", "B01ABC", Some("T0123")),
            ("slack-b", "slack", "B01ABC", Some("T0123")),
        ];
        let err = verify_no_duplicate_identities(identities.iter().map(|t| (t.0, t.1, t.2, t.3)))
            .unwrap_err();
        assert!(format!("{err}").contains("same identity"));
    }

    #[test]
    fn duplicate_identity_check_permits_same_bot_token_in_different_teams() {
        let identities = [
            ("slack-a", "slack", "B01ABC", Some("T0123")),
            ("slack-b", "slack", "B01ABC", Some("T9999")),
        ];
        verify_no_duplicate_identities(identities.iter().map(|t| (t.0, t.1, t.2, t.3))).unwrap();
    }

    #[test]
    fn duplicate_identity_check_permits_distinct_bots_same_kind() {
        let identities = [
            ("tg-a", "telegram", "12345678", None),
            ("tg-b", "telegram", "98765432", None),
        ];
        verify_no_duplicate_identities(identities.iter().map(|t| (t.0, t.1, t.2, t.3))).unwrap();
    }
}
