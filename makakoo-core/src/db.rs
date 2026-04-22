//! Rusqlite setup + idempotent schema migrations.
//!
//! Schemas are byte-reproduced from the Python source of truth so the
//! Rust rewrite and Python implementation can share the same acceptance
//! tests. Each table below has its Python source cited inline.
//!
//! Schema version lives in `PRAGMA user_version`. Wave 1 ships version 1:
//! the unified superset of every sqlite schema the user's install
//! currently runs. Later waves bump the version and layer ALTER TABLEs
//! for fields that don't exist yet.

use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;

pub const SCHEMA_VERSION: i64 = 1;

/// Open a SQLite database in WAL mode with sensible defaults.
pub fn open_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    // Several PRAGMAs (notably `journal_mode`, `busy_timeout`) return
    // the applied value as a result row. rusqlite's `execute_batch` and
    // `pragma_update` both reject row-returning statements, so we step
    // through them via `query` and drop the rows.
    for stmt in [
        "PRAGMA journal_mode = WAL",
        "PRAGMA synchronous = NORMAL",
        "PRAGMA busy_timeout = 5000",
        "PRAGMA foreign_keys = ON",
    ] {
        let mut s = conn.prepare(stmt)?;
        let mut rows = s.query([])?;
        // Drain any returned rows; don't care about the value.
        while rows.next()?.is_some() {}
    }
    Ok(conn)
}

/// Run idempotent migrations. Safe to call on every boot.
pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_V1)?;
    heal_legacy_schema_drift(conn)?;
    // `PRAGMA user_version = N` is a no-row PRAGMA on most builds but
    // we run it through the same drain-rows path as `open_db` for
    // forward-compat with rusqlite strictness changes.
    let sql = format!("PRAGMA user_version = {SCHEMA_VERSION}");
    let mut s = conn.prepare(&sql)?;
    let mut rows = s.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// Drop Python-era triggers that shadow the external-content FTS5 tables.
///
/// The Python implementation maintained `brain_fts` / `brain_anchors_fts`
/// with explicit AFTER INSERT/UPDATE/DELETE triggers on `brain_docs`. The
/// Rust store writes FTS rows directly in `write_document`, so the old
/// triggers double-insert the same rowid into the FTS shadow table and
/// crash with `PRIMARY KEY constraint failed` (SQLite 1555). DBs created
/// under SCHEMA_V1 never have these triggers; DBs migrated from Python do.
/// Drop-if-exists is cheap and idempotent.
fn heal_legacy_schema_drift(conn: &Connection) -> Result<()> {
    for trig in [
        "brain_docs_ai",
        "brain_docs_ad",
        "brain_docs_au",
        "brain_docs_anchors_ai",
        "brain_docs_anchors_ad",
        "brain_docs_anchors_au",
    ] {
        conn.execute(&format!("DROP TRIGGER IF EXISTS {trig}"), [])?;
    }
    Ok(())
}

/// Read the current schema version from `PRAGMA user_version`.
pub fn schema_version(conn: &Connection) -> Result<i64> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(v)
}

