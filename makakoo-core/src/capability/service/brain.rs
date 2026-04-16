//! `BrainHandler` — read + search + append-journal over the socket.
//!
//! Spec: `spec/CAPABILITIES.md §1.1`. Plugins declare `brain/read` to
//! pull Brain content (journals, pages, FTS hits) and `brain/write` to
//! append to today's journal or to persist a new Brain document.
//!
//! The Brain has two storage layers:
//!
//! 1. **Superbrain** — SQLite FTS5 + vector index at
//!    `$MAKAKOO_HOME/data/superbrain.db`. Primary read path; gives
//!    plugins fast keyword + semantic search.
//! 2. **Plain markdown** — `$MAKAKOO_HOME/data/Brain/journals/YYYY_MM_DD.md`
//!    + `$MAKAKOO_HOME/data/Brain/pages/*.md`. Source of truth; the
//!    Superbrain is an index over these.
//!
//! This handler serves reads out of Superbrain (fast) and writes into
//! plain markdown (canonical). A later slice can add a background sync
//! that re-indexes the new journal entry; for now the write is
//! fire-and-forget to the filesystem.
//!
//! **Methods served:**
//! - `brain.search` params `{ query, limit? }` → `{ hits: [SearchHit] }`
//! - `brain.recent` params `{ limit?, doc_type? }` → `{ hits: [SearchHit] }`
//! - `brain.read` params `{ doc_id }` → `{ doc: Document | null }`
//! - `brain.write_journal` params `{ line }` → `{ appended_to: path }`

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Local;
use serde::Deserialize;
use serde_json::json;

use crate::capability::socket::{
    CapabilityError, CapabilityHandler, CapabilityRequest,
};
use crate::superbrain::store::SuperbrainStore;

const DEFAULT_LIMIT: usize = 10;

pub struct BrainHandler {
    store: Arc<SuperbrainStore>,
    brain_root: PathBuf,
}

impl BrainHandler {
    pub fn new(store: Arc<SuperbrainStore>, brain_root: PathBuf) -> Self {
        Self { store, brain_root }
    }

