//! Tool registry, `ToolHandler` trait, and the shared `ToolContext` that
//! lets Wave 4 parallel agents (T13 read tools, T14 write tools, T15 heavy
//! tools + swarm) plug handlers into the server spine.
//!
//! # How T13/T14/T15 add a handler
//!
//! Create a struct and `impl ToolHandler for it` — that's it:
//!
//! ```ignore
//! use async_trait::async_trait;
//! use serde_json::{json, Value};
//! use std::sync::Arc;
//! use makakoo_mcp::dispatch::{ToolContext, ToolHandler};
//! use makakoo_mcp::jsonrpc::RpcError;
//!
//! pub struct BrainSearchHandler {
//!     ctx: Arc<ToolContext>,
//! }
//!
//! impl BrainSearchHandler {
//!     pub fn new(ctx: Arc<ToolContext>) -> Self { Self { ctx } }
//! }
//!
//! #[async_trait]
//! impl ToolHandler for BrainSearchHandler {
//!     fn name(&self) -> &str { "harvey_brain_search" }
//!     fn description(&self) -> &str {
//!         "Search Harvey's Brain (Brain journals + pages) with access \
//!          control and audit logging."
//!     }
//!     fn input_schema(&self) -> Value {
//!         json!({
//!             "type": "object",
//!             "properties": {
//!                 "query": {"type": "string"},
//!                 "top_k": {"type": "integer", "default": 10}
//!             },
//!             "required": ["query"]
//!         })
//!     }
//!     async fn call(&self, params: Value) -> Result<Value, RpcError> {
//!         let query = params.get("query").and_then(Value::as_str)
//!             .ok_or_else(|| RpcError::invalid_params("missing 'query'"))?;
//!         let top_k = params.get("top_k").and_then(Value::as_u64).unwrap_or(10) as usize;
//!         let hits = self.ctx.store.search_fts(query, top_k)
//!             .map_err(|e| RpcError::internal(e.to_string()))?;
//!         Ok(json!(hits))
//!     }
//! }
//! ```
//!
//! Then in `handlers/mod.rs::register_all`:
//!
//! ```ignore
//! registry.register(Arc::new(tier_a::BrainSearchHandler::new(ctx.clone())));
//! ```

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::jsonrpc::{RpcError, METHOD_NOT_FOUND};

use makakoo_core::agents::AgentScaffold;
use makakoo_core::channel_ops::ChannelOpsRegistry;
use makakoo_core::chat::ChatStore;
use makakoo_core::embeddings::EmbeddingClient;
use makakoo_core::event_bus::PersistentEventBus;
use makakoo_core::llm::LlmClient;
use makakoo_core::nursery::{BuddyTracker, MascotRegistry};
use makakoo_core::outbound::OutboundQueue;
use makakoo_core::superbrain::graph::GraphStore;
use makakoo_core::superbrain::memory_stack::MemoryStack;
use makakoo_core::superbrain::promoter::MemoryPromoter;
use makakoo_core::superbrain::recall::RecallTracker;
use makakoo_core::superbrain::store::SuperbrainStore;
use makakoo_core::swarm::SwarmState;
use makakoo_core::telemetry::CostTracker;

tokio::task_local! {
    /// The originating subagent slot id for the current MCP
    /// invocation, propagated from `X-Makakoo-Agent-Id` header
    /// (HTTP path) or `MAKAKOO_AGENT_SLOT` env var (stdio path).
    ///
    /// `None` when the call did not originate from a subagent
    /// (e.g. CLI / human operator). Tool handlers consult this
    /// when filtering grants by `bound_to_agent`, prefixing brain
    /// journal lines, or attributing cost-tracker entries.
    pub static AGENT_ID: Option<String>;
}

/// Convenience wrapper: read the current agent id, returning
/// `None` outside an `AGENT_ID::scope` (e.g. unit tests or stdio
/// without `MAKAKOO_AGENT_SLOT`).
pub fn current_agent_id() -> Option<String> {
    AGENT_ID
        .try_with(|v| v.clone())
        .ok()
        .flatten()
}

