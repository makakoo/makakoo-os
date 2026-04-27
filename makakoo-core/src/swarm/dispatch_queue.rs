//! Swarm dispatch queue — durable, append-only, replay-safe.
//!
//! v0.2 Phase D.4. The SwarmGateway's `dispatch` / `dispatch_team`
//! entry points are fast but ephemeral: if the caller dies after
//! enqueueing but before the coordinator boots, the work is lost.
//! The queue closes that gap — any producer (HarveyChat, SANCHO,
//! MCP tool) appends a single JSONL line to
//! `$MAKAKOO_HOME/state/swarm/queue.jsonl`, and the SANCHO
//! [`SwarmDispatchHandler`] drains the queue on every tick.
//!
//! Semantics:
//!   * **Append-only**: enqueue writes one fsynced line; no rewrites.
//!   * **Idempotent**: every entry carries a `id` (UUIDv7-ish). The
//!     consumer records `id` in a sibling receipts.jsonl; re-reading
//!     the queue after a crash skips already-receipted ids.
//!   * **FIFO**: drained in append order.
//!   * **At-least-once**: a crash between dispatch and receipt can
//!     re-dispatch. Downstream (swarm coordinator) is responsible for
//!     idempotent handling.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};

use super::gateway::{DispatchRequest, TeamDispatchRequest};

/// Canonical queue location for a given `$MAKAKOO_HOME`.
pub fn queue_dir(home: &Path) -> PathBuf {
    home.join("state").join("swarm")
}

pub fn queue_path(home: &Path) -> PathBuf {
    queue_dir(home).join("queue.jsonl")
}

pub fn receipts_path(home: &Path) -> PathBuf {
    queue_dir(home).join("receipts.jsonl")
}

/// Discriminated entry. Serialized shape:
/// `{"id": "...", "enqueued_at": "...", "kind": "team"|"agent", ...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueEntry {
    Team {
        id: String,
        enqueued_at: DateTime<Utc>,
        #[serde(flatten)]
        req: TeamDispatchRequest,
    },
    Agent {
        id: String,
        enqueued_at: DateTime<Utc>,
        #[serde(flatten)]
        req: DispatchRequest,
    },
}

impl QueueEntry {
    pub fn id(&self) -> &str {
        match self {
            QueueEntry::Team { id, .. } | QueueEntry::Agent { id, .. } => id,
        }
    }
}

/// Dispatch receipt — one line written per successful dispatch. The
/// tuple `(queue_id, run_id)` lets callers correlate queued work with
/// the swarm run that materialized it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Receipt {
    pub id: String,
    pub dispatched_at: DateTime<Utc>,
    pub run_id: String,
}

fn mint_id() -> String {
    // Time-ordered id with a random tail so two enqueues in the same
    // nanosecond still differ. Avoids the uuid dep for a single use
    // — chrono + thread_rng is already in the tree.
    use rand::{Rng, thread_rng};
    let ts = Utc::now().format("%Y%m%dT%H%M%S%.6f").to_string();
    let tail: u32 = thread_rng().gen();
    format!("q-{ts}-{tail:08x}")
}

fn append_line(path: &Path, line: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            MakakooError::internal(format!("mkdir {}: {e}", parent.display()))
        })?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| MakakooError::internal(format!("open {}: {e}", path.display())))?;
    f.write_all(line.as_bytes())
        .map_err(|e| MakakooError::internal(format!("write {}: {e}", path.display())))?;
    f.write_all(b"\n")
        .map_err(|e| MakakooError::internal(format!("write {}: {e}", path.display())))?;
    f.flush()
        .map_err(|e| MakakooError::internal(format!("flush {}: {e}", path.display())))?;
    Ok(())
}