    /// Path to today's journal file. Creates parent dir if missing on
    /// first write; the file itself is created on demand.
    pub fn todays_journal(&self) -> PathBuf {
        let today = Local::now().format("%Y_%m_%d").to_string();
        self.brain_root.join("journals").join(format!("{today}.md"))
    }
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct RecentParams {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    doc_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReadParams {
    doc_id: String,
}

#[derive(Debug, Deserialize)]
struct WriteJournalParams {
    line: String,
}

fn bad_params(e: serde_json::Error) -> CapabilityError {
    CapabilityError::bad_request(format!("bad params: {e}"))
}

#[async_trait]
impl CapabilityHandler for BrainHandler {
    async fn handle(
        &self,
        request: &CapabilityRequest,
        _scope: Option<&str>,
    ) -> Result<serde_json::Value, CapabilityError> {
        match request.method.as_str() {
            "brain.search" => {
                let p: SearchParams =
                    serde_json::from_value(request.params.clone()).map_err(bad_params)?;
                let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
                let hits = self
                    .store
                    .search(&p.query, limit)
                    .map_err(|e| CapabilityError::handler(format!("search: {e}")))?;
                Ok(json!({ "hits": hits }))
            }
            "brain.recent" => {
                let p: RecentParams = if request.params.is_null() {
                    RecentParams::default()
                } else {
                    serde_json::from_value(request.params.clone())
                        .map_err(bad_params)?
                };
                let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
                let hits = self
                    .store
                    .recent(limit, p.doc_type.as_deref())
                    .map_err(|e| CapabilityError::handler(format!("recent: {e}")))?;
                Ok(json!({ "hits": hits }))
            }
            "brain.read" => {
                let p: ReadParams =
                    serde_json::from_value(request.params.clone()).map_err(bad_params)?;
                let doc = self
                    .store
                    .get_document(&p.doc_id)
                    .map_err(|e| CapabilityError::handler(format!("get: {e}")))?;
                Ok(json!({ "doc": doc }))
            }
            "brain.write_journal" => {
                let p: WriteJournalParams =
                    serde_json::from_value(request.params.clone()).map_err(bad_params)?;
                let path = self.todays_journal();
                append_journal_line(&path, &p.line)
                    .await
                    .map_err(|e| CapabilityError::handler(format!("journal: {e}")))?;
                Ok(json!({ "appended_to": path.to_string_lossy() }))
            }
            other => Err(CapabilityError::handler(format!(
                "unknown brain method {other:?}"
            ))),
        }
    }
}

/// Append a line to the journal file, creating it if missing. The
/// line is normalised: always starts with `- ` (Logseq outliner
/// format), trailing whitespace trimmed, exactly one trailing newline.
pub async fn append_journal_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut text = line.trim_end().to_string();
    if !text.starts_with("- ") {
        text = format!("- {text}");
    }
    text.push('\n');

    use tokio::io::AsyncWriteExt;
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    f.write_all(text.as_bytes()).await?;
    f.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store(dir: &Path) -> Arc<SuperbrainStore> {
        let db = dir.join("superbrain.db");
        Arc::new(SuperbrainStore::open(&db).expect("open store"))
    }

    fn req(method: &str, params: serde_json::Value) -> CapabilityRequest {
        CapabilityRequest {
            id: json!(1),
            method: method.into(),
            params,
            verb: "brain/read".into(),
            scope: String::new(),
            correlation_id: None,
        }
    }

    #[tokio::test]
    async fn brain_search_returns_empty_on_fresh_store() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let h = BrainHandler::new(store, tmp.path().join("Brain"));
        let r = h
            .handle(&req("brain.search", json!({ "query": "anything" })), None)
            .await
            .unwrap();
        assert_eq!(r["hits"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn brain_recent_returns_empty_on_fresh_store() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let h = BrainHandler::new(store, tmp.path().join("Brain"));
        let r = h
            .handle(&req("brain.recent", json!({})), None)
            .await
            .unwrap();
        assert_eq!(r["hits"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn brain_recent_accepts_null_params() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let h = BrainHandler::new(store, tmp.path().join("Brain"));
        let r = h
            .handle(
                &req("brain.recent", serde_json::Value::Null),
                None,
            )
            .await
            .unwrap();
        assert!(r["hits"].is_array());
    }

    #[tokio::test]
    async fn brain_read_of_missing_doc_returns_null() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let h = BrainHandler::new(store, tmp.path().join("Brain"));
        let r = h
            .handle(&req("brain.read", json!({ "doc_id": "ghost" })), None)
            .await
            .unwrap();
        assert!(r["doc"].is_null());
    }

    #[tokio::test]
    async fn write_journal_creates_file_and_appends() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let brain_root = tmp.path().join("Brain");
        let h = BrainHandler::new(store, brain_root.clone());

        let r = h
            .handle(
                &req("brain.write_journal", json!({ "line": "Shipped Phase E/3b" })),
                None,
            )
            .await
            .unwrap();
        let path_str = r["appended_to"].as_str().unwrap();
        let path = Path::new(path_str);
        assert!(path.exists());

        let content = tokio::fs::read_to_string(path).await.unwrap();
        assert!(content.contains("- Shipped Phase E/3b"));
    }

    #[tokio::test]
    async fn write_journal_prepends_dash_if_missing() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let brain_root = tmp.path().join("Brain");
        let h = BrainHandler::new(store, brain_root);

        // Line without leading "- "
        h.handle(
            &req("brain.write_journal", json!({ "line": "plain text" })),
            None,
        )
        .await
        .unwrap();
        // Line already normalised
        h.handle(
            &req("brain.write_journal", json!({ "line": "- already bulleted" })),
            None,
        )
        .await
        .unwrap();

        let path = h.todays_journal();
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("- plain text"));
        assert!(lines[1].starts_with("- already bulleted"));
    }

    #[tokio::test]
    async fn unknown_method_errors() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let h = BrainHandler::new(store, tmp.path().join("Brain"));
        let err = h
            .handle(&req("brain.teleport", json!({})), None)
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown brain method"));
    }
}
