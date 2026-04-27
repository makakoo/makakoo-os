// Entry point for the Makakoo OS docs MCP server.
//
// Usage: `makakoo-docs-mcp --stdio`
// (Phase D: also reachable as `makakoo docs-mcp --stdio` once bundled.)

use anyhow::Result;
use rmcp::{transport::stdio, ServiceExt};
use tracing_subscriber::EnvFilter;

mod index;
mod server;
mod tools;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--stdio") {
        eprintln!("usage: makakoo-docs-mcp --stdio");
        eprintln!();
        eprintln!("Stdio JSON-RPC MCP server for Makakoo OS docs. Add to your");
        eprintln!("AI CLI's MCP config — see docs/docs-mcp-setup.md.");
        std::process::exit(2);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("makakoo-docs-mcp starting (stdio transport)");

    let docs = server::DocsServer::new()?;
    let service = docs.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;
    service.waiting().await?;
    Ok(())
}