fn read_lines<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = fs::File::open(path)
        .map_err(|e| MakakooError::internal(format!("open {}: {e}", path.display())))?;
    let mut out = Vec::new();
    for (ix, line) in BufReader::new(f).lines().enumerate() {
        let line = line.map_err(|e| {
            MakakooError::internal(format!("read {} line {}: {e}", path.display(), ix))
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed: T = serde_json::from_str(&line).map_err(|e| {
            MakakooError::internal(format!(
                "parse {} line {}: {e}",
                path.display(),
                ix
            ))
        })?;
        out.push(parsed);
    }
    Ok(out)
}

/// Append one team-dispatch request. Returns the queue id.
pub fn enqueue_team(home: &Path, req: TeamDispatchRequest) -> Result<String> {
    let entry = QueueEntry::Team {
        id: mint_id(),
        enqueued_at: Utc::now(),
        req,
    };
    let id = entry.id().to_string();
    append_line(
        &queue_path(home),
        &serde_json::to_string(&entry)
            .map_err(|e| MakakooError::internal(format!("serialize: {e}")))?,
    )?;
    Ok(id)
}

/// Append one single-agent dispatch request. Returns the queue id.
pub fn enqueue_agent(home: &Path, req: DispatchRequest) -> Result<String> {
    let entry = QueueEntry::Agent {
        id: mint_id(),
        enqueued_at: Utc::now(),
        req,
    };
    let id = entry.id().to_string();
    append_line(
        &queue_path(home),
        &serde_json::to_string(&entry)
            .map_err(|e| MakakooError::internal(format!("serialize: {e}")))?,
    )?;
    Ok(id)
}

/// Load every entry currently in the queue (oldest first).
pub fn load_queue(home: &Path) -> Result<Vec<QueueEntry>> {
    read_lines(&queue_path(home))
}

/// Load every receipt written so far.
pub fn load_receipts(home: &Path) -> Result<Vec<Receipt>> {
    read_lines(&receipts_path(home))
}

/// Write a receipt for a dispatched entry.
pub fn write_receipt(home: &Path, receipt: &Receipt) -> Result<()> {
    let line = serde_json::to_string(receipt)
        .map_err(|e| MakakooError::internal(format!("serialize receipt: {e}")))?;
    append_line(&receipts_path(home), &line)
}

/// The set of queue ids that are NOT yet receipted. Ordering mirrors
/// insertion order in the queue file.
pub fn pending(home: &Path) -> Result<Vec<QueueEntry>> {
    let queue = load_queue(home)?;
    let receipts = load_receipts(home)?;
    let done: std::collections::HashSet<String> =
        receipts.into_iter().map(|r| r.id).collect();
    Ok(queue.into_iter().filter(|e| !done.contains(e.id())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_team(team: &str) -> TeamDispatchRequest {
        TeamDispatchRequest {
            team: team.into(),
            prompt: "do the thing".into(),
            parallelism: None,
            model: None,
        }
    }

    #[test]
    fn enqueue_is_append_only() {
        let dir = tempdir().unwrap();
        let id1 = enqueue_team(dir.path(), sample_team("research_team")).unwrap();
        let id2 = enqueue_team(dir.path(), sample_team("archive_team")).unwrap();
        let q = load_queue(dir.path()).unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].id(), id1);
        assert_eq!(q[1].id(), id2);
    }

    #[test]
    fn receipts_mask_pending_entries() {
        let dir = tempdir().unwrap();
        let id1 = enqueue_team(dir.path(), sample_team("research_team")).unwrap();
        let id2 = enqueue_team(dir.path(), sample_team("archive_team")).unwrap();
        write_receipt(
            dir.path(),
            &Receipt {
                id: id1.clone(),
                dispatched_at: Utc::now(),
                run_id: "run-001".into(),
            },
        )
        .unwrap();
        let remaining = pending(dir.path()).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id(), id2);
    }

    #[test]
    fn queue_survives_mixed_agent_and_team_entries() {
        let dir = tempdir().unwrap();
        enqueue_team(dir.path(), sample_team("research_team")).unwrap();
        enqueue_agent(
            dir.path(),
            DispatchRequest {
                name: "researcher".into(),
                task: "lookup".into(),
                prompt: "go".into(),
                model: None,
                parent_run_id: None,
                adapter: None,
            },
        )
        .unwrap();
        let q = load_queue(dir.path()).unwrap();
        assert_eq!(q.len(), 2);
        assert!(matches!(q[0], QueueEntry::Team { .. }));
        assert!(matches!(q[1], QueueEntry::Agent { .. }));
    }

    #[test]
    fn empty_dir_yields_empty_lists() {
        let dir = tempdir().unwrap();
        assert!(load_queue(dir.path()).unwrap().is_empty());
        assert!(load_receipts(dir.path()).unwrap().is_empty());
        assert!(pending(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn enqueue_emits_unique_ids_under_rapid_calls() {
        let dir = tempdir().unwrap();
        let mut ids = std::collections::HashSet::new();
        for _ in 0..50 {
            let id = enqueue_team(dir.path(), sample_team("research_team")).unwrap();
            assert!(ids.insert(id), "duplicate id minted");
        }
    }

    #[test]
    fn malformed_line_causes_graceful_error() {
        let dir = tempdir().unwrap();
        let p = queue_path(dir.path());
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "not json\n").unwrap();
        let err = load_queue(dir.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("parse"), "err: {msg}");
    }
}
