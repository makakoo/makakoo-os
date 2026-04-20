//! Email adapter — Gmail API via a Python plugin subprocess bridge.
//!
//! The kernel doesn't speak SMTP / IMAP / Gmail-OAuth directly. Sending
//! email is delegated to the `skill-infrastructure-gmail-send` plugin,
//! which the `makakoo skill run` bridge invokes out-of-process. This
//! matches the rest of Harvey's "send" layer — Python handles OAuth and
//! token refresh, Rust handles the durable queue + audit trail.
//!
//! Contract with the plugin:
//!   * executable: `$MAKAKOO_HOME/plugins/skill-infrastructure-gmail-send/src/send.py`
//!   * stdin: JSON `{"to": "...", "subject": "...", "body": "..."}`
//!   * stdout: JSON `{"ok": true, "message_id": "..."}` on success,
//!             `{"ok": false, "error": "..."}` on failure
//!   * exit 0 on JSON success, exit != 0 on fatal error
//!
//! The plugin doesn't ship in v0.2 — this adapter is the call site so
//! once the plugin lands it plugs straight in.

use std::process::Stdio;

use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::{MakakooError, Result};
use crate::outbound::{Draft, DraftStatus, OutboundQueue};

pub struct EmailAdapter;

impl EmailAdapter {
    /// Send an approved draft via the gmail-send plugin subprocess.
    pub async fn send_approved(queue: &OutboundQueue, draft: &Draft) -> Result<()> {
        if draft.status != DraftStatus::Approved {
            return Err(MakakooError::InvalidInput(format!(
                "email: draft {} not approved (status={:?})",
                draft.id, draft.status,
            )));
        }
        if draft.channel != "email" {
            return Err(MakakooError::InvalidInput(format!(
                "email: draft {} is channel={:?}, not 'email'",
                draft.id, draft.channel,
            )));
        }

        let home = crate::platform::makakoo_home();
        let script = home
            .join("plugins")
            .join("skill-infrastructure-gmail-send")
            .join("src")
            .join("send.py");
        if !script.exists() {
            return Err(MakakooError::internal(format!(
                "email: gmail-send plugin missing at {}. \
                 Install via `makakoo plugin install skill-infrastructure-gmail-send`.",
                script.display()
            )));
        }

        let payload = json!({
            "to": draft.recipient,
            "subject": draft.subject.clone().unwrap_or_default(),
            "body": draft.body,
        });

        let mut child = Command::new("python3")
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| MakakooError::internal(format!("email: spawn python: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            let bytes = serde_json::to_vec(&payload)
                .map_err(|e| MakakooError::internal(format!("email: encode payload: {e}")))?;
            stdin
                .write_all(&bytes)
                .await
                .map_err(|e| MakakooError::internal(format!("email: stdin write: {e}")))?;
        }

        let out = child
            .wait_with_output()
            .await
            .map_err(|e| MakakooError::internal(format!("email: wait: {e}")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(MakakooError::internal(format!(
                "email: gmail-send exited {}: {}",
                out.status, stderr.trim()
            )));
        }
        // Parse success payload — the plugin should emit {"ok": true, ...}.
        let reply: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| {
            MakakooError::internal(format!("email: gmail-send returned non-JSON: {e}"))
        })?;
        if reply.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let err = reply
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            return Err(MakakooError::internal(format!(
                "email: gmail-send reported failure: {err}"
            )));
        }

        queue.mark_sent(draft.id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn send_approved_refuses_unapproved_draft() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(&dir.path().join("t.db")).unwrap();
        run_migrations(&conn).unwrap();
        let q = OutboundQueue::open(Arc::new(Mutex::new(conn))).unwrap();
        let id = q.draft("email", "a@b.c", Some("hi"), "body").unwrap();
        let d = q.get(id).unwrap().unwrap();
        let err = EmailAdapter::send_approved(&q, &d).await.unwrap_err();
        assert!(matches!(err, MakakooError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn send_approved_errors_when_plugin_missing() {
        // Point MAKAKOO_HOME at a tempdir with no plugin installed.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let conn = open_db(&dir.path().join("t.db")).unwrap();
        run_migrations(&conn).unwrap();
        let q = OutboundQueue::open(Arc::new(Mutex::new(conn))).unwrap();
        let id = q.draft("email", "a@b.c", Some("hi"), "body").unwrap();
        q.approve(id).unwrap();
        let d = q.get(id).unwrap().unwrap();
        let err = EmailAdapter::send_approved(&q, &d).await.unwrap_err();
        let s = err.to_string();
        assert!(s.contains("gmail-send plugin missing"), "got: {s}");
        std::env::remove_var("MAKAKOO_HOME");
    }
}
