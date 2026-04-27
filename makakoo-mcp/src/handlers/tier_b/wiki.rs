//! Tier-B wiki handlers: compile + save.
//!
//! These handlers map directly onto the `makakoo_core::wiki` subsystem:
//!
//! * `wiki_compile` — reads `source_path`, runs the `WikiCompiler` with
//!   default options, and returns the compiled page body plus transform
//!   stats. It does NOT write the compiled result — callers combine this
//!   with `wiki_save` when they want to persist.
//! * `wiki_save` — atomic, fs2-locked write of `content` to `path` via
//!   `makakoo_core::wiki::save`. The canonical write path for every
//!   Rust-side Brain page mutation.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

use makakoo_core::wiki::{self, CompileOptions, WikiCompiler};

// ─────────────────────────────────────────────────────────────────────
// wiki_compile
// ─────────────────────────────────────────────────────────────────────

pub struct WikiCompileHandler {
    #[allow(dead_code)]
    ctx: Arc<ToolContext>,
}

impl WikiCompileHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for WikiCompileHandler {
    fn name(&self) -> &str {
        "wiki_compile"
    }
    fn description(&self) -> &str {
        "Compile a freeform markdown file into a Logseq-ready bullet tree. \
         Read-only; returns the compiled body + transform stats."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_path": { "type": "string" },
                "title": { "type": "string" },
                "collapse_blanks": { "type": "boolean", "default": false }
            },
            "required": ["source_path"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let source_path = params
            .get("source_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'source_path'"))?;
        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let collapse_blanks = params
            .get("collapse_blanks")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let path = PathBuf::from(source_path);
        let body = std::fs::read_to_string(&path).map_err(|e| {
            RpcError::internal(format!(
                "wiki_compile: read {} failed: {e}",
                path.display()
            ))
        })?;

        let opts = CompileOptions {
            title,
            properties: Vec::new(),
            collapse_blanks,
        };
        let compiled = WikiCompiler::new().compile(&body, &opts);

        Ok(json!({
            "content": compiled.content,
            "lines_rewritten": compiled.lines_rewritten,
            "lines_total": compiled.lines_total,
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// wiki_save
// ─────────────────────────────────────────────────────────────────────

pub struct WikiSaveHandler {
    #[allow(dead_code)]
    ctx: Arc<ToolContext>,
}

impl WikiSaveHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for WikiSaveHandler {
    fn name(&self) -> &str {
        "wiki_save"
    }
    fn description(&self) -> &str {
        "Atomic fs2-locked write of content to a wiki page path. \
         Creates parent directories as needed; overwrites existing files."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let path_s = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'path'"))?;
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'content'"))?;

        let path = PathBuf::from(path_s);
        wiki::save(&path, content)
            .map_err(|e| RpcError::internal(format!("wiki_save: {e}")))?;

        Ok(json!({
            "ok": true,
            "bytes": content.len(),
        }))
    }
}

// ═════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(std::env::temp_dir()))
    }

    #[tokio::test]
    async fn wiki_compile_normalises_plain_prose() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("notes.md");
        std::fs::write(&src, "Harvey runs on switchAILocal\nHermes too\n").unwrap();

        let h = WikiCompileHandler::new(ctx());
        let out = h
            .call(json!({ "source_path": src.to_string_lossy() }))
            .await
            .unwrap();
        let content = out["content"].as_str().unwrap();
        assert!(content.contains("- Harvey runs on switchAILocal"));
        assert!(content.contains("- Hermes too"));
        assert_eq!(out["lines_rewritten"].as_u64().unwrap(), 2);
    }

    #[tokio::test]
    async fn wiki_save_writes_and_returns_byte_count() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("sub").join("page.md");

        let h = WikiSaveHandler::new(ctx());
        let body = "- [[Harvey]]\n  - autonomous\n";
        let out = h
            .call(json!({
                "path": target.to_string_lossy(),
                "content": body,
            }))
            .await
            .unwrap();
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["bytes"].as_u64().unwrap(), body.len() as u64);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), body);
    }

    #[tokio::test]
    async fn wiki_compile_errors_on_missing_source() {
        let h = WikiCompileHandler::new(ctx());
        let err = h
            .call(json!({ "source_path": "/nonexistent/path.md" }))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }
}
