// MCP server handler for the docs MCP.
//
// Phase C: 4 tools wired (search / read / list / topic). Each tool is
// a thin #[tool] method delegating to a `run()` function in the
// matching `tools/*.rs` module. The actual SQLite/FTS5 logic lives in
// `index/runtime.rs`. This file is rmcp glue only.

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, InitializeResult, ProtocolVersion,
        ServerCapabilities,
    },
    tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};

use crate::index::Index;
use crate::tools::{
    list::{self, ListInput},
    read::{self, ReadInput},
    search::{self, SearchInput},
    topic::{self, TopicInput},
};

#[derive(Clone)]
pub struct DocsServer {
    pub index: Index,
    tool_router: ToolRouter<DocsServer>,
}

#[tool_router]
impl DocsServer {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            index: Index::open()?,
            tool_router: Self::tool_router(),
        })
    }

    #[tool(
        description = "Full-text search Makakoo OS docs. Returns up to `limit` hits ordered by BM25 relevance, each with path + title + snippet + score. Use this to find relevant docs, then call makakoo_docs_read on a specific path."
    )]
    async fn makakoo_docs_search(
        &self,
        Parameters(input): Parameters<SearchInput>,
    ) -> Result<CallToolResult, McpError> {
        let hits = search::run(&self.index, input).map_err(internal)?;
        json_result(&hits)
    }

    #[tool(
        description = "Read the full markdown content of a Makakoo doc by repo-relative path (e.g. 'docs/concepts/architecture.md'). Path must come from a prior search/list call — unknown paths return NotFound."
    )]
    async fn makakoo_docs_read(
        &self,
        Parameters(input): Parameters<ReadInput>,
    ) -> Result<CallToolResult, McpError> {
        match read::run(&self.index, input).map_err(internal)? {
            Some(body) => Ok(CallToolResult::success(vec![Content::text(body)])),
            None => Err(McpError::invalid_params("path not in indexed corpus", None)),
        }
    }

    #[tool(
        description = "List indexed Makakoo docs, optionally filtered by path prefix (e.g. 'docs/concepts/'). Returns one entry per doc with path + size + title."
    )]
    async fn makakoo_docs_list(
        &self,
        Parameters(input): Parameters<ListInput>,
    ) -> Result<CallToolResult, McpError> {
        let entries = list::run(&self.index, input).map_err(internal)?;
        json_result(&entries)
    }

    #[tool(
        description = "Resolve a topic name (e.g. 'agent', 'infect', 'brain') to its canonical Makakoo doc plus a breadcrumb and a list of related docs in the same directory."
    )]
    async fn makakoo_docs_topic(
        &self,
        Parameters(input): Parameters<TopicInput>,
    ) -> Result<CallToolResult, McpError> {
        let res = topic::run(&self.index, input).map_err(internal)?;
        json_result(&res)
    }
}

#[tool_handler]
impl ServerHandler for DocsServer {
    fn get_info(&self) -> InitializeResult {
        // E0639: InitializeResult and Implementation are #[non_exhaustive]
        // — must mutate after Default rather than using a struct literal.
        let mut server_info = Implementation::default();
        server_info.name = "makakoo-docs-mcp".to_string();
        server_info.version = env!("CARGO_PKG_VERSION").to_string();

        let mut info = InitializeResult::default();
        info.protocol_version = ProtocolVersion::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = server_info;
        info.instructions = Some(format!(
            "Search and read Makakoo OS public docs ({} indexed). Tools: \
             makakoo_docs_search / read / list / topic.",
            self.index.doc_count
        ));
        info
    }
}

fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string(value)
        .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

fn internal(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}
