//! Tier-B journal write handlers.
//!
//! Three related tools, all variations on "append a bullet to today's
//! Brain journal":
//!
//! * `brain_write_journal` — the low-level primitive. Accepts a raw
//!   content string, normalises it to a `-` bullet, appends to today's
//!   `data/Brain/journals/YYYY_MM_DD.md`, and (if the SuperbrainStore
//!   is wired) upserts the journal doc so FTS5 stays fresh.
//! * `harvey_brain_write` — the namespaced MCP name that Claude Code,
//!   Gemini, Codex, Vibe, OpenCode, Cursor, and Qwen actually see.
//!   Same behaviour as `brain_write_journal`; Python keeps both
//!   spellings for historical reasons.
//! * `harvey_journal_entry` — a thin "remember that…" wrapper with
//!   identical semantics; kept for CLI-prompt ergonomics.
//!
//! The append is done via `wiki::save` on a freshly-read-then-extended
//! buffer so the fs2 file lock covers the whole read+write window. No
//! `O_APPEND` race.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Local;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

use makakoo_core::wiki;

/// Resolve today's journal path under `{home}/data/Brain/journals/`.
fn today_journal_path(home: &std::path::Path) -> PathBuf {
    let today = Local::now().format("%Y_%m_%d").to_string();
    home.join("data")
        .join("Brain")
        .join("journals")
        .join(format!("{today}.md"))
}

/// Normalise an incoming entry to a bullet line. Prepend "- " if the
/// caller forgot it; leave existing bullets alone. Always ends with a
/// single `\n`.
fn normalize_bullet(entry: &str) -> String {
    let trimmed = entry.trim_end_matches('\n');
    let bullet = if trimmed.starts_with("- ") || trimmed.starts_with("-\n") || trimmed == "-" {
        trimmed.to_string()
    } else {
        format!("- {trimmed}")
    };
    format!("{bullet}\n")
}

/// Core write logic shared by all three handlers. Returns the journal
/// doc id (the path under home) so callers can surface it back to the
/// model.
async fn write_today_journal(
    ctx: &Arc<ToolContext>,
    content: &str,
    doc_type: &str,
) -> Result<String, RpcError> {
    if content.trim().is_empty() {
        return Err(RpcError::invalid_params("content is empty"));
    }

    let path = today_journal_path(&ctx.home);
    let bullet = normalize_bullet(content);

    // Read-modify-write under the fs2 lock inside `wiki::save`. The
    // journal file might not exist yet, in which case we start fresh.
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(RpcError::internal(format!(
                "brain_write_journal: read {} failed: {e}",
                path.display()
            )))
        }
    };

    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&bullet);

    wiki::save(&path, &next)
        .map_err(|e| RpcError::internal(format!("brain_write_journal: save failed: {e}")))?;

    // If the SuperbrainStore is wired, keep FTS5 fresh too. The doc_id
    // matches the Python convention: the full absolute path.
    let doc_id = path.to_string_lossy().to_string();
    if let Some(store) = ctx.store.as_ref() {
        store
            .write_document(&doc_id, &next, doc_type, Value::Null)
            .map_err(|e| RpcError::internal(format!(
                "brain_write_journal: store upsert failed: {e}"
            )))?;
    }
    Ok(doc_id)
}

// ─────────────────────────────────────────────────────────────────────
// brain_write_journal
// ─────────────────────────────────────────────────────────────────────

pub struct BrainWriteJournalHandler {
    ctx: Arc<ToolContext>,
}

