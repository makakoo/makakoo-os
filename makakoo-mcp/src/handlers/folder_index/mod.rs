//! MCP tool handlers for per-folder compact vector indexes:
//!
//! Write handlers live here (`folder_index_upsert`, `folder_index_mark_skipped`,
//! `folder_index_remove`); read/search handlers live in [`read`].

#[cfg(test)]
mod model_tests;
mod read;

use std::sync::Arc;

use async_trait::async_trait;
use makakoo_core::superbrain::folder_index::{
    chunk_file_engine_output, Anchor, ChunkInput, ChunkOptions, FileEngineOutput,
    FolderIndexManager, SkipReason, SourceKind, TrustLevel, UpsertFileRequest, UpsertModelMeta,
    DEFAULT_VECTOR_DIM,
};
use makakoo_core::MakakooError;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler, ToolRegistry};
use crate::jsonrpc::RpcError;

/// Register all 5 folder-index MCP handlers.
pub fn register_folder_index(registry: &mut ToolRegistry, ctx: Arc<ToolContext>) {
    registry.register(Arc::new(FolderIndexUpsertHandler::new(Arc::clone(&ctx))));
    registry.register(Arc::new(FolderIndexMarkSkippedHandler::new(Arc::clone(
        &ctx,
    ))));
    registry.register(Arc::new(FolderIndexRemoveHandler::new(Arc::clone(&ctx))));
    registry.register(Arc::new(read::FolderIndexSearchHandler::new(Arc::clone(
        &ctx,
    ))));
    registry.register(Arc::new(read::FolderIndexStatesHandler::new(ctx)));
}

pub(super) fn require_manager(ctx: &ToolContext) -> Result<&Arc<FolderIndexManager>, RpcError> {
    ctx.folder_index
        .as_ref()
        .ok_or_else(|| RpcError::internal("subsystem not wired: folder_index"))
}

pub(super) fn map_core_err(err: MakakooError) -> RpcError {
    match err {
        MakakooError::InvalidInput(msg) => RpcError::invalid_params(msg),
        MakakooError::RetryableFolderModelMismatch(msg) => {
            let mut error = RpcError::internal(format!("retryable folder model mismatch: {msg}"));
            error.data = Some(json!({ "retryable": true }));
            error
        }
        other => RpcError::internal(other.to_string()),
    }
}

pub(super) fn require_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("missing or empty '{key}'")))
}

pub(super) fn parse_dim(params: &Value) -> Result<usize, RpcError> {
    match params.get("dim") {
        None | Some(Value::Null) => Ok(DEFAULT_VECTOR_DIM),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| RpcError::invalid_params("'dim' must be a positive integer")),
    }
}

fn parse_flush(params: &Value) -> bool {
    params.get("flush").and_then(Value::as_bool).unwrap_or(true)
}

#[derive(Deserialize)]
struct WireChunkInput {
    anchor: Anchor,
    char_start: usize,
    char_end: usize,
    text: String,
    vector: Vec<f32>,
}

// ─────────────────────────────────────────────────────────────────────
// folder_index_upsert
// ─────────────────────────────────────────────────────────────────────

pub struct FolderIndexUpsertHandler {
    ctx: Arc<ToolContext>,
}

impl FolderIndexUpsertHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for FolderIndexUpsertHandler {
    fn name(&self) -> &str {
        "folder_index_upsert"
    }

