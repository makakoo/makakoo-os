//! Swarm artifact store — append-only typed log of plans / results / logs /
//! checkpoints produced by subagents during a swarm run.
//!
//! Distinct from the Python `ArtifactStore` at `core/orchestration/artifact_store.py`
//! (which is a name-versioned key/value store used for Phase 1.5 shared state).
//! This store is scoped to *swarm runs*: every row carries a `run_id` so a
//! dispatcher can fetch the full trace of a dispatch.
//!
//! Schema lives in `db.rs` under `swarm_artifacts` (added in SCHEMA_V1 as the
//! Tier-C extension). Shape:
//!
//! ```text
//! CREATE TABLE swarm_artifacts (
//!     id            INTEGER PRIMARY KEY AUTOINCREMENT,
//!     kind          TEXT NOT NULL,   -- plan | result | log | checkpoint
//!     run_id        TEXT NOT NULL,
//!     parent_id     INTEGER,
//!     agent         TEXT NOT NULL,
//!     content       TEXT NOT NULL,
//!     metadata_json TEXT,
//!     created_at    TEXT NOT NULL
//! );
//! ```
//!
//! Thread-safe: the inner connection is wrapped in `Arc<Mutex<Connection>>`
//! matching every other makakoo-core subsystem.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{MakakooError, Result};

/// Artifact kind — typed enum over the stored `kind` TEXT column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    /// Upfront plan produced by a planner before execution.
    Plan,
    /// Final result of a subagent run.
    Result,
    /// Progress log line — typically `tracing`-style.
    Log,
    /// Mid-run checkpoint a resumable agent can restart from.
    Checkpoint,
}

impl ArtifactKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactKind::Plan => "plan",
            ArtifactKind::Result => "result",
            ArtifactKind::Log => "log",
            ArtifactKind::Checkpoint => "checkpoint",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "plan" => Ok(ArtifactKind::Plan),
            "result" => Ok(ArtifactKind::Result),
            "log" => Ok(ArtifactKind::Log),
            "checkpoint" => Ok(ArtifactKind::Checkpoint),
            other => Err(MakakooError::internal(format!(
                "unknown artifact kind: {other}"
            ))),
        }
    }
}

/// One row out of `swarm_artifacts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: i64,
    pub kind: ArtifactKind,
    pub run_id: String,
    pub parent_id: Option<i64>,
    pub agent: String,
    pub content: String,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

/// SQLite-backed append-only log of swarm artifacts.
#[derive(Clone)]
pub struct ArtifactStore {
    conn: Arc<Mutex<Connection>>,
}

impl ArtifactStore {
    /// Wrap an existing shared connection. Caller has already run
    /// `db::run_migrations` so the `swarm_artifacts` table exists.
    pub fn open(conn: Arc<Mutex<Connection>>) -> Result<Self> {
        Ok(Self { conn })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| MakakooError::internal("swarm artifact store mutex poisoned"))
    }

    /// Insert an artifact. The `id` and `created_at` fields on the passed
    /// struct are ignored — they come back from SQLite. Returns the new id.
    pub fn write(&self, a: Artifact) -> Result<i64> {
        let conn = self.lock()?;
        let metadata_str = serde_json::to_string(&a.metadata)?;
        let ts = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO swarm_artifacts
                (kind, run_id, parent_id, agent, content, metadata_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                a.kind.as_str(),
                a.run_id,
                a.parent_id,
                a.agent,
                a.content,
                metadata_str,
                ts,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// All artifacts for a given run, oldest first.
    pub fn by_run(&self, run_id: &str) -> Result<Vec<Artifact>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, run_id, parent_id, agent, content, metadata_json, created_at
             FROM swarm_artifacts WHERE run_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![run_id], row_to_artifact)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(MakakooError::from)?);
        }
        Ok(out)
    }

    /// Most recent `limit` artifacts of the given kind, newest first.
    pub fn by_kind(&self, kind: ArtifactKind, limit: usize) -> Result<Vec<Artifact>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, run_id, parent_id, agent, content, metadata_json, created_at
             FROM swarm_artifacts WHERE kind = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![kind.as_str(), limit as i64], row_to_artifact)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(MakakooError::from)?);
        }
        Ok(out)
    }

    /// Most recent artifact of a given kind for a run, or `None`.
    pub fn latest(&self, run_id: &str, kind: ArtifactKind) -> Result<Option<Artifact>> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT id, kind, run_id, parent_id, agent, content, metadata_json, created_at
             FROM swarm_artifacts
             WHERE run_id = ?1 AND kind = ?2
             ORDER BY id DESC LIMIT 1",
            params![run_id, kind.as_str()],
            row_to_artifact,
        )
        .optional()
        .map_err(MakakooError::from)
    }

    /// Total artifact count. Cheap diagnostic.
    pub fn count(&self) -> Result<i64> {
        let conn = self.lock()?;
        let n: i64 =
            conn.query_row("SELECT COUNT(*) FROM swarm_artifacts", params![], |row| {
                row.get(0)
            })?;
        Ok(n)
    }
}

fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    let id: i64 = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let run_id: String = row.get(2)?;
    let parent_id: Option<i64> = row.get(3)?;
    let agent: String = row.get(4)?;
    let content: String = row.get(5)?;
    let metadata_raw: Option<String> = row.get(6)?;
    let created_at_raw: String = row.get(7)?;

    let kind = ArtifactKind::parse(&kind_str).unwrap_or(ArtifactKind::Log);
    let metadata = metadata_raw
        .as_deref()
        .map(|s| serde_json::from_str::<Value>(s).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    let created_at = DateTime::parse_from_rfc3339(&created_at_raw)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    Ok(Artifact {
        id,
        kind,
        run_id,
        parent_id,
        agent,
        content,
        metadata,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use serde_json::json;
    use std::sync::Mutex;

    fn tmp_store() -> (tempfile::TempDir, ArtifactStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swarm.db");
        let conn = open_db(&path).unwrap();
        run_migrations(&conn).unwrap();
        let store = ArtifactStore::open(Arc::new(Mutex::new(conn))).unwrap();
        (dir, store)
    }

    fn mk(kind: ArtifactKind, run: &str, agent: &str, content: &str) -> Artifact {
        Artifact {
            id: 0,
            kind,
            run_id: run.to_string(),
            parent_id: None,
            agent: agent.to_string(),
            content: content.to_string(),
            metadata: json!({"note": "test"}),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn write_assigns_monotonic_id() {
        let (_dir, store) = tmp_store();
        let id1 = store
            .write(mk(ArtifactKind::Plan, "run-1", "planner", "plan body"))
            .unwrap();
        let id2 = store
            .write(mk(ArtifactKind::Result, "run-1", "worker", "result body"))
            .unwrap();
        assert!(id2 > id1);
    }

    #[test]
    fn by_run_returns_in_insertion_order() {
        let (_dir, store) = tmp_store();
        store
            .write(mk(ArtifactKind::Plan, "r-a", "p", "first"))
            .unwrap();
        store
            .write(mk(ArtifactKind::Log, "r-a", "p", "second"))
            .unwrap();
        store
            .write(mk(ArtifactKind::Result, "r-b", "p", "other run"))
            .unwrap();
        let run_a = store.by_run("r-a").unwrap();
        assert_eq!(run_a.len(), 2);
        assert_eq!(run_a[0].content, "first");
        assert_eq!(run_a[1].content, "second");
    }

    #[test]
    fn by_kind_returns_newest_first() {
        let (_dir, store) = tmp_store();
        store
            .write(mk(ArtifactKind::Log, "r-1", "a", "log 1"))
            .unwrap();
        store
            .write(mk(ArtifactKind::Log, "r-1", "a", "log 2"))
            .unwrap();
        store
            .write(mk(ArtifactKind::Log, "r-2", "a", "log 3"))
            .unwrap();
        let hits = store.by_kind(ArtifactKind::Log, 5).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].content, "log 3");
        assert_eq!(hits[2].content, "log 1");
    }

    #[test]
    fn latest_finds_most_recent_of_kind() {
        let (_dir, store) = tmp_store();
        store
            .write(mk(ArtifactKind::Checkpoint, "r", "a", "cp1"))
            .unwrap();
        store
            .write(mk(ArtifactKind::Result, "r", "a", "r1"))
            .unwrap();
        store
            .write(mk(ArtifactKind::Checkpoint, "r", "a", "cp2"))
            .unwrap();
        let latest = store.latest("r", ArtifactKind::Checkpoint).unwrap().unwrap();
        assert_eq!(latest.content, "cp2");
    }

    #[test]
    fn roundtrip_metadata_preserves_json() {
        let (_dir, store) = tmp_store();
        let mut a = mk(ArtifactKind::Result, "run-x", "agent", "body");
        a.metadata = json!({"score": 0.97, "tags": ["good", "done"]});
        let id = store.write(a).unwrap();
        let got = store
            .by_run("run-x")
            .unwrap()
            .into_iter()
            .find(|x| x.id == id)
            .unwrap();
        assert_eq!(got.metadata["score"].as_f64().unwrap(), 0.97);
        assert_eq!(got.metadata["tags"][0].as_str().unwrap(), "good");
    }

    #[test]
    fn kind_parse_round_trip() {
        for k in [
            ArtifactKind::Plan,
            ArtifactKind::Result,
            ArtifactKind::Log,
            ArtifactKind::Checkpoint,
        ] {
            assert_eq!(ArtifactKind::parse(k.as_str()).unwrap(), k);
        }
        assert!(ArtifactKind::parse("nope").is_err());
    }
}
