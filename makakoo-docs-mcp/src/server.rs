// MCP server handler. Phase B: scaffold with no tools registered.
// Phase C: register search / read / list / topic tools via #[tool_router].

use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{
        Implementation, InitializeResult, ProtocolVersion, ServerCapabilities,
    },
    tool_handler, tool_router, ServerHandler,
};

use crate::index::Index;

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
        info.instructions = Some(
            "Search and read Makakoo OS public docs. Tools land in Phase C: \
             makakoo_docs_search / read / list / topic."
                .to_string(),
        );
        info
    }
}
