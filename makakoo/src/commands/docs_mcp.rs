// `makakoo docs-mcp` — Makakoo OS docs MCP server (single-binary, Option A).
//
// Dispatches to `makakoo_docs_mcp::run_stdio()` in-process. The
// standalone `makakoo-docs-mcp` binary does the same — both paths are
// byte-identical in behavior.

use anyhow::Result;
use tracing_subscriber::EnvFilter;

/// Run the docs MCP server if `--stdio` is set; otherwise print usage
/// and exit with code 2.
pub async fn run(stdio: bool) -> Result<i32> {
    if !stdio {
        eprintln!("usage: makakoo docs-mcp --stdio");
        eprintln!();
        eprintln!("Stdio JSON-RPC MCP server for Makakoo OS docs. Add to your");
        eprintln!("AI CLI's MCP config — see docs/docs-mcp-setup.md.");
        return Ok(2);
    }

    // Initialise tracing for this invocation path. The main makakoo
    // binary already called `makakoo_core::telemetry::init_stderr` at a
    // "warn" level; upgrade to the user-controlled $RUST_LOG filter if
    // present so MCP-debug output works as expected.
    // `try_init` is used so we don't panic if the subscriber was already
    // set by a test harness or a previous call.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();

    makakoo_docs_mcp::run_stdio().await?;
    Ok(0)
}
