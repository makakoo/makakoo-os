//! Tier-A brain handlers: FTS5 search, vector search, LLM-synthesised
//! query, recent docs, entity graph neighbours, and memory-stack context
//! assembly. All read-only. All routed through `SuperbrainStore`,
//! `GraphStore`, `MemoryStack`, and `LlmClient` on the shared
//! `ToolContext`.

use async_trait::async_trait;
use makakoo_core::llm::ChatMessage as LlmMessage;
use makakoo_core::superbrain::memory_stack::{SemanticHit, SessionIdentity};
use makakoo_core::superbrain::recall::RecallItem;
use makakoo_core::superbrain::store::{SearchHit, VectorHit};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

/// Record a batch of FTS search hits into `recall_log`. Non-fatal —
/// tracker absence or insert failure only warns in tracing.
fn track_search_hits(ctx: &ToolContext, query: &str, hits: &[SearchHit], source: &str) {
    let Some(tracker) = ctx.recall.as_ref() else {
        return;
    };
    if hits.is_empty() || query.is_empty() {
        return;
    }
    let items: Vec<RecallItem> = hits
        .iter()
        .map(|h| {
            RecallItem::new(0, h.doc_id.clone(), h.content.clone())
                .with_score(h.score as f64)
                .with_source(source.to_string())
        })
        .collect();
    if let Err(e) = tracker.track_batch(&items, query) {
        tracing::warn!("recall track_batch failed (source={source}): {e}");
    }
}

/// Record a batch of vector-similarity hits into `recall_log`. Non-fatal.
fn track_vector_hits(ctx: &ToolContext, query: &str, hits: &[VectorHit], source: &str) {
    let Some(tracker) = ctx.recall.as_ref() else {
        return;
    };
    if hits.is_empty() || query.is_empty() {
        return;
    }
    let items: Vec<RecallItem> = hits
        .iter()
        .map(|h| {
            RecallItem::new(0, h.doc_id.clone(), h.content.clone())
                .with_score(h.similarity as f64)
                .with_source(source.to_string())
        })
        .collect();
    if let Err(e) = tracker.track_batch(&items, query) {
        tracing::warn!("recall track_batch (vector) failed (source={source}): {e}");
    }
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

fn require_query(params: &Value) -> Result<String, RpcError> {
    params
        .get("query")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RpcError::invalid_params("missing or empty 'query'"))
}

fn optional_u64(params: &Value, key: &str, default: u64) -> u64 {
    params.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn optional_str(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

const BRAIN_QUERY_MODEL: &str = "minimax/ail-compound";

// ─────────────────────────────────────────────────────────────────────
// brain_search  +  harvey_brain_search  (same backing call)
// ─────────────────────────────────────────────────────────────────────

pub struct BrainSearchHandler {
    ctx: Arc<ToolContext>,
}

impl BrainSearchHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }

    /// Shared search body. Alias handlers call this with their own
    /// `recall_source` string so `recall_log.source` reflects which MCP
    /// tool name the client invoked.
    async fn search_with_source(
        &self,
        params: Value,
        recall_source: &str,
    ) -> Result<Value, RpcError> {
        let query = require_query(&params)?;
        let limit = optional_u64(&params, "limit", 10) as usize;
        let store = self
            .ctx
            .store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: store"))?;
        let hits = store
            .search(&query, limit)
            .map_err(|e| RpcError::internal(format!("brain_search: {e}")))?;
        track_search_hits(&self.ctx, &query, &hits, recall_source);
        Ok(json!(hits))
    }
}

#[async_trait]
impl ToolHandler for BrainSearchHandler {
    fn name(&self) -> &str {
        "brain_search"
    }
    fn description(&self) -> &str {
        "FTS5 full-text search over the Brain. Returns BM25-ranked hits with \
         journal recency boost."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Free-text query" },
                "limit": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 }
            },
            "required": ["query"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        self.search_with_source(params, "mcp:brain_search").await
    }
}

/// Python-parity alias for `brain_search`.
pub struct HarveyBrainSearchHandler {
    inner: BrainSearchHandler,
}

