//! Outbound draft buffer + human-in-the-loop approval queue.
//!
//! HARD RULE — the Rust API intentionally does not expose a `send()`
//! method. A draft always starts in `Pending`, must pass through
//! `Approved` on its way to `Sent`, and only channel adapters (Gmail,
//! LinkedIn, Telegram) are allowed to call `mark_sent` — and they must
//! reject any draft whose status is not already `Approved`. This is
//! the Rust embodiment of the project's "never auto-send" policy.
//!
//! Schema lives in `db.rs` as `outbound_drafts`. All functions here
//! operate on an `Arc<Mutex<Connection>>` shared with the rest of the
//! makakoo-core crate so a single sqlite file owns the whole OS.
//!
//! This module is intentionally small — it does not speak SMTP, IMAP,
//! LinkedIn, or the Telegram API. It is a durable buffer with a rule.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftStatus {
    Pending,
    Approved,
    Sent,
    Rejected,
}

impl DraftStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DraftStatus::Pending => "pending",
            DraftStatus::Approved => "approved",
            DraftStatus::Sent => "sent",
            DraftStatus::Rejected => "rejected",
        }
    }

    pub fn from_db(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => DraftStatus::Pending,
            "approved" => DraftStatus::Approved,
            "sent" => DraftStatus::Sent,
            "rejected" => DraftStatus::Rejected,
            other => {
                return Err(MakakooError::internal(format!(
                    "outbound: unknown draft status '{other}' in db"
                )))
            }
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Draft {
    pub id: i64,
    pub channel: String,
    pub recipient: String,
    pub subject: Option<String>,
    pub body: String,
    pub status: DraftStatus,
    pub created_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
    pub sent_at: Option<DateTime<Utc>>,
    pub reject_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OutboundStats {
    pub pending: i64,
    pub approved: i64,
    pub sent: i64,
    pub rejected: i64,
    pub total: i64,
}

pub struct OutboundQueue {
    conn: Arc<Mutex<Connection>>,
}

impl OutboundQueue {
    /// Open a queue on top of an existing shared connection. Expects
    /// the `outbound_drafts` table to already exist (created by
    /// `db::run_migrations`).
    pub fn open(conn: Arc<Mutex<Connection>>) -> Result<Self> {
        Ok(Self { conn })
    }

    /// Create a new draft. Always stored with `status = Pending`. This
    /// function deliberately does not accept a status argument — the
    /// only way to advance a draft is through `approve` / `reject` /
    /// `mark_sent`.
    pub fn draft(
        &self,
        channel: &str,
        recipient: &str,
        subject: Option<&str>,
        body: &str,
    ) -> Result<i64> {
        if channel.is_empty() {
            return Err(MakakooError::internal("outbound::draft channel is empty"));
        }
        if recipient.is_empty() {
            return Err(MakakooError::internal("outbound::draft recipient is empty"));
        }
        if body.is_empty() {
            return Err(MakakooError::internal("outbound::draft body is empty"));
        }

        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO outbound_drafts
                (channel, recipient, subject, body, status, created_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![channel, recipient, subject, body, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Advance a pending draft to approved. Rejected/sent/approved
    /// drafts cannot be re-approved — the function returns an error
    /// rather than silently overwriting history.
    pub fn approve(&self, draft_id: i64) -> Result<()> {
        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();
        let updated = conn.execute(
            "UPDATE outbound_drafts
                SET status = 'approved', approved_at = ?1
              WHERE id = ?2 AND status = 'pending'",
            params![now, draft_id],
        )?;
        if updated == 0 {
            return Err(MakakooError::internal(format!(
                "outbound::approve draft {draft_id} is not pending (or does not exist)"
            )));
        }
        Ok(())
    }

    /// Reject a pending draft with a reason. Rejected drafts stay in
    /// the table for audit; they cannot be resurrected.
    pub fn reject(&self, draft_id: i64, reason: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        let updated = conn.execute(
            "UPDATE outbound_drafts
                SET status = 'rejected', reject_reason = ?1
              WHERE id = ?2 AND status = 'pending'",
            params![reason, draft_id],
        )?;
        if updated == 0 {
            return Err(MakakooError::internal(format!(
                "outbound::reject draft {draft_id} is not pending"
            )));
        }
        Ok(())
    }

    /// Called by a channel adapter after the message has gone out on
    /// the wire. HARD RULE: this transitions `Approved → Sent` only.
    /// A draft that is still `Pending` can never be marked `Sent`
    /// without first passing through `approve()`.
    pub fn mark_sent(&self, draft_id: i64) -> Result<()> {
        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();
        let updated = conn.execute(
            "UPDATE outbound_drafts
                SET status = 'sent', sent_at = ?1
              WHERE id = ?2 AND status = 'approved'",
            params![now, draft_id],
        )?;
        if updated == 0 {
            return Err(MakakooError::internal(format!(
                "outbound::mark_sent draft {draft_id} is not approved \
                 (auto-send is not permitted)"
            )));
        }
        Ok(())
    }

    pub fn pending(&self) -> Result<Vec<Draft>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, channel, recipient, subject, body, status,
                    created_at, approved_at, sent_at, reject_reason
               FROM outbound_drafts
              WHERE status = 'pending'
              ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], row_to_draft)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get(&self, draft_id: i64) -> Result<Option<Draft>> {
        let conn = self.lock_conn()?;
        let row = conn
            .query_row(
                "SELECT id, channel, recipient, subject, body, status,
                        created_at, approved_at, sent_at, reject_reason
                   FROM outbound_drafts
                  WHERE id = ?1",
                params![draft_id],
                row_to_draft,
            )
            .optional()?;
        Ok(row)
    }

    pub fn stats(&self) -> Result<OutboundStats> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*) FROM outbound_drafts GROUP BY status",
        )?;
        let rows = stmt.query_map([], |row| {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((status, count))
        })?;
        let mut stats = OutboundStats::default();
        for r in rows {
            let (status, count) = r?;
            match status.as_str() {
                "pending" => stats.pending = count,
                "approved" => stats.approved = count,
                "sent" => stats.sent = count,
                "rejected" => stats.rejected = count,
                _ => {}
            }
            stats.total += count;
        }
        Ok(stats)
    }

    /// Purge rejected drafts older than `days` days. Returns the
    /// number of rows deleted. Sent drafts are kept for audit.
    pub fn purge_rejected_older_than(&self, days: i64) -> Result<usize> {
        let cutoff = (Utc::now() - Duration::days(days)).to_rfc3339();
        let conn = self.lock_conn()?;
        let n = conn.execute(
            "DELETE FROM outbound_drafts
              WHERE status = 'rejected' AND created_at < ?1",
            params![cutoff],
        )?;
        Ok(n)
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| MakakooError::internal("outbound queue mutex poisoned"))
    }
}

