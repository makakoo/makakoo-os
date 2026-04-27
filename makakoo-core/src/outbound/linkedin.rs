//! LinkedIn adapter — draft-only, send **always refused**.
//!
//! Policy: LinkedIn carries the highest reputational cost of any outbound
//! channel; one bad auto-reply to the wrong VP torches a relationship.
//! The capability grant `outbound/linkedin/draft` allows drafting;
//! `outbound/linkedin/send` does not exist and this adapter will NEVER
//! mark a linkedin draft as sent. Humans land pitches manually.

use crate::error::{MakakooError, Result};
use crate::outbound::{Draft, OutboundQueue};

pub struct LinkedInAdapter;

impl LinkedInAdapter {
    /// Never succeeds. Returns `InvalidInput` unconditionally so callers
    /// surface the policy rather than hit a confusing subprocess error.
    pub async fn send_approved(_queue: &OutboundQueue, draft: &Draft) -> Result<()> {
        Err(MakakooError::InvalidInput(format!(
            "linkedin: send refused by policy. Draft {} stays in {:?} status. \
             Open LinkedIn directly and paste the approved body. \
             (This is not a bug — Makakoo never auto-sends LinkedIn messages.)",
            draft.id, draft.status,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use crate::outbound::DraftStatus;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn send_approved_always_refuses_even_on_approved_draft() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(&dir.path().join("t.db")).unwrap();
        run_migrations(&conn).unwrap();
        let q = OutboundQueue::open(Arc::new(Mutex::new(conn))).unwrap();
        let id = q.draft("linkedin", "in/alice", None, "hey alice").unwrap();
        q.approve(id).unwrap();
        let d = q.get(id).unwrap().unwrap();
        assert_eq!(d.status, DraftStatus::Approved);
        let err = LinkedInAdapter::send_approved(&q, &d).await.unwrap_err();
        assert!(matches!(err, MakakooError::InvalidInput(_)));
        // Draft stays in Approved — nothing advanced.
        let after = q.get(id).unwrap().unwrap();
        assert_eq!(after.status, DraftStatus::Approved);
        assert!(after.sent_at.is_none());
    }
}