    fn description(&self) -> &str {
        "Add or replace a file's chunks and compact int8 vectors in a folder index (idempotent by file_version). Accepts pre-chunked vectors or file-engine wire output."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "folder_id": { "type": "string", "description": "Knowledge folder identifier" },
                "file_path": { "type": "string", "description": "Relative file path inside the folder" },
                "file_version": { "type": "string", "description": "Stable content/version hash for idempotency" },
                "source_kind": { "type": "string", "enum": ["file", "memory", "laptop_note"], "default": "file" },
                "trust": { "type": "string", "enum": ["trusted", "untrusted"], "default": "trusted" },
                "owner": { "type": "string", "default": "" },
                "dim": { "type": "integer", "default": 512, "minimum": 1, "maximum": 512 },
                "flush": { "type": "boolean", "default": true, "description": "Persist vector index immediately; set false for batched multi-file runs" },
                "chunks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "anchor": {
                                "type": "object",
                                "properties": {
                                    "kind": { "type": "string", "enum": ["page", "sheet", "slide", "heading"] },
                                    "value": { "type": "string" }
                                },
                                "required": ["kind", "value"]
                            },
                            "char_start": { "type": "integer", "minimum": 0 },
                            "char_end": { "type": "integer", "minimum": 0 },
                            "text": { "type": "string" },
                            "vector": { "type": "array", "items": { "type": "number" } }
                        },
                        "required": ["anchor", "char_start", "char_end", "text", "vector"]
                    }
                },
                "file_engine_output": { "type": "object", "description": "Raw file-engine v1/v2 extraction JSON (alternative to chunks)" },
                "vectors": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } },
                "embed_model": { "type": "string", "description": "Optional reported embedding model id for precomputed vectors" },
                "embed_dim": { "type": "integer", "minimum": 1, "description": "Optional pre-truncation embedding dim for precomputed vectors" },
                "chunk_options": { "type": "object" }
            },
            "required": ["folder_id", "file_path", "file_version"]
        })
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let mgr = require_manager(&self.ctx)?;
        let folder_id = require_str(&params, "folder_id")?;
        let file_path = require_str(&params, "file_path")?;
        let file_version = require_str(&params, "file_version")?;
        let dim = parse_dim(&params)?;
        let flush = parse_flush(&params);

        let source_kind = match params.get("source_kind").and_then(Value::as_str) {
            Some(s) => SourceKind::parse(s).map_err(map_core_err)?,
            None => SourceKind::File,
        };
        let trust = match params.get("trust").and_then(Value::as_str) {
            Some(s) => TrustLevel::parse(s).map_err(map_core_err)?,
            None => TrustLevel::Trusted,
        };
        let owner = params
            .get("owner")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let (chunks, model_meta) = self.resolve_chunks(&params).await?;
        let (outcome, sizes) = mgr
            .upsert_folder_file_and_flush(
                folder_id,
                dim,
                &UpsertFileRequest {
                    file_path: file_path.to_string(),
                    file_version: file_version.to_string(),
                    source_kind,
                    trust,
                    owner,
                    chunks,
                },
                model_meta.as_ref(),
                flush,
            )
            .await
            .map_err(map_core_err)?;

        Ok(json!({
            "folder_id": folder_id,
            "file_path": file_path,
            "file_version": file_version,
            "state": "indexed",
            "replaced": outcome.replaced,
            "skipped_unchanged": outcome.skipped_unchanged,
            "chunks_written": outcome.chunks_written,
            "vector_index_bytes": sizes.vector_index_bytes,
            "sqlite_bytes": sizes.sqlite_bytes,
        }))
    }
}

impl FolderIndexUpsertHandler {
    fn parse_explicit_model_meta(
        &self,
        params: &Value,
        chunks: &[ChunkInput],
    ) -> Result<Option<UpsertModelMeta>, RpcError> {
        let explicit_model = params
            .get("embed_model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let explicit_dim = match params.get("embed_dim") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|&n| n > 0)
                    .ok_or_else(|| RpcError::invalid_params("'embed_dim' must be positive"))?,
            ),
        };
        let Some(model) = explicit_model else {
            return Ok(None);
        };
        let embed_dim = explicit_dim
            .or_else(|| chunks.first().map(|c| c.vector.len()))
            .filter(|&d| d > 0)
            .ok_or_else(|| RpcError::invalid_params("'embed_dim' required when chunks empty"))?;
        let bulk_alias = self
            .ctx
            .bulk_emb
            .as_ref()
            .or(self.ctx.emb.as_ref())
            .map(|e| e.model().to_string())
            .unwrap_or_else(|| model.to_string());
        let short_alias = self.ctx.emb.as_ref().map(|e| e.model().to_string());
        Ok(Some(UpsertModelMeta {
            model: model.to_string(),
            embed_dim,
            bulk_alias,
            short_alias,
        }))
    }

    async fn resolve_chunks(
        &self,
        params: &Value,
    ) -> Result<(Vec<ChunkInput>, Option<UpsertModelMeta>), RpcError> {
        if let Some(raw_chunks) = params.get("chunks") {
            let parsed: Vec<WireChunkInput> = serde_json::from_value(raw_chunks.clone())
                .map_err(|e| RpcError::invalid_params(format!("invalid 'chunks': {e}")))?;
            let chunks: Vec<ChunkInput> = parsed
                .into_iter()
                .map(|c| ChunkInput {
                    anchor: c.anchor,
                    char_start: c.char_start,
                    char_end: c.char_end,
                    text: c.text,
                    vector: c.vector,
                })
                .collect();
            let meta = self.parse_explicit_model_meta(params, &chunks)?;
            if !chunks.is_empty() && meta.is_none() {
                return Err(RpcError::invalid_params(
                    "'embed_model' is required for precomputed chunk vectors",
                ));
            }
            return Ok((chunks, meta));
        }
        let Some(fe_val) = params.get("file_engine_output") else {
            return Err(RpcError::invalid_params(
                "either 'chunks' or 'file_engine_output' is required",
            ));
        };
        let fe: FileEngineOutput = serde_json::from_value(fe_val.clone())
            .map_err(|e| RpcError::invalid_params(format!("invalid 'file_engine_output': {e}")))?;
        let opts: ChunkOptions = match params.get("chunk_options") {
            Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                .map_err(|e| RpcError::invalid_params(format!("invalid 'chunk_options': {e}")))?,
            _ => ChunkOptions::default(),
        };
        let text_chunks = chunk_file_engine_output(&fe, &opts).map_err(map_core_err)?;
        let mut meta: Option<UpsertModelMeta> = None;
        let vectors: Vec<Vec<f32>> = if let Some(raw_vecs) = params.get("vectors") {
            serde_json::from_value(raw_vecs.clone())
                .map_err(|e| RpcError::invalid_params(format!("invalid 'vectors': {e}")))?
        } else if text_chunks.is_empty() {
            Vec::new()
        } else {
            let emb = self
                .ctx
                .bulk_emb
                .as_ref()
                .or(self.ctx.emb.as_ref())
                .ok_or_else(|| RpcError::internal("subsystem not wired: embeddings"))?;
            let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
            let res = emb
                .embed_batch_with_meta(&texts)
                .await
                .map_err(|e| RpcError::internal(format!("embedding batch failed: {e}")))?;
            meta = Some(UpsertModelMeta {
                model: res.model,
                embed_dim: res.dim,
                bulk_alias: emb.model().to_string(),
                short_alias: self.ctx.emb.as_ref().map(|e| e.model().to_string()),
            });
            res.vectors
        };
        if vectors.len() != text_chunks.len() {
            return Err(RpcError::invalid_params(format!(
                "vectors length ({}) does not match chunk count ({})",
                vectors.len(),
                text_chunks.len()
            )));
        }
        let chunks: Vec<ChunkInput> = text_chunks
            .into_iter()
            .zip(vectors)
            .map(|(c, vector)| ChunkInput {
                anchor: c.anchor,
                char_start: c.char_start,
                char_end: c.char_end,
                text: c.text,
                vector,
            })
            .collect();
        if meta.is_none() {
            meta = self.parse_explicit_model_meta(params, &chunks)?;
        }
        if !chunks.is_empty() && meta.is_none() {
            return Err(RpcError::invalid_params(
                "'embed_model' is required when supplying precomputed vectors",
            ));
        }
        Ok((chunks, meta))
    }
}