fn row_to_draft(row: &Row<'_>) -> rusqlite::Result<Draft> {
    let status_str: String = row.get(5)?;
    let status = DraftStatus::from_db(&status_str).map_err(|_| {
        rusqlite::Error::InvalidColumnType(5, "status".into(), rusqlite::types::Type::Text)
    })?;
    let created_at: String = row.get(6)?;
    let approved_at: Option<String> = row.get(7)?;
    let sent_at: Option<String> = row.get(8)?;
    Ok(Draft {
        id: row.get(0)?,
        channel: row.get(1)?,
        recipient: row.get(2)?,
        subject: row.get(3)?,
        body: row.get(4)?,
        status,
        created_at: parse_dt(&created_at),
        approved_at: approved_at.as_deref().map(parse_dt),
        sent_at: sent_at.as_deref().map(parse_dt),
        reject_reason: row.get(9)?,
    })
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};

    fn open_queue() -> (tempfile::TempDir, OutboundQueue) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = open_db(&path).unwrap();
        run_migrations(&conn).unwrap();
        let shared = Arc::new(Mutex::new(conn));
        let q = OutboundQueue::open(shared).unwrap();
        (dir, q)
    }

    #[test]
    fn draft_starts_pending_and_is_visible_in_pending_list() {
        let (_d, q) = open_queue();
        let id = q
            .draft("email", "alice@example.com", Some("hi"), "hello")
            .unwrap();
        let d = q.get(id).unwrap().unwrap();
        assert_eq!(d.status, DraftStatus::Pending);
        assert_eq!(d.channel, "email");
        assert_eq!(d.recipient, "alice@example.com");
        assert_eq!(q.pending().unwrap().len(), 1);
        assert!(d.approved_at.is_none());
        assert!(d.sent_at.is_none());
    }

    #[test]
    fn pending_draft_cannot_be_marked_sent_without_approval() {
        let (_d, q) = open_queue();
        let id = q.draft("email", "bob@example.com", None, "body").unwrap();
        let err = q.mark_sent(id).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not approved") || msg.contains("auto-send"),
            "unexpected err: {msg}"
        );
        // Status is still pending.
        let d = q.get(id).unwrap().unwrap();
        assert_eq!(d.status, DraftStatus::Pending);
    }

    #[test]
    fn approve_then_mark_sent_transitions_cleanly() {
        let (_d, q) = open_queue();
        let id = q.draft("linkedin", "carol", None, "body").unwrap();
        q.approve(id).unwrap();
        let d = q.get(id).unwrap().unwrap();
        assert_eq!(d.status, DraftStatus::Approved);
        assert!(d.approved_at.is_some());

        q.mark_sent(id).unwrap();
        let d = q.get(id).unwrap().unwrap();
        assert_eq!(d.status, DraftStatus::Sent);
        assert!(d.sent_at.is_some());
    }

    #[test]
    fn approve_is_idempotent_guard_returns_error_on_double_approve() {
        let (_d, q) = open_queue();
        let id = q.draft("email", "dave", None, "body").unwrap();
        q.approve(id).unwrap();
        assert!(q.approve(id).is_err(), "double approve should fail");
    }

    #[test]
    fn reject_removes_draft_from_pending_list() {
        let (_d, q) = open_queue();
        let id = q.draft("email", "eve", None, "body").unwrap();
        q.reject(id, "looks like spam").unwrap();
        assert!(q.pending().unwrap().is_empty());
        let d = q.get(id).unwrap().unwrap();
        assert_eq!(d.status, DraftStatus::Rejected);
        assert_eq!(d.reject_reason.as_deref(), Some("looks like spam"));
        // Can't resurrect a rejected draft.
        assert!(q.approve(id).is_err());
    }

    #[test]
    fn stats_counts_by_status() {
        let (_d, q) = open_queue();
        let a = q.draft("email", "a", None, "x").unwrap();
        let b = q.draft("email", "b", None, "y").unwrap();
        let c = q.draft("email", "c", None, "z").unwrap();
        q.approve(a).unwrap();
        q.mark_sent(a).unwrap();
        q.reject(b, "nope").unwrap();
        // c stays pending.
        let _ = c;
        let s = q.stats().unwrap();
        assert_eq!(s.pending, 1);
        assert_eq!(s.approved, 0);
        assert_eq!(s.sent, 1);
        assert_eq!(s.rejected, 1);
        assert_eq!(s.total, 3);
    }

    #[test]
    fn input_validation_rejects_empty_fields() {
        let (_d, q) = open_queue();
        assert!(q.draft("", "x", None, "y").is_err());
        assert!(q.draft("email", "", None, "y").is_err());
        assert!(q.draft("email", "x", None, "").is_err());
    }
}