// ─────────────────────────────────────────────────────────────────────
// SCHEMA_V1 — superset reproducing the Python schemas verbatim.
// Sources:
//   brain_docs / brain_fts / brain_vectors / entity_graph / events /
//     cache / recall_log / recall_stats:
//       plugins-core/lib-harvey-core/src/core/superbrain/store.py:109-227
//   brain_anchors_fts:
//       plugins-core/lib-harvey-core/src/core/superbrain/migrations/001_add_anchor_columns.py:36
//   brain_anchor_vectors:
//       plugins-core/lib-harvey-core/src/core/superbrain/migrations/002_add_anchor_vectors.py:26
//   artifacts:
//       plugins-core/lib-harvey-core/src/core/orchestration/artifact_store.py:119
//   bus_events (persistent event bus; renamed to avoid clash with
//     superbrain.events):
//       plugins-core/lib-harvey-core/src/core/orchestration/persistent_event_bus.py:97
//   chat_messages / chat_sessions (renamed from messages/sessions to
//     avoid clash with brain tables):
//       plugins-core/lib-harvey-core/src/core/chat/store.py:28,41
//   chat_tasks (renamed from tasks):
//       plugins-core/lib-harvey-core/src/core/chat/task_queue.py:139
//   chat_cooldowns:
//       plugins-core/lib-harvey-core/src/core/chat/channels/cooldowns.py:50
//   agents:
//       plugins-core/lib-harvey-core/src/core/orchestration/agent_discovery/store.py:32
//
// IMPORTANT: table column names match the Python implementation exactly.
// Where a table name conflicts across Python modules (each module opens
// its own sqlite file, so "events" means different things in
// superbrain.store vs persistent_event_bus), we namespace here because
// makakoo-core targets a single unified DB file.
// ─────────────────────────────────────────────────────────────────────
const SCHEMA_V1: &str = r#"
-- ─── superbrain ───────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS brain_docs (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    doc_type TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    entities TEXT DEFAULT '[]',
    char_count INTEGER DEFAULT 0,
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE VIRTUAL TABLE IF NOT EXISTS brain_fts USING fts5(
    name, content, entities,
    content=brain_docs,
    content_rowid=id,
    tokenize='porter unicode61'
);

-- FTS5 with content= automatically synchronizes the index on INSERT/UPDATE/DELETE.
-- No manual triggers needed. The old triggers used invalid syntax for FTS5 virtual tables.

CREATE TABLE IF NOT EXISTS brain_vectors (
    doc_id INTEGER PRIMARY KEY REFERENCES brain_docs(id),
    embedding BLOB NOT NULL,
    dim INTEGER NOT NULL,
    model TEXT DEFAULT 'unknown',
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE VIRTUAL TABLE IF NOT EXISTS brain_anchors_fts USING fts5(
    anchor, context, doc_path,
    tokenize='porter unicode61'
);

CREATE TABLE IF NOT EXISTS brain_anchor_vectors (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    anchor_hash TEXT NOT NULL,
    doc_id INTEGER,
    embedding BLOB NOT NULL,
    dim INTEGER NOT NULL,
    model TEXT NOT NULL,
    created_at TEXT DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_brain_anchor_vectors_model
    ON brain_anchor_vectors(model);
CREATE INDEX IF NOT EXISTS idx_brain_anchor_vectors_anchor_hash
    ON brain_anchor_vectors(anchor_hash);

CREATE TABLE IF NOT EXISTS entity_graph (
    id INTEGER PRIMARY KEY,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    valid_from TEXT,
    valid_to TEXT,
    confidence REAL DEFAULT 1.0,
    source TEXT,
    created_at TEXT DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS eg_subject ON entity_graph(subject);
CREATE INDEX IF NOT EXISTS eg_object ON entity_graph(object);

-- ─── superbrain_events ──────────────────────────────────────────────
-- T18 — schema drift fix. Originally this table was called `events`, a
-- direct port of Python's `core/superbrain/store.py` events table. But
-- the user's live `data/events.db` (written by the Python
-- PersistentEventBus at `core/orchestration/persistent_event_bus.py`) ALSO
-- has a table called `events`, with a completely different shape:
-- `(seq, topic, source, data, timestamp)`. When Rust runs migrations
-- against a pre-existing events.db, `CREATE TABLE IF NOT EXISTS events`
-- sees the legacy table and skips, then
-- `CREATE INDEX ... ON events(event_type, ...)` crashes with
-- "no such column: event_type". That's the "swarm subsystem unavailable"
-- error reported by `makakoo-mcp --health`.
--
-- Fix: rename the Rust-side table to `superbrain_events` so it can never
-- collide with the legacy Python PersistentEventBus table name. The
-- `bus_events` table (Rust PersistentEventBus canonical) is already
-- collision-free. Historical events from the legacy file remain
-- reachable only through the Python reader — migrating them is deferred
-- to a dedicated sprint.
CREATE TABLE IF NOT EXISTS superbrain_events (
    id INTEGER PRIMARY KEY,
    event_type TEXT NOT NULL,
    agent TEXT NOT NULL,
    summary TEXT NOT NULL,
    details TEXT DEFAULT '{}',
    occurred_at TEXT DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_superbrain_events_type
    ON superbrain_events(event_type, occurred_at);

CREATE TABLE IF NOT EXISTS cache (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    expires_at TEXT
);

CREATE TABLE IF NOT EXISTS recall_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id INTEGER NOT NULL,
    doc_path TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL,
    query_hash TEXT NOT NULL,
    score REAL NOT NULL DEFAULT 0.0,
    source TEXT NOT NULL DEFAULT 'search',
    recalled_at TEXT NOT NULL DEFAULT (datetime('now')),
    recall_day TEXT NOT NULL DEFAULT (date('now'))
);
CREATE INDEX IF NOT EXISTS idx_recall_doc_id ON recall_log(doc_id);
CREATE INDEX IF NOT EXISTS idx_recall_content_hash ON recall_log(content_hash);
CREATE INDEX IF NOT EXISTS idx_recall_day ON recall_log(recall_day);
CREATE INDEX IF NOT EXISTS idx_recall_query_hash ON recall_log(query_hash);

CREATE TABLE IF NOT EXISTS recall_stats (
    content_hash TEXT PRIMARY KEY,
    doc_id INTEGER NOT NULL,
    doc_path TEXT NOT NULL DEFAULT '',
    snippet TEXT DEFAULT '',
    recall_count INTEGER DEFAULT 0,
    unique_queries INTEGER DEFAULT 0,
    unique_days INTEGER DEFAULT 0,
    total_score REAL DEFAULT 0.0,
    max_score REAL DEFAULT 0.0,
    first_recalled_at TEXT,
    last_recalled_at TEXT,
    consolidation_hits INTEGER DEFAULT 0,
    promoted_at TEXT,
    concept_tags TEXT DEFAULT '[]'
);

-- ─── memory promotions (wave 2 fills behaviour; table lives here) ──
CREATE TABLE IF NOT EXISTS memory_promotions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content_hash TEXT NOT NULL,
    doc_id INTEGER,
    doc_path TEXT NOT NULL DEFAULT '',
    promoted_at TEXT NOT NULL DEFAULT (datetime('now')),
    reason TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_memory_promotions_content_hash
    ON memory_promotions(content_hash);

-- ─── orchestration ────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS artifacts (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    producer        TEXT NOT NULL,
    payload         TEXT NOT NULL,
    depends_on      TEXT NOT NULL DEFAULT '[]',
    created_at      REAL NOT NULL,
    ttl_seconds     INTEGER NOT NULL DEFAULT 86400,
    version         INTEGER NOT NULL DEFAULT 1,
    pinned          INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_artifacts_name
    ON artifacts(name, version DESC);
CREATE INDEX IF NOT EXISTS idx_artifacts_producer
    ON artifacts(producer);
CREATE INDEX IF NOT EXISTS idx_artifacts_created
    ON artifacts(created_at);

CREATE TABLE IF NOT EXISTS bus_events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    topic       TEXT NOT NULL,
    source      TEXT NOT NULL DEFAULT '',
    data        TEXT NOT NULL,
    timestamp   REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_bus_events_topic
    ON bus_events(topic, seq);
CREATE INDEX IF NOT EXISTS idx_bus_events_timestamp
    ON bus_events(timestamp);

CREATE TABLE IF NOT EXISTS agents (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    status          TEXT NOT NULL,
    lease_expires_at REAL,
    metadata        TEXT NOT NULL DEFAULT '{}',
    registered_at   REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lease ON agents(lease_expires_at);

-- ─── chat ─────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS chat_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    channel_user_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    metadata TEXT DEFAULT '{}',
    created_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chat_messages_channel_user
    ON chat_messages(channel, channel_user_id, created_at);

CREATE TABLE IF NOT EXISTS chat_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    channel_user_id TEXT NOT NULL,
    started_at REAL NOT NULL,
    last_active REAL NOT NULL,
    message_count INTEGER DEFAULT 0,
    metadata TEXT DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_channel_user
    ON chat_sessions(channel, channel_user_id);

CREATE TABLE IF NOT EXISTS chat_tasks (
    id TEXT PRIMARY KEY,
    channel TEXT NOT NULL,
    user_id TEXT NOT NULL,
    goal TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at REAL NOT NULL,
    started_at REAL,
    completed_at REAL,
    context TEXT DEFAULT '{}',
    messages TEXT DEFAULT '[]',
    current_goal TEXT DEFAULT '',
    awaiting_input_prompt TEXT DEFAULT '',
    progress_messages TEXT DEFAULT '[]',
    result TEXT DEFAULT '',
    files_to_send TEXT DEFAULT '[]',
    error TEXT DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_chat_tasks_user_active
    ON chat_tasks(channel, user_id, state);
CREATE INDEX IF NOT EXISTS idx_chat_tasks_created
    ON chat_tasks(created_at);

CREATE TABLE IF NOT EXISTS chat_cooldowns (
    channel TEXT NOT NULL,
    channel_user_id TEXT NOT NULL,
    expires_at REAL NOT NULL,
    reason TEXT DEFAULT '',
    PRIMARY KEY (channel, channel_user_id)
);
CREATE INDEX IF NOT EXISTS idx_chat_cooldowns_expires
    ON chat_cooldowns(expires_at);

-- ─── costs (T11 — CostTracker canonical schema) ───────────────────
-- Source of truth for the Rust `telemetry::costs::CostTracker`. Python's
-- `core/telemetry/cost_tracker.py` writes to a JSONL file — the Rust
-- port is authoritative for the sqlite shape. Columns mirror the Rust
-- `CostRecord` struct one-to-one.
CREATE TABLE IF NOT EXISTS costs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    occurred_at TEXT NOT NULL,
    agent TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    usd REAL NOT NULL DEFAULT 0.0,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_costs_occurred_at ON costs(occurred_at);
CREATE INDEX IF NOT EXISTS idx_costs_agent ON costs(agent);
CREATE INDEX IF NOT EXISTS idx_costs_model ON costs(model);

-- ─── outbound drafts (T11 — human-in-the-loop approval queue) ─────
-- Draft buffer for emails/LinkedIn/telegram. HARD RULE: status starts
-- as 'pending' and must pass through 'approved' before 'sent'. No
-- auto-send path exists in the Rust API — the OutboundQueue intentionally
-- does not expose a send() method. Channel adapters live elsewhere and
-- only accept drafts whose status is already 'approved'.
CREATE TABLE IF NOT EXISTS outbound_drafts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    recipient TEXT NOT NULL,
    subject TEXT,
    body TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL,
    approved_at TEXT,
    sent_at TEXT,
    reject_reason TEXT
);
CREATE INDEX IF NOT EXISTS idx_outbound_status
    ON outbound_drafts(status, created_at);
CREATE INDEX IF NOT EXISTS idx_outbound_channel
    ON outbound_drafts(channel, created_at);

-- ─── swarm (Tier-C subsystem — T15) ──────────────────────────────
-- Typed artifact log scoped to swarm runs. Separate from the legacy
-- `artifacts` table above (which is the name-versioned Phase 1.5 KV
-- store and is shaped very differently). Every row here carries a
-- `run_id` so a dispatcher can fetch the full trace of one dispatch.
CREATE TABLE IF NOT EXISTS swarm_artifacts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    kind          TEXT NOT NULL,
    run_id        TEXT NOT NULL,
    parent_id     INTEGER,
    agent         TEXT NOT NULL,
    content       TEXT NOT NULL,
    metadata_json TEXT,
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_swarm_artifacts_run
    ON swarm_artifacts(run_id);
CREATE INDEX IF NOT EXISTS idx_swarm_artifacts_kind
    ON swarm_artifacts(kind, id DESC);

-- ─── graph nodes/edges (wave 2 populates) ────────────────────────
CREATE TABLE IF NOT EXISTS brain_graph_nodes (
    id TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    node_type TEXT NOT NULL DEFAULT 'entity',
    degree INTEGER NOT NULL DEFAULT 0,
    metadata TEXT DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS brain_graph_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    src TEXT NOT NULL,
    dst TEXT NOT NULL,
    edge_type TEXT NOT NULL DEFAULT 'mentions',
    weight REAL NOT NULL DEFAULT 1.0,
    metadata TEXT DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_brain_graph_edges_src
    ON brain_graph_edges(src);
CREATE INDEX IF NOT EXISTS idx_brain_graph_edges_dst
    ON brain_graph_edges(dst);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = open_db(&path).unwrap();
        (dir, conn)
    }

    #[test]
    fn open_db_applies_wal_pragma() {
        let (_dir, conn) = tmp_db();
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn run_migrations_sets_user_version() {
        let (_dir, conn) = tmp_db();
        run_migrations(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn run_migrations_is_idempotent() {
        let (_dir, conn) = tmp_db();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn every_expected_table_exists() {
        let (_dir, conn) = tmp_db();
        run_migrations(&conn).unwrap();

        let tables = [
            "brain_docs",
            "brain_fts",
            "brain_vectors",
            "brain_anchors_fts",
            "brain_anchor_vectors",
            "entity_graph",
            "superbrain_events",
            "cache",
            "recall_log",
            "recall_stats",
            "memory_promotions",
            "artifacts",
            "bus_events",
            "agents",
            "chat_messages",
            "chat_sessions",
            "chat_tasks",
            "chat_cooldowns",
            "costs",
            "outbound_drafts",
            "swarm_artifacts",
            "brain_graph_nodes",
            "brain_graph_edges",
        ];
        for t in tables {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                    [t],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {t} missing after migrations");
        }
    }

    /// T18 regression — the user's live `data/events.db` has a legacy
    /// `events` table with the Python PersistentEventBus shape
    /// `(seq, topic, source, data, timestamp)`. Running migrations over
    /// that file used to fail with "no such column: event_type" because
    /// the old `events_type` index tried to reference `event_type` on
    /// the legacy table. After the rename to `superbrain_events`,
    /// migrations must succeed over a pre-existing legacy events DB and
    /// the `bus_events` table must also be queryable (T12 PersistentEventBus
    /// health check).
    #[test]
    fn migrations_survive_legacy_python_events_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        // Pre-seed the legacy Python shape.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE events (
                    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
                    topic       TEXT NOT NULL,
                    source      TEXT NOT NULL DEFAULT '',
                    data        TEXT NOT NULL,
                    timestamp   REAL NOT NULL
                );
                CREATE INDEX idx_events_topic ON events(topic, seq);
                INSERT INTO events (topic, source, data, timestamp)
                    VALUES ('sancho.tick', 'test', '{}', 1775000000.0);
                "#,
            )
            .unwrap();
        }
        // Now run Rust migrations over it — must not crash.
        let conn = open_db(&path).unwrap();
        run_migrations(&conn).unwrap();
        // Rust-side bus_events + superbrain_events must both exist now.
        for t in ["bus_events", "superbrain_events"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                    [t],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{t} must exist after migrations");
        }
        // Legacy events table still present and untouched — we did not
        // drop or alter it. Historical rows remain readable.
        let legacy_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(legacy_count, 1);
    }

    #[test]
    fn legacy_fts_triggers_get_dropped_by_migration() {
        // A DB carrying the Python-era brain_docs_ai/ad/au + _anchors_*
        // triggers must come out of run_migrations with all 6 gone.
        let (_dir, path) = tmp_db_path();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE brain_docs (
                    id INTEGER PRIMARY KEY,
                    path TEXT UNIQUE NOT NULL,
                    name TEXT NOT NULL,
                    doc_type TEXT NOT NULL,
                    content TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    entities TEXT DEFAULT '[]',
                    char_count INTEGER DEFAULT 0,
                    updated_at TEXT DEFAULT (datetime('now')),
                    anchor TEXT,
                    anchor_keywords TEXT,
                    anchor_entities TEXT
                 );
                 CREATE VIRTUAL TABLE brain_fts USING fts5(
                    name, content, entities,
                    content=brain_docs, content_rowid=id,
                    tokenize='porter unicode61');
                 CREATE VIRTUAL TABLE brain_anchors_fts USING fts5(
                    anchor, anchor_keywords, anchor_entities,
                    content='brain_docs', content_rowid='id',
                    tokenize='porter unicode61');
                 CREATE TRIGGER brain_docs_ai AFTER INSERT ON brain_docs BEGIN
                    INSERT INTO brain_fts(rowid, name, content, entities)
                    VALUES (new.id, new.name, new.content, new.entities);
                 END;
                 CREATE TRIGGER brain_docs_ad AFTER DELETE ON brain_docs BEGIN
                    INSERT INTO brain_fts(brain_fts, rowid, name, content, entities)
                    VALUES ('delete', old.id, old.name, old.content, old.entities);
                 END;
                 CREATE TRIGGER brain_docs_au AFTER UPDATE ON brain_docs BEGIN
                    INSERT INTO brain_fts(brain_fts, rowid, name, content, entities)
                    VALUES ('delete', old.id, old.name, old.content, old.entities);
                    INSERT INTO brain_fts(rowid, name, content, entities)
                    VALUES (new.id, new.name, new.content, new.entities);
                 END;
                 CREATE TRIGGER brain_docs_anchors_ai AFTER INSERT ON brain_docs BEGIN
                    INSERT INTO brain_anchors_fts(rowid, anchor, anchor_keywords, anchor_entities)
                    VALUES (new.id, new.anchor, new.anchor_keywords, new.anchor_entities);
                 END;
                 CREATE TRIGGER brain_docs_anchors_ad AFTER DELETE ON brain_docs BEGIN
                    INSERT INTO brain_anchors_fts(brain_anchors_fts, rowid, anchor, anchor_keywords, anchor_entities)
                    VALUES ('delete', old.id, old.anchor, old.anchor_keywords, old.anchor_entities);
                 END;
                 CREATE TRIGGER brain_docs_anchors_au AFTER UPDATE ON brain_docs BEGIN
                    INSERT INTO brain_anchors_fts(brain_anchors_fts, rowid, anchor, anchor_keywords, anchor_entities)
                    VALUES ('delete', old.id, old.anchor, old.anchor_keywords, old.anchor_entities);
                    INSERT INTO brain_anchors_fts(rowid, anchor, anchor_keywords, anchor_entities)
                    VALUES (new.id, new.anchor, new.anchor_keywords, new.anchor_entities);
                 END;",
            )
            .unwrap();
            let before: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'brain_docs_%'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(before, 6, "fixture should pre-seed the 6 legacy triggers");
        }
        // Rust migration over the legacy fixture must drop every trigger.
        let conn = open_db(&path).unwrap();
        run_migrations(&conn).unwrap();
        let after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'brain_docs_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, 0, "all legacy triggers must be dropped by run_migrations");
    }

    fn tmp_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        (dir, path)
    }
}
