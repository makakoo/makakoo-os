//! Tier-A skill_discover — filesystem scan of `$MAKAKOO_HOME/plugins/`
//! for `SKILL.md` files. No DB, no subsystems — just walks the tree and
//! filters by substring on the relative path / description.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

const MAX_SKILL_DEPTH: usize = 6;

pub struct SkillDiscoverHandler {
    ctx: Arc<ToolContext>,
}

impl SkillDiscoverHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }

    fn skills_root(&self) -> PathBuf {
        // Post-retirement (2026-04-20, archived tree): skills live as
        // installed plugin directories under $MAKAKOO_HOME/plugins/.
        self.ctx.home.join("plugins")
    }
}

#[async_trait]
impl ToolHandler for SkillDiscoverHandler {
    fn name(&self) -> &str {
        "skill_discover"
    }
    fn description(&self) -> &str {
        "Find SKILL.md entries under $MAKAKOO_HOME/plugins/. Optional \
         substring filter on path or skill name."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Optional substring filter" },
                "limit": { "type": "integer", "default": 50 }
            }
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;

        let root = self.skills_root();
        let mut out: Vec<Value> = Vec::new();
        if !root.is_dir() {
            return Ok(json!(out));
        }
        walk(&root, &root, 0, &query, &mut out, limit);
        Ok(json!(out))
    }
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    query: &str,
    out: &mut Vec<Value>,
    limit: usize,
) {
    if out.len() >= limit || depth > MAX_SKILL_DEPTH {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, depth + 1, query, out, limit);
        } else if name.eq_ignore_ascii_case("SKILL.md") {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            let rel_str = rel.to_string_lossy().to_string();
            // Skill name = immediate parent dir; category = the dir above that.
            let parent = path.parent();
            let skill_name = parent
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let category = parent
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let haystack = format!("{rel_str} {skill_name} {category}").to_ascii_lowercase();
            if !query.is_empty() && !haystack.contains(query) {
                continue;
            }
            out.push(json!({
                "name": skill_name,
                "category": category,
                "path": path.to_string_lossy(),
                "relative_path": rel_str,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx_with_home(home: PathBuf) -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(home))
    }

    #[tokio::test]
    async fn skill_discover_empty_home_returns_empty_array() {
        let tmp = tempdir().unwrap();
        let h = SkillDiscoverHandler::new(ctx_with_home(tmp.path().to_path_buf()));
        let out = h.call(json!({})).await.unwrap();
        assert!(out.is_array());
        assert_eq!(out.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn skill_discover_finds_seeded_skill() {
        let tmp = tempdir().unwrap();
        let skill_dir = tmp
            .path()
            .join("plugins")
            .join("test-category")
            .join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# my-skill\n").unwrap();

        let h = SkillDiscoverHandler::new(ctx_with_home(tmp.path().to_path_buf()));
        let out = h.call(json!({})).await.unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "my-skill");
        assert_eq!(arr[0]["category"], "test-category");
    }

    #[tokio::test]
    async fn skill_discover_filters_by_query() {
        let tmp = tempdir().unwrap();
        let base = tmp.path().join("plugins").join("meta");
        for name in ["alpha", "beta", "gamma"] {
            let d = base.join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("SKILL.md"), "").unwrap();
        }
        let h = SkillDiscoverHandler::new(ctx_with_home(tmp.path().to_path_buf()));
        let out = h.call(json!({"query": "beta"})).await.unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "beta");
    }
}