impl HarveyBrainSearchHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self {
            inner: BrainSearchHandler::new(ctx),
        }
    }
}

#[async_trait]
impl ToolHandler for HarveyBrainSearchHandler {
    fn name(&self) -> &str {
        "harvey_brain_search"
    }
    fn description(&self) -> &str {
        "Alias for brain_search. Search Harvey's Brain via FTS5."
    }
    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        self.inner
            .search_with_source(params, "mcp:harvey_brain_search")
            .await
    }
}

// ─────────────────────────────────────────────────────────────────────
// brain_query  +  harvey_superbrain_query (FTS5 + LLM synthesis)
// ─────────────────────────────────────────────────────────────────────

pub struct BrainQueryHandler {
    ctx: Arc<ToolContext>,
}

impl BrainQueryHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }

    async fn run(&self, params: Value, recall_source: &str) -> Result<Value, RpcError> {
        let query = require_query(&params)?;
        let top_k = optional_u64(&params, "top_k", 5) as usize;

        let store = self
            .ctx
            .store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: store"))?;
        let hits = store
            .search(&query, top_k)
            .map_err(|e| RpcError::internal(format!("brain_query: {e}")))?;

        // Record underlying FTS hits regardless of LLM synthesis path.
        track_search_hits(&self.ctx, &query, &hits, recall_source);

        // No LLM wired → return the hits unchanged with an empty answer.
        let llm = match self.ctx.llm.as_ref() {
            Some(l) => l,
            None => {
                return Ok(json!({
                    "answer": "",
                    "sources": hits,
                }));
            }
        };

        if hits.is_empty() {
            return Ok(json!({
                "answer": "No relevant documents found in the Brain.",
                "sources": [],
            }));
        }

        // Build a compact context from top hits (≤6000 chars).
        let mut ctx_buf = String::new();
        for h in hits.iter() {
            let snippet = h.content.chars().take(800).collect::<String>();
            ctx_buf.push_str(&format!("- [{}] {}\n", h.doc_id, snippet));
            if ctx_buf.len() > 6000 {
                break;
            }
        }

        let system = "You are Harvey, the user's cognitive extension. \
                      Answer strictly from the provided Brain excerpts. \
                      Be concise and cite doc ids inline as [doc_id]."
            .to_string();
        let user = format!(
            "Question: {query}\n\nBrain excerpts:\n{ctx_buf}\n\n\
             Answer in 2-5 sentences, grounded in the excerpts."
        );

        let answer = llm
            .chat(
                BRAIN_QUERY_MODEL,
                vec![LlmMessage::system(system), LlmMessage::user(user)],
            )
            .await
            .unwrap_or_else(|_| {
                // LLM failure → still return hits with a fallback note.
                "(llm call failed — raw hits returned below)".to_string()
            });

        Ok(json!({
            "answer": answer,
            "sources": hits,
        }))
    }
}

#[async_trait]
impl ToolHandler for BrainQueryHandler {
    fn name(&self) -> &str {
        "brain_query"
    }
    fn description(&self) -> &str {
        "FTS5 search + LLM synthesis. Returns a grounded answer plus the \
         source hits it was built from."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "top_k": { "type": "integer", "default": 5 }
            },
            "required": ["query"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        self.run(params, "mcp:brain_query").await
    }
}

pub struct HarveySuperbrainQueryHandler {
    inner: BrainQueryHandler,
}

impl HarveySuperbrainQueryHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self {
            inner: BrainQueryHandler::new(ctx),
        }
    }
}

#[async_trait]
impl ToolHandler for HarveySuperbrainQueryHandler {
    fn name(&self) -> &str {
        "harvey_superbrain_query"
    }
    fn description(&self) -> &str {
        "Alias for brain_query. Search Harvey's Brain + synthesize an answer."
    }
    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        self.inner
            .run(params, "mcp:harvey_superbrain_query")
            .await
    }
}

// ─────────────────────────────────────────────────────────────────────
// brain_recent
// ─────────────────────────────────────────────────────────────────────

