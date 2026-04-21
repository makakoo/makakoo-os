//! `makakoo` — umbrella CLI for Makakoo OS.
//!
//! This binary is the unified entry point the user's shell and every
//! infected CLI host invokes. It covers the read paths that power
//! everyday workflow (search, query, promotions) and the write paths
//! that touch persistent state (nursery hatch, sancho tick, dream,
//! skill runner). The MCP server is exposed as a subcommand that
//! re-execs the `makakoo-mcp` binary side-by-side in the same release
//! dir so distribution stays a single bundle.
//!
//! T16 wave 5 scope. Daemon + infect subcommands land in T17.
//!
//! The crate-level `#[allow(dead_code)]` is intentional: wave 5 lands
//! a wider public API surface than the subcommands actually consume
//! (e.g. `ctx.chat()`, `ctx.graph()`, secrets key constants) so later
//! waves can wire new subcommands without reshaping core types.

#![allow(dead_code)]

use clap::Parser;

mod cli;
mod commands;
mod context;
mod daemon;
mod detect;
mod infect;
mod output;
mod secrets;
mod skill_runner;
#[cfg(test)]
mod test_support;

use cli::Cli;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // Logs to stderr — stdout is reserved for human-readable command
    // output (tables, JSON). Format + default level controlled by the
    // shared makakoo-core helper; $MAKAKOO_LOG_FORMAT selects compact|
    // pretty|json output.
    makakoo_core::telemetry::init_stderr("warn");

    let cli = Cli::parse();
    let ctx = context::CliContext::new()?;
    match commands::dispatch(cli.command, &ctx).await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            output::print_error(format!("{e:#}"));
            std::process::exit(1);
        }
    }
}
