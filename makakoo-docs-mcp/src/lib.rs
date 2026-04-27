// Library entry point for makakoo-docs-mcp.
//
// Exposes `run_stdio()` so the main makakoo binary can call it directly
// from the `docs-mcp --stdio` subcommand (Phase D "Option A — single
// binary multiplexer"). The standalone `makakoo-docs-mcp` binary
// delegates here too, keeping both invocation paths byte-identical.

pub mod index;
pub mod server;
pub mod tools;

/// Run the MCP server on stdin/stdout. Blocks until the client
/// disconnects. Tracing and the tokio runtime must already be
/// initialised by the caller.
pub async fn run_stdio() -> anyhow::Result<()> {
    use rmcp::{transport::stdio, ServiceExt};

    tracing::info!("makakoo-docs-mcp starting (stdio transport)");

    let docs = server::DocsServer::new()?;
    let service = docs.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;
    service.waiting().await?;
    Ok(())
}