/// Shared, read-mostly context handed to every tool handler at
/// construction time. Wrap in `Arc` once at server boot and clone the Arc
/// into each handler struct.
///
/// Every field is optional so T12 can construct a minimal context for
/// `--health` / `--list-tools` and unit tests without touching the real
/// filesystem. T13/T14/T15 handlers that need a specific subsystem must
/// return `RpcError::internal("subsystem not wired")` when it is `None`.
///
/// `makakoo-mcp serve` fills every slot via `main::build_context`.
pub struct ToolContext {
    #[allow(dead_code)]
    pub home: PathBuf,
    pub store: Option<Arc<SuperbrainStore>>,
    pub graph: Option<Arc<GraphStore>>,
    pub memory: Option<Arc<MemoryStack>>,
    pub recall: Option<Arc<RecallTracker>>,
    pub promoter: Option<Arc<MemoryPromoter>>,
    pub bus: Option<Arc<PersistentEventBus>>,
    pub llm: Option<Arc<LlmClient>>,
    pub emb: Option<Arc<EmbeddingClient>>,
    pub chat_store: Option<Arc<ChatStore>>,
    pub nursery: Option<Arc<MascotRegistry>>,
    pub buddy: Option<Arc<BuddyTracker>>,
    pub outbound: Option<Arc<OutboundQueue>>,
    pub costs: Option<Arc<CostTracker>>,
    pub agents: Option<Arc<AgentScaffold>>,
    pub swarm_state: Option<Arc<SwarmState>>,
    pub channel_ops: Option<Arc<ChannelOpsRegistry>>,
}

impl ToolContext {
    /// Construct an empty context rooted at the given Makakoo home
    /// directory. Suitable for tests and for `--health` / `--list-tools`
    /// where no subsystem is needed.
    pub fn empty(home: PathBuf) -> Self {
        Self {
            home,
            store: None,
            graph: None,
            memory: None,
            recall: None,
            promoter: None,
            bus: None,
            llm: None,
            emb: None,
            chat_store: None,
            nursery: None,
            buddy: None,
            outbound: None,
            costs: None,
            agents: None,
            swarm_state: None,
            channel_ops: None,
        }
    }

    /// Fluent setter sugar — the `build_context` helper uses this to wire
    /// subsystems one at a time without shadowing the whole struct. Most
    /// of these are unused at T12 and get activated as T13/T14/T15 add
    /// handlers that need the corresponding subsystem.
    #[allow(dead_code)]
    pub fn with_store(mut self, store: Arc<SuperbrainStore>) -> Self {
        self.store = Some(store);
        self
    }
    #[allow(dead_code)]
    pub fn with_graph(mut self, graph: Arc<GraphStore>) -> Self {
        self.graph = Some(graph);
        self
    }
    #[allow(dead_code)]
    pub fn with_memory(mut self, memory: Arc<MemoryStack>) -> Self {
        self.memory = Some(memory);
        self
    }
    #[allow(dead_code)]
    pub fn with_recall(mut self, recall: Arc<RecallTracker>) -> Self {
        self.recall = Some(recall);
        self
    }
    #[allow(dead_code)]
    pub fn with_promoter(mut self, promoter: Arc<MemoryPromoter>) -> Self {
        self.promoter = Some(promoter);
        self
    }
    #[allow(dead_code)]
    pub fn with_bus(mut self, bus: Arc<PersistentEventBus>) -> Self {
        self.bus = Some(bus);
        self
    }
    pub fn with_llm(mut self, llm: Arc<LlmClient>) -> Self {
        self.llm = Some(llm);
        self
    }
    pub fn with_embeddings(mut self, emb: Arc<EmbeddingClient>) -> Self {
        self.emb = Some(emb);
        self
    }
    #[allow(dead_code)]
    pub fn with_chat(mut self, chat_store: Arc<ChatStore>) -> Self {
        self.chat_store = Some(chat_store);
        self
    }
    #[allow(dead_code)]
    pub fn with_nursery(mut self, nursery: Arc<MascotRegistry>) -> Self {
        self.nursery = Some(nursery);
        self
    }
    #[allow(dead_code)]
    pub fn with_buddy(mut self, buddy: Arc<BuddyTracker>) -> Self {
        self.buddy = Some(buddy);
        self
    }
    #[allow(dead_code)]
    pub fn with_outbound(mut self, outbound: Arc<OutboundQueue>) -> Self {
        self.outbound = Some(outbound);
        self
    }
    #[allow(dead_code)]
    pub fn with_costs(mut self, costs: Arc<CostTracker>) -> Self {
        self.costs = Some(costs);
        self
    }
    #[allow(dead_code)]
    pub fn with_agents(mut self, agents: Arc<AgentScaffold>) -> Self {
        self.agents = Some(agents);
        self
    }
    #[allow(dead_code)]
    pub fn with_swarm_state(mut self, swarm_state: Arc<SwarmState>) -> Self {
        self.swarm_state = Some(swarm_state);
        self
    }
    #[allow(dead_code)]
    pub fn with_channel_ops(mut self, registry: Arc<ChannelOpsRegistry>) -> Self {
        self.channel_ops = Some(registry);
        self
    }
}

