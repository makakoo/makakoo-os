//! Durable agent sessions: persistent child-agent work, bounded handles, and gates.
//!
//! This is the Rust/Makakoo OS implementation of the Agent Sessions v1
//! primitive. It is sync-first for CLI use and stores full outputs behind
//! handles so parent contexts stay small.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Utc};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{MakakooError, Result};
use crate::event_bus::PersistentEventBus;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agent_sessions (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    role                TEXT NOT NULL,
    status              TEXT NOT NULL,
    assignment          TEXT NOT NULL,
    workspace           TEXT NOT NULL,
    parent_task_id      TEXT,
    parent_session_id   TEXT,
    owner_token         TEXT,
    created_at          TEXT NOT NULL,
    started_at          TEXT,
    completed_at        TEXT,
    closed_at           TEXT,
    updated_at          TEXT NOT NULL,
    agent_type          TEXT NOT NULL DEFAULT 'sync_cli',
    model               TEXT,
    tool_policy_json    TEXT NOT NULL DEFAULT '{}',
    summary             TEXT NOT NULL DEFAULT '',
    result_handle       TEXT,
    transcript_handle   TEXT,
    error               TEXT NOT NULL DEFAULT '',
    metadata_json       TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_name_status ON agent_sessions(name, status);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_status_updated ON agent_sessions(status, updated_at);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_parent_task ON agent_sessions(parent_task_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_sessions_active_name
    ON agent_sessions(name)
    WHERE status IN ('queued', 'running');

CREATE TABLE IF NOT EXISTS agent_session_events (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   TEXT NOT NULL,
    event_type   TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_session_events_session_seq ON agent_session_events(session_id, seq);

CREATE TABLE IF NOT EXISTS agent_session_items (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    kind        TEXT NOT NULL,
    status      TEXT NOT NULL,
    summary     TEXT NOT NULL DEFAULT '',
    artifact_id TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_session_items_session ON agent_session_items(session_id, created_at);

CREATE TABLE IF NOT EXISTS agent_session_artifacts (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    session_id  TEXT,
    producer    TEXT NOT NULL,
    payload     TEXT NOT NULL,
    mime        TEXT NOT NULL DEFAULT 'text/plain',
    created_at  TEXT NOT NULL,
    pinned      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_agent_session_artifacts_session ON agent_session_artifacts(session_id, created_at);

CREATE TABLE IF NOT EXISTS agent_session_gates (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    name            TEXT NOT NULL,
    command         TEXT NOT NULL,
    cwd             TEXT NOT NULL,
    exit_code       INTEGER NOT NULL,
    duration_ms     INTEGER NOT NULL,
    classification  TEXT NOT NULL,
    summary         TEXT NOT NULL DEFAULT '',
    log_artifact_id TEXT,
    created_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_session_gates_session ON agent_session_gates(session_id, created_at);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionRole {
    General,
    Explore,
    Plan,
    Review,
    Implementer,
    Verifier,
    Custom,
}

impl AgentSessionRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Review => "review",
            Self::Implementer => "implementer",
            Self::Verifier => "verifier",
            Self::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "general" => Ok(Self::General),
            "explore" => Ok(Self::Explore),
            "plan" => Ok(Self::Plan),
            "review" => Ok(Self::Review),
            "implementer" => Ok(Self::Implementer),
            "verifier" => Ok(Self::Verifier),
            "custom" => Ok(Self::Custom),
            other => Err(MakakooError::invalid_input(format!(
                "invalid role '{other}'. Accepted roles: {}",
                Self::accepted().join(", ")
            ))),
        }
    }

    pub fn accepted() -> Vec<&'static str> {
        vec![
            "general",
            "explore",
            "plan",
            "review",
            "implementer",
            "verifier",
            "custom",
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
    Closed,
}

impl AgentSessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::Closed => "closed",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "cancelled" => Ok(Self::Cancelled),
            "closed" => Ok(Self::Closed),
            other => Err(MakakooError::invalid_input(format!(
                "invalid status '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub name: String,
    pub role: AgentSessionRole,
    pub status: AgentSessionStatus,
    pub assignment: String,
    pub workspace: PathBuf,
    pub parent_task_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub owner_token: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub agent_type: String,
    pub model: Option<String>,
    pub tool_policy: Value,
    pub summary: String,
    pub result_handle: Option<String>,
    pub transcript_handle: Option<String>,
    pub error: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRecord {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub command: String,
    pub cwd: PathBuf,
    pub exit_code: i32,
    pub duration_ms: i64,
    pub classification: String,
    pub summary: String,
    pub log_artifact_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandleRead {
    pub ok: bool,
    pub handle: String,
    pub mode: String,
    pub content: String,
    pub truncated: bool,
    pub bytes_returned: usize,
    pub total_bytes: usize,
    pub error: Option<String>,
    pub error_type: Option<String>,
    pub available_sections: Vec<String>,
    pub value: Option<Value>,
}

#[derive(Clone)]
pub struct AgentSessionStore {
    conn: Arc<Mutex<Connection>>,
    bus: Option<Arc<PersistentEventBus>>,
}

pub fn db_path(home: &Path) -> PathBuf {
    home.join("data").join("agent_sessions.db")
}

impl AgentSessionStore {
    pub fn open(path: &Path, bus: Option<Arc<PersistentEventBus>>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            bus,
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| MakakooError::internal("agent session mutex poisoned"))
    }

    pub fn open_session(
        &self,
        name: &str,
        role: AgentSessionRole,
        assignment: &str,
        workspace: &Path,
        model: Option<&str>,
        metadata: Value,
    ) -> Result<AgentSession> {
        let name = validate_session_name(name)?;
        let assignment = assignment.trim();
        if assignment.is_empty() {
            return Err(MakakooError::invalid_input("task is required"));
        }
        let workspace = normalize_existing_dir(workspace)?;
        let conn = self.lock()?;
        if let Some(status) = conn
            .query_row(
                "SELECT status FROM agent_sessions WHERE name=?1 ORDER BY created_at DESC LIMIT 1",
                params![name],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            if status == "queued" || status == "running" {
                return Err(MakakooError::invalid_input(format!(
                    "active session name already exists: {name}"
                )));
            }
        }
        let id = id("as");
        let now = Utc::now();
        conn.execute(
            "INSERT INTO agent_sessions (id,name,role,status,assignment,workspace,created_at,updated_at,agent_type,model,tool_policy_json,metadata_json) VALUES (?1,?2,?3,'queued',?4,?5,?6,?6,'sync_cli',?7,'{}',?8)",
            params![id, name, role.as_str(), assignment, workspace.to_string_lossy(), now.to_rfc3339(), model, serde_json::to_string(&metadata)?]
        )?;
        drop(conn);
        self.append_event(
            &id,
            "agent.opened",
            json!({"name": name, "role": role.as_str()}),
        )?;
        self.append_item(
            &id,
            "assignment",
            "ok",
            assignment,
            None,
            json!({"workspace": workspace}),
        )?;
        self.get(&id)
    }

    pub fn list(
        &self,
        status: Option<AgentSessionStatus>,
        include_closed: bool,
    ) -> Result<Vec<AgentSession>> {
        let conn = self.lock()?;
        let mut sql = "SELECT * FROM agent_sessions".to_string();
        let mut clauses = Vec::new();
        if status.is_some() {
            clauses.push("status = ?1".to_string());
        }
        if !include_closed {
            clauses.push("status != 'closed'".to_string());
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY updated_at DESC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = if let Some(s) = status {
            stmt.query_map(params![s.as_str()], row_to_session)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], row_to_session)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub fn get(&self, name_or_id: &str) -> Result<AgentSession> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT * FROM agent_sessions WHERE id=?1 OR name=?1 ORDER BY created_at DESC LIMIT 1",
            params![name_or_id],
            row_to_session,
        )
        .optional()?
        .ok_or_else(|| MakakooError::NotFound(format!("session not found: {name_or_id}")))
    }

    pub fn mark_running(&self, session_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.lock()?;
        let n = conn.execute("UPDATE agent_sessions SET status='running', started_at=COALESCE(started_at, ?1), updated_at=?1 WHERE id=?2", params![now, session_id])?;
        if n == 0 {
            return Err(MakakooError::NotFound(format!(
                "session not found: {session_id}"
            )));
        }
        drop(conn);
        self.append_event(session_id, "agent.started", json!({}))?;
        Ok(())
    }

    pub fn complete(
        &self,
        session_id: &str,
        summary: &str,
        result_handle: Option<&str>,
    ) -> Result<()> {
        self.terminal(
            session_id,
            AgentSessionStatus::Completed,
            summary,
            "",
            result_handle,
        )?;
        self.append_item(
            session_id,
            "result",
            "ok",
            summary,
            result_handle,
            json!({}),
        )?;
        self.append_event(
            session_id,
            "agent.completed",
            json!({"result_handle": result_handle}),
        )?;
        Ok(())
    }

    pub fn fail(&self, session_id: &str, error: &str, result_handle: Option<&str>) -> Result<()> {
        self.terminal(
            session_id,
            AgentSessionStatus::Failed,
            "",
            error,
            result_handle,
        )?;
        self.append_item(
            session_id,
            "error",
            "failed",
            error,
            result_handle,
            json!({}),
        )?;
        self.append_event(
            session_id,
            "agent.failed",
            json!({"error": error, "result_handle": result_handle}),
        )?;
        Ok(())
    }

    pub fn close(&self, name_or_id: &str, cancel: bool) -> Result<AgentSession> {
        let s = self.get(name_or_id)?;
        let now = Utc::now().to_rfc3339();
        let status = if cancel
            && matches!(
                s.status,
                AgentSessionStatus::Queued | AgentSessionStatus::Running
            ) {
            "cancelled"
        } else {
            "closed"
        };
        let conn = self.lock()?;
        conn.execute(
            "UPDATE agent_sessions SET status=?1, closed_at=?2, updated_at=?2 WHERE id=?3",
            params![status, now, s.id],
        )?;
        drop(conn);
        self.append_event(&s.id, "agent.closed", json!({"cancel": cancel}))?;
        self.get(&s.id)
    }

    pub fn publish_artifact(
        &self,
        session_id: Option<&str>,
        name: &str,
        producer: &str,
        payload: &str,
        mime: &str,
        pinned: bool,
    ) -> Result<String> {
        let aid = id("asa");
        let now = Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO agent_session_artifacts (id,name,session_id,producer,payload,mime,created_at,pinned) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![aid, name, session_id, producer, payload, mime, now, if pinned {1} else {0}],
        )?;
        Ok(format!("agent-artifact://{aid}"))
    }

    pub fn read_handle(
        &self,
        handle: &str,
        mode: ReadMode,
        max_bytes: usize,
    ) -> Result<HandleRead> {
        let aid = handle
            .strip_prefix("agent-artifact://")
            .ok_or_else(|| MakakooError::invalid_input(format!("unsupported handle: {handle}")))?;
        let conn = self.lock()?;
        let payload: String = conn
            .query_row(
                "SELECT payload FROM agent_session_artifacts WHERE id=?1",
                params![aid],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| MakakooError::NotFound(format!("artifact not found: {handle}")))?;
        drop(conn);
        Ok(project_payload(handle, &payload, mode, max_bytes))
    }

    pub fn run_gate(
        &self,
        name_or_id: &str,
        name: &str,
        cwd: &Path,
        cmd: &str,
    ) -> Result<GateRecord> {
        let session = self.get(name_or_id)?;
        let cwd = normalize_existing_dir(cwd)?;
        let start = Instant::now();
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&cwd)
            .output()?;
        let duration_ms = start.elapsed().as_millis() as i64;
        let exit_code = output.status.code().unwrap_or(128);
        let log = format!(
            "$ {cmd}\n# cwd: {}\n# exit_code: {exit_code}\n\n--- stdout ---\n{}\n--- stderr ---\n{}",
            cwd.display(), String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)
        );
        let log_handle = self.publish_artifact(
            Some(&session.id),
            &format!("gate/{name}"),
            "agent_session_gate",
            &log,
            "text/plain",
            true,
        )?;
        let classification = if exit_code == 0 { "pass" } else { "fail" };
        let summary = summarize_command_output(&output.stdout, &output.stderr);
        let gate = GateRecord {
            id: id("gate"),
            session_id: session.id.clone(),
            name: name.to_string(),
            command: cmd.to_string(),
            cwd,
            exit_code,
            duration_ms,
            classification: classification.to_string(),
            summary,
            log_artifact_id: Some(log_handle.clone()),
            created_at: Utc::now(),
        };
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO agent_session_gates (id,session_id,name,command,cwd,exit_code,duration_ms,classification,summary,log_artifact_id,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![gate.id, gate.session_id, gate.name, gate.command, gate.cwd.to_string_lossy(), gate.exit_code, gate.duration_ms, gate.classification, gate.summary, gate.log_artifact_id, gate.created_at.to_rfc3339()],
        )?;
        drop(conn);
        self.append_item(
            &session.id,
            "gate",
            classification,
            &gate.summary,
            gate.log_artifact_id.as_deref(),
            json!({"gate_id": gate.id, "name": gate.name}),
        )?;
        self.append_event(&session.id, "agent.gate", json!({"gate_id": gate.id, "name": gate.name, "classification": gate.classification, "exit_code": gate.exit_code}))?;
        Ok(gate)
    }

    pub fn gates(&self, name_or_id: &str) -> Result<Vec<GateRecord>> {
        let session = self.get(name_or_id)?;
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT id,session_id,name,command,cwd,exit_code,duration_ms,classification,summary,log_artifact_id,created_at FROM agent_session_gates WHERE session_id=?1 ORDER BY created_at DESC")?;
        let rows = stmt
            .query_map(params![session.id], row_to_gate)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn terminal(
        &self,
        session_id: &str,
        status: AgentSessionStatus,
        summary: &str,
        error: &str,
        result_handle: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.lock()?;
        let n = conn.execute("UPDATE agent_sessions SET status=?1, completed_at=?2, updated_at=?2, summary=?3, error=?4, result_handle=COALESCE(?5, result_handle) WHERE id=?6", params![status.as_str(), now, summary, error, result_handle, session_id])?;
        if n == 0 {
            return Err(MakakooError::NotFound(format!(
                "session not found: {session_id}"
            )));
        }
        Ok(())
    }

    fn append_event(&self, session_id: &str, event_type: &str, payload: Value) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute("INSERT INTO agent_session_events (session_id,event_type,payload_json,created_at) VALUES (?1,?2,?3,?4)", params![session_id, event_type, serde_json::to_string(&payload)?, now])?;
        let seq = conn.last_insert_rowid();
        drop(conn);
        if let Some(bus) = &self.bus {
            let _ = bus.publish(
                &format!("agent_session.{}", event_type.trim_start_matches("agent.")),
                "agent_session",
                json!({"session_id": session_id, "payload": payload}),
            );
        }
        Ok(seq)
    }

    fn append_item(
        &self,
        session_id: &str,
        kind: &str,
        status: &str,
        summary: &str,
        artifact_id: Option<&str>,
        metadata: Value,
    ) -> Result<String> {
        let item_id = id("asi");
        let now = Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute("INSERT INTO agent_session_items (id,session_id,kind,status,summary,artifact_id,metadata_json,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![item_id, session_id, kind, status, summary, artifact_id, serde_json::to_string(&metadata)?, now])?;
        Ok(item_id)
    }
}

#[derive(Debug, Clone)]
pub enum ReadMode {
    Summary,
    Head(usize),
    Tail(usize),
    Section(String),
    JsonPath(String),
}

fn project_payload(handle: &str, payload: &str, mode: ReadMode, max_bytes: usize) -> HandleRead {
    let mode_name = match &mode {
        ReadMode::Summary => "summary",
        ReadMode::Head(_) => "head",
        ReadMode::Tail(_) => "tail",
        ReadMode::Section(_) => "section",
        ReadMode::JsonPath(_) => "jsonpath",
    }
    .to_string();
    let mut available_sections = Vec::new();
    let mut value = None;
    let mut ok = true;
    let mut error = None;
    let mut error_type = None;
    let mut content = match mode {
        ReadMode::Summary => payload
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .take(20)
            .collect::<Vec<_>>()
            .join(" "),
        ReadMode::Head(n) => payload.lines().take(n).collect::<Vec<_>>().join("\n"),
        ReadMode::Tail(n) => {
            let lines = payload.lines().collect::<Vec<_>>();
            lines[lines.len().saturating_sub(n)..].join("\n")
        }
        ReadMode::Section(section) => {
            let sections = parse_sections(payload);
            available_sections = sections.iter().map(|(k, _)| k.clone()).collect();
            sections
                .into_iter()
                .find(|(k, _)| k == &section.to_uppercase())
                .map(|(_, v)| v)
                .unwrap_or_else(|| {
                    ok = false;
                    error_type = Some("section_not_found".to_string());
                    error = Some(format!("section not found: {}", section.to_uppercase()));
                    error.clone().unwrap()
                })
        }
        ReadMode::JsonPath(path) => match serde_json::from_str::<Value>(payload)
            .ok()
            .and_then(|v| json_path(&v, &path).cloned())
        {
            Some(v) => {
                value = Some(v.clone());
                scalar_string(&v)
            }
            None => {
                ok = false;
                error_type = Some("jsonpath_not_found".to_string());
                error = Some(format!("jsonpath not found: {path}"));
                error.clone().unwrap()
            }
        },
    };
    let total_bytes = content.len();
    let truncated = total_bytes > max_bytes;
    if truncated {
        content.truncate(max_bytes);
    }
    HandleRead {
        ok,
        handle: handle.to_string(),
        mode: mode_name,
        bytes_returned: content.len(),
        total_bytes,
        truncated,
        content,
        error,
        error_type,
        available_sections,
        value,
    }
}

fn summarize_command_output(stdout: &[u8], stderr: &[u8]) -> String {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    err.lines()
        .chain(out.lines())
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no output")
        .chars()
        .take(500)
        .collect()
}

fn parse_sections(payload: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in payload.lines() {
        let t = line.trim();
        if t.ends_with(':')
            && t.chars().all(|c| {
                c.is_ascii_uppercase()
                    || c.is_ascii_digit()
                    || c == '_'
                    || c == '-'
                    || c == ' '
                    || c == ':'
            })
        {
            out.push((t.trim_end_matches(':').to_string(), Vec::new()));
        } else if let Some((_, lines)) = out.last_mut() {
            lines.push(line.to_string());
        }
    }
    out.into_iter()
        .map(|(k, v)| (k, v.join("\n").trim().to_string()))
        .collect()
}

fn json_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    if path == "$" {
        return Some(v);
    }
    let mut cur = v;
    let mut chars = path.strip_prefix("$.")?.chars().peekable();
    let mut key = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if !key.is_empty() {
                    cur = cur.get(&key)?;
                    key.clear();
                }
            }
            '[' => {
                if !key.is_empty() {
                    cur = cur.get(&key)?;
                    key.clear();
                }
                let mut idx = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    idx.push(c);
                }
                cur = cur.get(idx.parse::<usize>().ok()?)?;
            }
            _ => key.push(ch),
        }
    }
    if !key.is_empty() {
        cur = cur.get(&key)?;
    }
    Some(cur)
}