pub struct BrainRecentHandler {
    ctx: Arc<ToolContext>,
}

impl BrainRecentHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for BrainRecentHandler {
    fn name(&self) -> &str {
        "brain_recent"
    }
    fn description(&self) -> &str {
        "Return the most recently updated Brain documents. Optional \
         doc_type filter: 'journal', 'page', 'auto_memory', etc."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "default": 10 },
                "doc_type": { "type": "string" }
            }
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let limit = optional_u64(&params, "limit", 10) as usize;
        let doc_type = optional_str(&params, "doc_type");
        let store = self
            .ctx
            .store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: store"))?;
        let hits = store
            .recent(limit, doc_type.as_deref())
            .map_err(|e| RpcError::internal(format!("brain_recent: {e}")))?;
        Ok(json!(hits))
    }
}

// ─────────────────────────────────────────────────────────────────────
// brain_entities — graph neighbours + god nodes
// ─────────────────────────────────────────────────────────────────────

pub struct BrainEntitiesHandler {
    ctx: Arc<ToolContext>,
}

impl BrainEntitiesHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for BrainEntitiesHandler {
    fn name(&self) -> &str {
        "brain_entities"
    }
    fn description(&self) -> &str {
        "Return outgoing + incoming graph neighbours for an entity, plus \
         the current top god nodes for context."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Entity name (wikilink target)" },
                "god_limit": { "type": "integer", "default": 10 }
            },
            "required": ["name"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("missing 'name'"))?
            .to_string();
        let god_limit = optional_u64(&params, "god_limit", 10) as usize;
        let graph = self
            .ctx
            .graph
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: graph"))?;
        let (outgoing, incoming) = graph
            .neighbors(&name)
            .map_err(|e| RpcError::internal(format!("brain_entities: {e}")))?;
        let god_nodes = graph
            .god_nodes(god_limit)
            .map_err(|e| RpcError::internal(format!("brain_entities: {e}")))?;
        Ok(json!({
            "entity": name,
            "neighbors": {
                "outgoing": outgoing,
                "incoming": incoming,
            },
            "god_nodes": god_nodes,
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// brain_context — memory stack assembly
// ─────────────────────────────────────────────────────────────────────

pub struct BrainContextHandler {
    ctx: Arc<ToolContext>,
}

impl BrainContextHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for BrainContextHandler {
    fn name(&self) -> &str {
        "brain_context"
    }
    fn description(&self) -> &str {
        "Assemble a compact Brain context string for the given query via \
         the memory stack (L0 identity, L1 recent journals, L2 semantic \
         top-k, L3 graph neighbourhood)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "budget_tokens": { "type": "integer", "default": 512 }
            },
            "required": ["query"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let query = require_query(&params)?;
        let budget = optional_u64(&params, "budget_tokens", 512) as usize;
        let memory = self
            .ctx
            .memory
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: memory"))?;

        // Build semantic hits from vector_search if we have an embeddings
        // client and a store; otherwise skip L2.
        let semantic: Vec<SemanticHit> = match (self.ctx.emb.as_ref(), self.ctx.store.as_ref()) {
            (Some(emb), Some(store)) => match emb.embed(&query).await {
                Ok(qvec) => match store.vector_search(&qvec, 5) {
                    Ok(vhits) => vhits
                        .into_iter()
                        .map(|v| SemanticHit {
                            doc_id: v.doc_id,
                            content: v.content,
                            similarity: v.similarity,
                        })
                        .collect(),
                    Err(_) => Vec::new(),
                },
                Err(_) => Vec::new(),
            },
            _ => Vec::new(),
        };

        let identity =
            SessionIdentity::new("You are Harvey, the user's autonomous cognitive extension.");
        let context = memory
            .assemble_context(&identity, &query, &semantic, budget)
            .map_err(|e| RpcError::internal(format!("brain_context: {e}")))?;
        Ok(json!({ "context": context, "budget_tokens": budget }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// harvey_superbrain_vector_search
// ─────────────────────────────────────────────────────────────────────

pub struct HarveySuperbrainVectorSearchHandler {
    ctx: Arc<ToolContext>,
}

impl HarveySuperbrainVectorSearchHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for HarveySuperbrainVectorSearchHandler {
    fn name(&self) -> &str {
        "harvey_superbrain_vector_search"
    }
    fn description(&self) -> &str {
        "Embed the query via switchAILocal and run brute-force cosine \
         similarity against stored Brain vectors."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "default": 10 }
            },
            "required": ["query"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let query = require_query(&params)?;
        let limit = optional_u64(&params, "limit", 10) as usize;
        let store = self
            .ctx
            .store
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: store"))?;
        let emb = self
            .ctx
            .emb
            .as_ref()
            .ok_or_else(|| RpcError::internal("subsystem not wired: embeddings"))?;
        let qvec = emb
            .embed(&query)
            .await
            .map_err(|e| RpcError::internal(format!("embed: {e}")))?;
        let hits = store
            .vector_search(&qvec, limit)
            .map_err(|e| RpcError::internal(format!("vector_search: {e}")))?;
        track_vector_hits(
            &self.ctx,
            &query,
            &hits,
            "mcp:harvey_superbrain_vector_search",
        );
        Ok(json!(hits))
    }
}

// ─────────────────────────────────────────────────────────────────────
// Tests — schema + parameter validation. Real store calls go through
// the integration tests at the bottom of the module.
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn empty_ctx() -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(PathBuf::from("/tmp/makakoo-t13-test")))
    }

    #[tokio::test]
    async fn brain_search_requires_query() {
        let h = BrainSearchHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn brain_search_missing_store_returns_internal() {
        let h = BrainSearchHandler::new(empty_ctx());
        let err = h.call(json!({"query": "foo"})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
        assert!(err.message.contains("store"));
    }

    #[test]
    fn brain_search_schema_has_query_required() {
        let h = BrainSearchHandler::new(empty_ctx());
        let schema = h.input_schema();
        assert_eq!(schema["required"][0], "query");
    }

    #[tokio::test]
    async fn brain_entities_requires_name() {
        let h = BrainEntitiesHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn brain_recent_accepts_empty_params() {
        let h = BrainRecentHandler::new(empty_ctx());
        // No store wired → internal error but no invalid_params.
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn brain_context_requires_query() {
        let h = BrainContextHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn vector_search_requires_query() {
        let h = HarveySuperbrainVectorSearchHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[test]
    fn harvey_brain_search_is_alias_name() {
        let h = HarveyBrainSearchHandler::new(empty_ctx());
        assert_eq!(h.name(), "harvey_brain_search");
    }

    #[test]
    fn harvey_superbrain_query_name() {
        let h = HarveySuperbrainQueryHandler::new(empty_ctx());
        assert_eq!(h.name(), "harvey_superbrain_query");
    }

    // ── Phase A — recall-tracking integration tests ──────────────────
    //
    // These wire a real SuperbrainStore + RecallTracker into a
    // ToolContext, invoke the handler, and assert `recall_log` has a
    // row. They prove MCP brain searches from any CLI now feed the
    // memory promoter.

    use makakoo_core::superbrain::recall::RecallTracker;
    use makakoo_core::superbrain::store::SuperbrainStore;
    use tempfile::tempdir;

    fn wired_ctx(dir: &std::path::Path) -> (Arc<ToolContext>, Arc<SuperbrainStore>) {
        let store = Arc::new(SuperbrainStore::open(&dir.join("sb.db")).unwrap());
        let tracker = Arc::new(RecallTracker::new(store.conn_arc()));
        // Seed one document so search has something to return.
        store
            .write_document(
                "/tmp/makakoo-test-doc.md",
                "the quick brown fox jumps over the lazy dog",
                "page",
                serde_json::json!([]),
            )
            .unwrap();
        let ctx = Arc::new(
            ToolContext::empty(dir.to_path_buf())
                .with_store(store.clone())
                .with_recall(tracker),
        );
        (ctx, store)
    }

    fn count_recall(store: &SuperbrainStore) -> i64 {
        let conn = store.conn_arc();
        let conn = conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM recall_log", [], |r| r.get(0))
            .unwrap()
    }

    fn first_recall_source(store: &SuperbrainStore) -> Option<String> {
        let conn = store.conn_arc();
        let conn = conn.lock().unwrap();
        conn.query_row(
            "SELECT source FROM recall_log ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok()
    }

    #[tokio::test]
    async fn brain_search_writes_recall_log() {
        let dir = tempdir().unwrap();
        let (ctx, store) = wired_ctx(dir.path());
        let h = BrainSearchHandler::new(ctx);
        let out = h.call(json!({"query": "fox"})).await.unwrap();
        assert!(out.is_array());
        assert!(!out.as_array().unwrap().is_empty(), "should find the doc");
        assert_eq!(count_recall(&store), 1);
        assert_eq!(first_recall_source(&store).as_deref(), Some("mcp:brain_search"));
    }

    #[tokio::test]
    async fn brain_search_without_tracker_is_safe() {
        let dir = tempdir().unwrap();
        // Wire store but NOT recall — handler must still succeed.
        let store = Arc::new(SuperbrainStore::open(&dir.path().join("sb.db")).unwrap());
        store
            .write_document(
                "/tmp/makakoo-no-track.md",
                "hello world",
                "page",
                serde_json::json!([]),
            )
            .unwrap();
        let ctx = Arc::new(ToolContext::empty(dir.path().to_path_buf()).with_store(store));
        let h = BrainSearchHandler::new(ctx);
        let out = h.call(json!({"query": "hello"})).await.unwrap();
        assert!(out.is_array());
    }

    #[tokio::test]
    async fn harvey_brain_search_uses_aliased_source() {
        let dir = tempdir().unwrap();
        let (ctx, store) = wired_ctx(dir.path());
        let h = HarveyBrainSearchHandler::new(ctx);
        h.call(json!({"query": "fox"})).await.unwrap();
        assert_eq!(
            first_recall_source(&store).as_deref(),
            Some("mcp:harvey_brain_search")
        );
    }

    #[tokio::test]
    async fn brain_query_records_recall_for_each_hit() {
        let dir = tempdir().unwrap();
        let (ctx, store) = wired_ctx(dir.path());
        // Seed a second doc so query returns multiple hits.
        store
            .as_ref()
            .write_document(
                "/tmp/makakoo-extra-doc.md",
                "the lazy fox hops",
                "page",
                serde_json::json!([]),
            )
            .unwrap();
        let h = BrainQueryHandler::new(ctx);
        h.call(json!({"query": "fox", "top_k": 5})).await.unwrap();
        assert!(count_recall(&store) >= 1);
        assert_eq!(
            first_recall_source(&store).as_deref(),
            Some("mcp:brain_query")
        );
    }

    #[tokio::test]
    async fn harvey_superbrain_query_uses_aliased_source() {
        let dir = tempdir().unwrap();
        let (ctx, store) = wired_ctx(dir.path());
        let h = HarveySuperbrainQueryHandler::new(ctx);
        h.call(json!({"query": "fox"})).await.unwrap();
        assert_eq!(
            first_recall_source(&store).as_deref(),
            Some("mcp:harvey_superbrain_query")
        );
    }

    #[tokio::test]
    async fn brain_search_empty_query_does_not_record() {
        let dir = tempdir().unwrap();
        let (ctx, store) = wired_ctx(dir.path());
        let h = BrainSearchHandler::new(ctx);
        let err = h.call(json!({"query": ""})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
        assert_eq!(count_recall(&store), 0);
    }

    #[tokio::test]
    async fn brain_search_no_hits_does_not_record() {
        let dir = tempdir().unwrap();
        let (ctx, store) = wired_ctx(dir.path());
        let h = BrainSearchHandler::new(ctx);
        h.call(json!({"query": "zxqwvnonsenseXYZ"}))
            .await
            .unwrap();
        assert_eq!(count_recall(&store), 0);
    }
}
