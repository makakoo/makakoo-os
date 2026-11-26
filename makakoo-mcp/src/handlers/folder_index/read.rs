//! Read-side MCP handlers for folder indexes (`folder_index_search` and
//! `folder_index_states`) plus end-to-end roundtrip tests.

use std::sync::Arc;

use async_trait::async_trait;
use makakoo_core::superbrain::folder_index::{
    FolderSearchRequest, SearchFilter, SourceKind, TrustLevel,
};
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

use super::{map_core_err, parse_dim, require_manager, require_str};

// ─────────────────────────────────────────────────────────────────────
// folder_index_search
// ─────────────────────────────────────────────────────────────────────

pub struct FolderIndexSearchHandler {
    ctx: Arc<ToolContext>,
}

impl FolderIndexSearchHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for FolderIndexSearchHandler {
    fn name(&self) -> &str {
        "folder_index_search"
    }

    fn description(&self) -> &str {
        "Search a folder index by embedding vector or text query with optional provenance filters (source_kind, trust, owner), returning ranked passages with exact places and index freshness metadata."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "folder_id": { "type": "string" },
                "query_vector": { "type": "array", "items": { "type": "number" } },
                "query": { "type": "string", "description": "Query text (used for FTS fallback when stale/rebuilding, or embedded if query_vector omitted)" },
                "query_model": { "type": "string", "description": "Optional reported embedding model id for query_vector" },
                "query_dim": { "type": "integer", "minimum": 1, "description": "Optional pre-truncation embedding dim for query_vector" },
                "k": { "type": "integer", "default": 10, "minimum": 1, "maximum": 200 },
                "dim": { "type": "integer", "default": 512, "minimum": 1, "maximum": 512 },
                "filters": {
                    "type": "object",
                    "properties": {
                        "source_kind": { "type": "string", "enum": ["file", "memory", "laptop_note"] },
                        "trust": { "type": "string", "enum": ["trusted", "untrusted"] },
                        "owner": { "type": "string" }
                    }
                }
            },
            "required": ["folder_id"]
        })
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let mgr = require_manager(&self.ctx)?;
        let folder_id = require_str(&params, "folder_id")?;
        let dim = parse_dim(&params)?;
        let k = params
            .get("k")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(10);

        let query = params
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let query_vector: Option<Vec<f32>> =
            match params.get("query_vector") {
                None | Some(Value::Null) => None,
                Some(raw_q) => Some(serde_json::from_value(raw_q.clone()).map_err(|e| {
                    RpcError::invalid_params(format!("invalid 'query_vector': {e}"))
                })?),
            };
        if query.is_none() && query_vector.is_none() {
            return Err(RpcError::invalid_params(
                "either 'query' or 'query_vector' is required",
            ));
        }

        let query_model = params
            .get("query_model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let query_dim = match params.get("query_dim") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|&n| n > 0)
                    .ok_or_else(|| RpcError::invalid_params("'query_dim' must be positive"))?,
            ),
        };

        let filter = parse_search_filter(&params)?;
        let response = mgr
            .search_folder(
                FolderSearchRequest {
                    folder_id: folder_id.to_string(),
                    dim,
                    k,
                    query,
                    query_vector,
                    query_model,
                    query_dim,
                    filter,
                },
                self.ctx.emb.as_ref(),
                self.ctx.bulk_emb.as_ref(),
            )
            .await
            .map_err(map_core_err)?;

        serde_json::to_value(response).map_err(|e| RpcError::internal(e.to_string()))
    }
}

fn parse_search_filter(params: &Value) -> Result<SearchFilter, RpcError> {
    let container = params
        .get("filters")
        .filter(|v| v.is_object())
        .unwrap_or(params);

    let source_kind = match container.get("source_kind").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Some(SourceKind::parse(s).map_err(map_core_err)?),
        _ => None,
    };
    let trust = match container.get("trust").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Some(TrustLevel::parse(s).map_err(map_core_err)?),
        _ => None,
    };
    let owner = container
        .get("owner")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Ok(SearchFilter {
        source_kind,
        trust,
        owner,
    })
}