fn scalar_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn normalize_existing_dir(path: &Path) -> Result<PathBuf> {
    let p = path.canonicalize()?;
    if !p.is_dir() {
        return Err(MakakooError::invalid_input(format!(
            "not a directory: {}",
            p.display()
        )));
    }
    Ok(p)
}

fn validate_session_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(MakakooError::invalid_input("session name is required"));
    }
    if name.len() > 128 {
        return Err(MakakooError::invalid_input(
            "session name must be 128 bytes or shorter",
        ));
    }
    if name == "." || name == ".." {
        return Err(MakakooError::invalid_input(
            "session name cannot be '.' or '..'",
        ));
    }
    if name
        .chars()
        .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(MakakooError::invalid_input(
            "session name cannot contain slashes or control characters",
        ));
    }
    Ok(name)
}

fn id(prefix: &str) -> String {
    let mut rng = rand::thread_rng();
    format!(
        "{prefix}_{}_{:08x}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        rng.next_u32()
    )
}

fn parse_ts(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentSession> {
    Ok(AgentSession {
        id: row.get("id")?,
        name: row.get("name")?,
        role: AgentSessionRole::parse(&row.get::<_, String>("role")?)
            .unwrap_or(AgentSessionRole::General),
        status: AgentSessionStatus::parse(&row.get::<_, String>("status")?)
            .unwrap_or(AgentSessionStatus::Failed),
        assignment: row.get("assignment")?,
        workspace: PathBuf::from(row.get::<_, String>("workspace")?),
        parent_task_id: row.get("parent_task_id")?,
        parent_session_id: row.get("parent_session_id")?,
        owner_token: row.get("owner_token")?,
        created_at: parse_ts(row.get("created_at")?),
        started_at: row.get::<_, Option<String>>("started_at")?.map(parse_ts),
        completed_at: row.get::<_, Option<String>>("completed_at")?.map(parse_ts),
        closed_at: row.get::<_, Option<String>>("closed_at")?.map(parse_ts),
        updated_at: parse_ts(row.get("updated_at")?),
        agent_type: row.get("agent_type")?,
        model: row.get("model")?,
        tool_policy: serde_json::from_str(&row.get::<_, String>("tool_policy_json")?)
            .unwrap_or(Value::Null),
        summary: row.get("summary")?,
        result_handle: row.get("result_handle")?,
        transcript_handle: row.get("transcript_handle")?,
        error: row.get("error")?,
        metadata: serde_json::from_str(&row.get::<_, String>("metadata_json")?)
            .unwrap_or(Value::Null),
    })
}

fn row_to_gate(row: &rusqlite::Row<'_>) -> rusqlite::Result<GateRecord> {
    Ok(GateRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        name: row.get(2)?,
        command: row.get(3)?,
        cwd: PathBuf::from(row.get::<_, String>(4)?),
        exit_code: row.get(5)?,
        duration_ms: row.get(6)?,
        classification: row.get(7)?,
        summary: row.get(8)?,
        log_artifact_id: row.get(9)?,
        created_at: parse_ts(row.get(10)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_complete_read_result() {
        let dir = tempdir().unwrap();
        let store = AgentSessionStore::open(&dir.path().join("agent_sessions.db"), None).unwrap();
        let s = store
            .open_session(
                "worker",
                AgentSessionRole::General,
                "do it",
                dir.path(),
                None,
                json!({}),
            )
            .unwrap();
        store.mark_running(&s.id).unwrap();
        let handle = store
            .publish_artifact(
                Some(&s.id),
                "result",
                "test",
                "SUMMARY:\nok\nEVIDENCE:\nproof",
                "text/plain",
                true,
            )
            .unwrap();
        store.complete(&s.id, "ok", Some(&handle)).unwrap();
        let r = store
            .read_handle(&handle, ReadMode::Section("EVIDENCE".into()), 8192)
            .unwrap();
        assert_eq!(r.content, "proof");
        assert_eq!(
            store.get("worker").unwrap().status,
            AgentSessionStatus::Completed
        );
    }

    #[test]
    fn duplicate_active_rejected() {
        let dir = tempdir().unwrap();
        let store = AgentSessionStore::open(&dir.path().join("agent_sessions.db"), None).unwrap();
        store
            .open_session(
                "worker",
                AgentSessionRole::General,
                "one",
                dir.path(),
                None,
                json!({}),
            )
            .unwrap();
        let err = store
            .open_session(
                "worker",
                AgentSessionRole::General,
                "two",
                dir.path(),
                None,
                json!({}),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("active session"));
    }

    #[test]
    fn rejects_path_like_session_names() {
        let dir = tempdir().unwrap();
        let store = AgentSessionStore::open(&dir.path().join("agent_sessions.db"), None).unwrap();
        let err = store
            .open_session(
                "../worker",
                AgentSessionRole::General,
                "one",
                dir.path(),
                None,
                json!({}),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("slashes"));
    }

    #[test]
    fn gate_records_pass_and_log_handle() {
        let dir = tempdir().unwrap();
        let store = AgentSessionStore::open(&dir.path().join("agent_sessions.db"), None).unwrap();
        store
            .open_session(
                "gate",
                AgentSessionRole::Verifier,
                "gate",
                dir.path(),
                None,
                json!({}),
            )
            .unwrap();
        let g = store
            .run_gate("gate", "true", dir.path(), "printf hello")
            .unwrap();
        assert_eq!(g.classification, "pass");
        let log = store
            .read_handle(g.log_artifact_id.as_ref().unwrap(), ReadMode::Summary, 8192)
            .unwrap();
        assert!(log.content.contains("hello"));
    }
}