impl BrainWriteJournalHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for BrainWriteJournalHandler {
    fn name(&self) -> &str {
        "brain_write_journal"
    }
    fn description(&self) -> &str {
        "Append a bullet to today's Brain journal \
         (data/Brain/journals/YYYY_MM_DD.md). Low-level primitive used \
         by harvey_brain_write and harvey_journal_entry."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string" },
                "doc_type": { "type": "string", "default": "journal" }
            },
            "required": ["content"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'content'"))?;
        let doc_type = params
            .get("doc_type")
            .and_then(|v| v.as_str())
            .unwrap_or("journal");
        let doc_id = write_today_journal(&self.ctx, content, doc_type).await?;
        Ok(json!({ "doc_id": doc_id }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// harvey_brain_write
// ─────────────────────────────────────────────────────────────────────

pub struct HarveyBrainWriteHandler {
    ctx: Arc<ToolContext>,
}

impl HarveyBrainWriteHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for HarveyBrainWriteHandler {
    fn name(&self) -> &str {
        "harvey_brain_write"
    }
    fn description(&self) -> &str {
        "Append a bullet to today's Brain journal — namespaced MCP tool \
         surfaced to every CLI."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string" },
                "doc_type": { "type": "string", "default": "journal" }
            },
            "required": ["content"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'content'"))?;
        let doc_type = params
            .get("doc_type")
            .and_then(|v| v.as_str())
            .unwrap_or("journal");
        let doc_id = write_today_journal(&self.ctx, content, doc_type).await?;
        Ok(json!({ "doc_id": doc_id }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// harvey_journal_entry
// ─────────────────────────────────────────────────────────────────────

pub struct HarveyJournalEntryHandler {
    ctx: Arc<ToolContext>,
}

impl HarveyJournalEntryHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for HarveyJournalEntryHandler {
    fn name(&self) -> &str {
        "harvey_journal_entry"
    }
    fn description(&self) -> &str {
        "Remember-that-style wrapper over brain_write_journal. Use for \
         'log that I did X' prompts."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string" }
            },
            "required": ["content"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'content'"))?;
        let doc_id = write_today_journal(&self.ctx, content, "journal").await?;
        Ok(json!({ "doc_id": doc_id }))
    }
}

// ═════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(home: &std::path::Path) -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(home.to_path_buf()))
    }

    #[test]
    fn normalize_bullet_adds_dash_if_missing() {
        assert_eq!(normalize_bullet("hello"), "- hello\n");
        assert_eq!(normalize_bullet("- already"), "- already\n");
        assert_eq!(normalize_bullet("  - nested"), "-   - nested\n");
    }

    #[tokio::test]
    async fn brain_write_journal_creates_file_on_first_call() {
        let tmp = tempfile::tempdir().unwrap();
        let h = BrainWriteJournalHandler::new(ctx(tmp.path()));
        let out = h
            .call(json!({ "content": "first entry" }))
            .await
            .unwrap();
        let doc_id = out["doc_id"].as_str().unwrap().to_string();
        let body = std::fs::read_to_string(&doc_id).unwrap();
        assert!(body.contains("- first entry"));
    }

    #[tokio::test]
    async fn brain_write_journal_appends_without_clobbering() {
        let tmp = tempfile::tempdir().unwrap();
        let h = BrainWriteJournalHandler::new(ctx(tmp.path()));
        h.call(json!({ "content": "one" })).await.unwrap();
        h.call(json!({ "content": "two" })).await.unwrap();
        let path = today_journal_path(tmp.path());
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("- one"));
        assert!(body.contains("- two"));
        // "one" must come before "two".
        assert!(body.find("- one").unwrap() < body.find("- two").unwrap());
    }

    #[tokio::test]
    async fn harvey_brain_write_and_journal_entry_are_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        let c = ctx(tmp.path());
        HarveyBrainWriteHandler::new(c.clone())
            .call(json!({ "content": "alpha" }))
            .await
            .unwrap();
        HarveyJournalEntryHandler::new(c.clone())
            .call(json!({ "content": "beta" }))
            .await
            .unwrap();
        let path = today_journal_path(tmp.path());
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("- alpha"));
        assert!(body.contains("- beta"));
    }

    #[tokio::test]
    async fn empty_content_is_rejected_as_invalid_params() {
        let tmp = tempfile::tempdir().unwrap();
        let h = BrainWriteJournalHandler::new(ctx(tmp.path()));
        let err = h.call(json!({ "content": "   " })).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }
}