// ─────────────────────────────────────────────────────────────────────
// folder_index_mark_skipped
// ─────────────────────────────────────────────────────────────────────

pub struct FolderIndexMarkSkippedHandler {
    ctx: Arc<ToolContext>,
}

impl FolderIndexMarkSkippedHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for FolderIndexMarkSkippedHandler {
    fn name(&self) -> &str {
        "folder_index_mark_skipped"
    }

    fn description(&self) -> &str {
        "Mark a file in a folder index as skipped with an explicit SkipReason and purge any stale chunks."
    }

    fn input_schema(&self) -> Value {
        let reasons = SkipReason::ALL.map(SkipReason::as_str);
        json!({
            "type": "object",
            "properties": {
                "folder_id": { "type": "string" },
                "file_path": { "type": "string" },
                "file_version": { "type": "string" },
                "reason": { "type": "string", "enum": reasons },
                "dim": { "type": "integer", "default": 512, "minimum": 1, "maximum": 512 },
                "flush": { "type": "boolean", "default": true }
            },
            "required": ["folder_id", "file_path", "file_version", "reason"]
        })
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let mgr = require_manager(&self.ctx)?;
        let folder_id = require_str(&params, "folder_id")?;
        let file_path = require_str(&params, "file_path")?;
        let file_version = require_str(&params, "file_version")?;
        let reason = SkipReason::parse(require_str(&params, "reason")?).map_err(map_core_err)?;
        let dim = parse_dim(&params)?;
        let flush = parse_flush(&params);

        let index = mgr.get_or_open(folder_id, dim).map_err(map_core_err)?;
        index
            .mark_file_skipped(file_path, file_version, reason)
            .map_err(map_core_err)?;
        if flush {
            index.flush().map_err(map_core_err)?;
        }

        Ok(json!({
            "folder_id": folder_id,
            "file_path": file_path,
            "file_version": file_version,
            "state": "skipped",
            "reason": reason.as_str(),
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// folder_index_remove
// ─────────────────────────────────────────────────────────────────────

pub struct FolderIndexRemoveHandler {
    ctx: Arc<ToolContext>,
}

impl FolderIndexRemoveHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for FolderIndexRemoveHandler {
    fn name(&self) -> &str {
        "folder_index_remove"
    }

    fn description(&self) -> &str {
        "Remove a file's chunks, vectors, and state record from a folder index."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "folder_id": { "type": "string" },
                "file_path": { "type": "string" },
                "dim": { "type": "integer", "default": 512, "minimum": 1, "maximum": 512 },
                "flush": { "type": "boolean", "default": true }
            },
            "required": ["folder_id", "file_path"]
        })
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let mgr = require_manager(&self.ctx)?;
        let folder_id = require_str(&params, "folder_id")?;
        let file_path = require_str(&params, "file_path")?;
        let dim = parse_dim(&params)?;
        let flush = parse_flush(&params);

        let index = mgr.get_or_open(folder_id, dim).map_err(map_core_err)?;
        let removed = index.remove_file(file_path).map_err(map_core_err)?;
        if flush && removed {
            index.flush().map_err(map_core_err)?;
        }

        Ok(json!({
            "folder_id": folder_id,
            "file_path": file_path,
            "removed": removed,
        }))
    }
}