// ─────────────────────────────────────────────────────────────────────
// folder_index_states
// ─────────────────────────────────────────────────────────────────────

pub struct FolderIndexStatesHandler {
    ctx: Arc<ToolContext>,
}

impl FolderIndexStatesHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for FolderIndexStatesHandler {
    fn name(&self) -> &str {
        "folder_index_states"
    }

    fn description(&self) -> &str {
        "Report every file's indexing state (indexed, indexing done/total, or skipped with reason) plus separate vector-index and SQLite sizes."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "folder_id": { "type": "string" },
                "file_path": { "type": "string", "description": "Optional single file filter" },
                "dim": { "type": "integer", "default": 512, "minimum": 1, "maximum": 512 }
            },
            "required": ["folder_id"]
        })
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let mgr = require_manager(&self.ctx)?;
        let folder_id = require_str(&params, "folder_id")?;
        let dim = parse_dim(&params)?;

        let index = mgr.get_or_open(folder_id, dim).map_err(map_core_err)?;
        let files = match params.get("file_path").and_then(Value::as_str) {
            Some(fp) if !fp.is_empty() => match index.file_state(fp).map_err(map_core_err)? {
                Some(rec) => vec![rec],
                None => Vec::new(),
            },
            _ => index.file_states().map_err(map_core_err)?,
        };
        let sizes = index.size_report().map_err(map_core_err)?;
        let status = index.model_status().map_err(map_core_err)?;

        let mut out = json!({
            "folder_id": folder_id,
            "files": files,
            "vector_index_bytes": sizes.vector_index_bytes,
            "sqlite_bytes": sizes.sqlite_bytes,
            "chunk_count": sizes.chunk_count,
            "index_state": status.index_state,
            "halted": status.halted,
            "generation": status.generation,
            "unproven_vectors_since_gen": status.unproven_vectors_since_gen,
        });
        if let Some(obj) = out.as_object_mut() {
            if let Some(m) = status.embed_model {
                obj.insert("embed_model".to_string(), json!(m));
            }
            if let Some(d) = status.embed_dim {
                obj.insert("embed_dim".to_string(), json!(d));
            }
            if let Some(until) = status.rebuild_deferred_until {
                obj.insert("rebuild_deferred_until".to_string(), json!(until));
            }
            if let Some(reason) = status.stale_reason {
                obj.insert("stale_reason".to_string(), json!(reason));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::register_folder_index;
    use super::*;
    use crate::dispatch::ToolRegistry;
    use makakoo_core::superbrain::folder_index::{FolderIndexManager, SkipReason};
    use sha2::{Digest, Sha256};

    fn setup() -> (tempfile::TempDir, ToolRegistry) {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = Arc::new(FolderIndexManager::new(tmp.path().join("indexes")));
        let ctx = Arc::new(ToolContext::empty(tmp.path().to_path_buf()).with_folder_index(mgr));
        let mut reg = ToolRegistry::new();
        register_folder_index(&mut reg, ctx);
        (tmp, reg)
    }

    #[tokio::test]
    async fn full_mcp_roundtrip_upsert_search_skip_states_remove() {
        let (_tmp, reg) = setup();
        let mut v = vec![0.0_f32; 32];
        v[3] = 1.0;

        let md = "Clause 4: delivery due within 10 business days.";
        let sha = hex::encode(Sha256::digest(md.as_bytes()));

        let up = reg
            .call(
                "folder_index_upsert",
                json!({
                    "folder_id": "proj-1",
                    "file_path": "contract.pdf",
                    "file_version": "v1",
                    "source_kind": "file",
                    "trust": "trusted",
                    "owner": "alice",
                    "dim": 32,
                    "embed_model": "test-model",
                    "embed_dim": 32,
                    "file_engine_output": {
                        "contract": "v1",
                        "format": "pdf",
                        "pages": [{
                            "page_number": 4,
                            "unit": "page",
                            "title": null,
                            "markdown": md,
                            "markdown_sha256": sha
                        }],
                        "warnings": []
                    },
                    "vectors": [v]
                }),
            )
            .await
            .unwrap();
        assert_eq!(up["state"], "indexed");
        assert_eq!(up["chunks_written"], 1);

        let search = reg
            .call(
                "folder_index_search",
                json!({
                    "folder_id": "proj-1",
                    "query_vector": v,
                    "k": 5,
                    "dim": 32,
                    "filters": { "source_kind": "file", "trust": "trusted", "owner": "alice" }
                }),
            )
            .await
            .unwrap();
        let passages = search["passages"].as_array().unwrap();
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0]["anchor"]["kind"], "page");
        assert_eq!(passages[0]["anchor"]["value"], "4");
        assert_eq!(passages[0]["text"], md);
        assert!(passages[0].get("owner").is_none());

        let skip = reg
            .call(
                "folder_index_mark_skipped",
                json!({
                    "folder_id": "proj-1",
                    "file_path": "locked.pdf",
                    "file_version": "v1",
                    "reason": SkipReason::Password.as_str(),
                    "dim": 32
                }),
            )
            .await
            .unwrap();
        assert_eq!(skip["state"], "skipped");
        assert_eq!(skip["reason"], SkipReason::Password.as_str());

        let states = reg
            .call(
                "folder_index_states",
                json!({ "folder_id": "proj-1", "dim": 32 }),
            )
            .await
            .unwrap();
        assert_eq!(states["files"].as_array().unwrap().len(), 2);

        let rem = reg
            .call(
                "folder_index_remove",
                json!({ "folder_id": "proj-1", "file_path": "contract.pdf", "dim": 32 }),
            )
            .await
            .unwrap();
        assert_eq!(rem["removed"], true);
    }

    #[tokio::test]
    async fn mcp_rejects_precomputed_vectors_without_model_provenance() {
        let (_tmp, reg) = setup();
        let err = reg
            .call(
                "folder_index_upsert",
                json!({
                    "folder_id": "no-meta",
                    "file_path": "a.md",
                    "file_version": "v1",
                    "dim": 4,
                    "chunks": [{
                        "anchor": { "kind": "heading", "value": "A" },
                        "char_start": 0,
                        "char_end": 1,
                        "text": "A",
                        "vector": [1.0, 0.0, 0.0, 0.0]
                    }]
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("embed_model"));
    }

    #[tokio::test]
    async fn mark_skipped_schema_and_handler_derive_from_skip_reason() {
        let (_tmp, reg) = setup();
        let desc = reg
            .list()
            .into_iter()
            .find(|t| t.name == "folder_index_mark_skipped")
            .expect("folder_index_mark_skipped registered");
        let schema_enum = desc.input_schema["properties"]["reason"]["enum"]
            .as_array()
            .expect("reason enum array");
        let expected: Vec<Value> = SkipReason::ALL.iter().map(|r| json!(r.as_str())).collect();
        assert_eq!(schema_enum, &expected);

        for (idx, reason) in SkipReason::ALL.iter().enumerate() {
            let res = reg
                .call(
                    "folder_index_mark_skipped",
                    json!({
                        "folder_id": "proj-skip",
                        "file_path": format!("file-{idx}.pdf"),
                        "file_version": "v1",
                        "reason": reason.as_str(),
                        "dim": 32
                    }),
                )
                .await
                .unwrap();
            assert_eq!(res["state"], "skipped");
            assert_eq!(res["reason"], reason.as_str());
        }

        let err = reg
            .call(
                "folder_index_mark_skipped",
                json!({
                    "folder_id": "proj-skip",
                    "file_path": "bad.pdf",
                    "file_version": "v1",
                    "reason": "unsupported",
                    "dim": 32
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }
}
