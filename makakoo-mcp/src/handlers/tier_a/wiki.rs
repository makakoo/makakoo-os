//! Tier-A wiki_lint — read-only lint pass over a wiki page file.

use async_trait::async_trait;
use makakoo_core::wiki::lint::WikiLinter;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

pub struct WikiLintHandler {
    #[allow(dead_code)]
    ctx: Arc<ToolContext>,
}

impl WikiLintHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for WikiLintHandler {
    fn name(&self) -> &str {
        "wiki_lint"
    }
    fn description(&self) -> &str {
        "Run WikiLinter over a wiki page. Accepts a `page_path` (absolute \
         or relative to $MAKAKOO_HOME) or an inline `content` string."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "page_path": { "type": "string", "description": "Path to .md file" },
                "content": { "type": "string", "description": "Inline content alternative" }
            }
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let linter = WikiLinter::new();

        // Prefer inline content when supplied — useful for the CLI.
        if let Some(content) = params.get("content").and_then(Value::as_str) {
            let report = linter.lint_str(content, None);
            return Ok(json!(report));
        }

        let raw_path = params
            .get("page_path")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("missing 'page_path' or 'content'"))?;
        let path = PathBuf::from(raw_path);
        let full = if path.is_absolute() {
            path
        } else {
            self.ctx.home.join(path)
        };
        if !full.is_file() {
            return Err(RpcError::invalid_params(format!(
                "page_path not a file: {}",
                full.display()
            )));
        }
        let report = linter
            .lint_file(&full)
            .map_err(|e| RpcError::internal(format!("wiki_lint: {e}")))?;
        Ok(json!(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn empty_ctx() -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(PathBuf::from("/tmp/makakoo-t13-test")))
    }

    #[tokio::test]
    async fn requires_page_path_or_content() {
        let h = WikiLintHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn inline_content_lints_clean() {
        let h = WikiLintHandler::new(empty_ctx());
        let out = h
            .call(json!({"content": "- hello [[World]]\n"}))
            .await
            .unwrap();
        // Clean reports have an empty issues array.
        assert!(out["issues"].is_array());
    }

    #[tokio::test]
    async fn lints_real_file_from_disk() {
        let tmp = tempdir().unwrap();
        let file = tmp.path().join("page.md");
        std::fs::write(&file, "- hello [[World]]\n").unwrap();
        let h = WikiLintHandler::new(empty_ctx());
        let out = h
            .call(json!({"page_path": file.to_string_lossy()}))
            .await
            .unwrap();
        assert!(out["issues"].is_array());
    }
}