/// One MCP tool = one type implementing this trait. Handlers are
/// registered into a `ToolRegistry` at server boot.
///
/// Send + Sync because the registry is shared across every inbound
/// `tools/call` — the stdio event loop is single-threaded today but
/// routing on a `tokio::spawn` per request is a cheap upgrade.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Stable tool name (e.g. `harvey_brain_search`). Must be unique.
    fn name(&self) -> &str;

    /// Human-facing description emitted on `tools/list`.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's input parameters. Emitted verbatim on
    /// `tools/list` under the `inputSchema` key — OpenAI function-calling
    /// compatible.
    fn input_schema(&self) -> Value;

    /// Execute the tool against parsed arguments. Returns a JSON value
    /// to wrap in the `content[].text` field, or a structured `RpcError`.
    async fn call(&self, params: Value) -> Result<Value, RpcError>;
}

/// Descriptor returned inside the `tools/list` response array.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Name-keyed registry of tool handlers. Built once at boot, then shared
/// read-only behind an `Arc` to the server.
#[derive(Default)]
pub struct ToolRegistry {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler. Overwrites any previous entry with the same
    /// name — last-writer-wins, same as Python's `TOOLS` list.
    #[allow(dead_code)]
    pub fn register(&mut self, h: Arc<dyn ToolHandler>) {
        self.handlers.insert(h.name().to_string(), h);
    }

    /// Number of registered tools. `--health` prints this.
    pub fn count(&self) -> usize {
        self.handlers.len()
    }

    /// All tool descriptors, sorted alphabetically by name for stable
    /// `tools/list` output across runs (easier diffing in tests).
    pub fn list(&self) -> Vec<ToolDescriptor> {
        let mut v: Vec<ToolDescriptor> = self
            .handlers
            .values()
            .map(|h| ToolDescriptor {
                name: h.name().to_string(),
                description: h.description().to_string(),
                input_schema: h.input_schema(),
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Dispatch a `tools/call` by tool name. Returns METHOD_NOT_FOUND if
    /// the tool is unknown (mapped to `isError` by the server).
    pub async fn call(&self, name: &str, params: Value) -> Result<Value, RpcError> {
        match self.handlers.get(name) {
            Some(h) => h.call(params).await,
            None => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown tool: {}", name),
            )),
        }
    }

    /// Whether a tool with the given name is registered.
    #[allow(dead_code)]
    pub fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Echo;

    #[async_trait]
    impl ToolHandler for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echoes its input"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, params: Value) -> Result<Value, RpcError> {
            Ok(params)
        }
    }

    struct Broken;

    #[async_trait]
    impl ToolHandler for Broken {
        fn name(&self) -> &str {
            "broken"
        }
        fn description(&self) -> &str {
            "always fails"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, _: Value) -> Result<Value, RpcError> {
            Err(RpcError::internal("on purpose"))
        }
    }

    #[tokio::test]
    async fn register_and_call() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(Echo));
        assert_eq!(r.count(), 1);
        assert!(r.contains("echo"));

        let got = r.call("echo", json!({"x": 1})).await.unwrap();
        assert_eq!(got, json!({"x": 1}));
    }

    #[tokio::test]
    async fn unknown_tool_returns_method_not_found() {
        let r = ToolRegistry::new();
        let err = r.call("nonexistent", json!({})).await.unwrap_err();
        assert_eq!(err.code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn handler_error_propagates_rpc_error() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(Broken));
        let err = r.call("broken", json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
        assert!(err.message.contains("on purpose"));
    }

    #[tokio::test]
    async fn list_is_sorted_by_name() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(Echo));
        // Register a second fake tool out of order
        struct Zulu;
        #[async_trait]
        impl ToolHandler for Zulu {
            fn name(&self) -> &str {
                "zulu"
            }
            fn description(&self) -> &str {
                ""
            }
            fn input_schema(&self) -> Value {
                json!({"type": "object"})
            }
            async fn call(&self, _: Value) -> Result<Value, RpcError> {
                Ok(json!(null))
            }
        }
        r.register(Arc::new(Zulu));
        let list = r.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "echo");
        assert_eq!(list[1].name, "zulu");
    }

    #[test]
    fn empty_context_has_home() {
        let ctx = ToolContext::empty(PathBuf::from("/tmp/makakoo-test"));
        assert_eq!(ctx.home.as_os_str(), "/tmp/makakoo-test");
        assert!(ctx.store.is_none());
    }
}
