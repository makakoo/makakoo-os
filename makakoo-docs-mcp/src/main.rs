// Entry point for the standalone `makakoo-docs-mcp` binary.
//
// Usage: `makakoo-docs-mcp --stdio`
// (Phase D: also reachable as `makakoo docs-mcp --stdio` once bundled.)
//
// The server loop lives in the library crate (`lib.rs` → `run_stdio()`).
// This file is a thin wrapper that handles the flag check, tracing init,
// and tokio runtime — nothing else.

use anyhow::Result;
use tracing_subscriber::EnvFilter;

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

    makakoo_docs_mcp::run_stdio().await
}
