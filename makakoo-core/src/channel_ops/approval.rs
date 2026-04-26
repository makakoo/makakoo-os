//! `ChannelApprovalAdapter` — request_approval (yes/no/timeout).
//!
//! Phase 6 / Q6. The LLM asks the human "may I do X?" and blocks until
//! the human replies yes/no in the same channel, or until the timeout
//! elapses. Implementations:
//!
//! 1. Register a pending entry on the [`ApprovalCenter`] (one entry
//!    per `(slot_id, transport_id, channel_id)`).
//! 2. Send the prompt as a message in the channel.
//! 3. Await the oneshot receiver with the requested timeout.
//! 4. Drop the pending entry on every return path (success, error,
//!    timeout) so a stale entry can't block a future approval.
//!
//! The transport's inbound flow is responsible for calling
//! `ApprovalCenter::try_resolve` when it sees a yes/no reply in the
//! same channel. Phase 6 ships the API + center + per-transport
//! impls; the inbound-side wiring lands when the supervisor flushes
//! frames through it (Phase 12 remainder).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::sync::oneshot;

use crate::channel_ops::directory::ChannelOpError;

/// What the human chose, or that they did not choose in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved {
        actor_id: String,
        at: SystemTime,
    },
    Denied {
        actor_id: String,
        at: SystemTime,
        reason: Option<String>,
    },
    Timeout,
}

/// Composite key: one approval per `(slot, transport, channel)` at a
/// time. A new request on the same key replaces the prior pending
/// entry (the prior caller's await returns `Timeout`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalKey {
    pub slot_id: String,
    pub transport_id: String,
    pub channel_id: String,
}

impl ApprovalKey {
    pub fn new(slot_id: &str, transport_id: &str, channel_id: &str) -> Self {
        Self {
            slot_id: slot_id.to_string(),
            transport_id: transport_id.to_string(),
            channel_id: channel_id.to_string(),
        }
    }
}

/// Process-wide approvals registry. One instance per running
/// supervisor.
pub struct ApprovalCenter {
    pending: Mutex<HashMap<ApprovalKey, oneshot::Sender<ApprovalDecision>>>,
}

impl ApprovalCenter {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register a pending approval. Returns the receiver the caller
    /// awaits on. If a prior approval is pending on the same key, it
    /// is resolved with `Timeout` so its caller can unwind.
    pub fn register(&self, key: ApprovalKey) -> oneshot::Receiver<ApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        let mut p = self.pending.lock().unwrap();
        if let Some(prior) = p.insert(key, tx) {
            let _ = prior.send(ApprovalDecision::Timeout);
        }
        rx
    }

    /// Drop a pending approval (called on completion/error paths).
    /// Idempotent.
    pub fn drop_pending(&self, key: &ApprovalKey) {
        self.pending.lock().unwrap().remove(key);
    }

    /// Resolve a pending approval. Returns true iff one was found.
    pub fn try_resolve(&self, key: &ApprovalKey, decision: ApprovalDecision) -> bool {
        let mut p = self.pending.lock().unwrap();
        match p.remove(key) {
            Some(tx) => {
                let _ = tx.send(decision);
                true
            }
            None => false,
        }
    }

    /// Number of pending approvals — diagnostic only.
    pub fn pending_len(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

impl Default for ApprovalCenter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
pub trait ChannelApprovalAdapter: Send + Sync {
    fn transport_id(&self) -> &str;
    fn transport_kind(&self) -> &'static str;

    /// Send the prompt and block until the user replies yes/no or
    /// the timeout elapses.
    async fn request_approval(
        &self,
        channel_id: &str,
        prompt: &str,
        timeout: Duration,
    ) -> Result<ApprovalDecision, ChannelOpError>;
}

/// Parse a free-text reply into an `ApprovalDecision`. Recognized
/// affirmative tokens: `yes`, `y`, `approve`, `approved`, `ok`, `👍`.
/// Recognized negative tokens: `no`, `n`, `deny`, `denied`, `reject`,
/// `rejected`, `👎`. Comparison is case-insensitive and only the
/// first whitespace-separated token is examined; the remainder, if
/// any, is captured as the deny reason.
///
/// Returns `None` when the reply matches neither — callers leave the
/// approval pending for a future reply.
pub fn parse_decision_text(reply: &str, actor_id: &str) -> Option<ApprovalDecision> {
    let trimmed = reply.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    let tail = parts.next().map(|s| s.trim().to_string());
    let lower = head.to_ascii_lowercase();
    let now = SystemTime::now();
    match lower.as_str() {
        "yes" | "y" | "approve" | "approved" | "ok" | "okay" | "👍" | "✅" => {
            Some(ApprovalDecision::Approved {
                actor_id: actor_id.to_string(),
                at: now,
            })
        }
        "no" | "n" | "deny" | "denied" | "reject" | "rejected" | "👎" | "❌" => {
            Some(ApprovalDecision::Denied {
                actor_id: actor_id.to_string(),
                at: now,
                reason: tail.filter(|s| !s.is_empty()),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_yes_variants() {
        for token in ["yes", "Y", "approve", "ok", "👍"] {
            let d = parse_decision_text(token, "U001").expect(token);
            assert!(matches!(d, ApprovalDecision::Approved { .. }), "{token}");
        }
    }

    #[test]
    fn parse_no_with_reason() {
        let d = parse_decision_text("no  not safe enough", "U001").unwrap();
        match d {
            ApprovalDecision::Denied { reason, .. } => {
                assert_eq!(reason.as_deref(), Some("not safe enough"));
            }
            _ => panic!("expected Denied"),
        }
    }

    #[test]
    fn parse_unrecognized_returns_none() {
        assert!(parse_decision_text("maybe", "x").is_none());
        assert!(parse_decision_text("", "x").is_none());
        assert!(parse_decision_text("   ", "x").is_none());
    }

    #[test]
    fn center_resolve_returns_true_when_pending() {
        let c = ApprovalCenter::new();
        let key = ApprovalKey::new("s", "t", "C1");
        let _rx = c.register(key.clone());
        assert!(c.try_resolve(
            &key,
            ApprovalDecision::Approved {
                actor_id: "U1".into(),
                at: SystemTime::now(),
            }
        ));
        // Resolving again returns false (entry already taken).
        assert!(!c.try_resolve(&key, ApprovalDecision::Timeout));
    }

    #[tokio::test]
    async fn center_replace_unblocks_prior_caller() {
        let c = ApprovalCenter::new();
        let key = ApprovalKey::new("s", "t", "C1");
        let rx_first = c.register(key.clone());
        // Second register replaces the first. The first awaiter
        // resolves to Timeout to free its task.
        let _rx_second = c.register(key.clone());
        let recv = rx_first.await.unwrap();
        assert_eq!(recv, ApprovalDecision::Timeout);
    }

    #[test]
    fn drop_pending_is_idempotent() {
        let c = ApprovalCenter::new();
        let key = ApprovalKey::new("s", "t", "C1");
        c.drop_pending(&key);
        let _ = c.register(key.clone());
        c.drop_pending(&key);
        c.drop_pending(&key);
        assert_eq!(c.pending_len(), 0);
    }
}
