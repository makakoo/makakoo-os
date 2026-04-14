//! `makakoo mcp` — re-exec the `makakoo-mcp` sibling binary.
//!
//! The distribution bundle puts `makakoo` and `makakoo-mcp` in the
//! same directory (release tarball, Homebrew keg, cargo-dist output).
//! We resolve the sibling via `std::env::current_exe()` so the
//! subcommand stays wired no matter where the user installed it.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

/// Forward to the `makakoo-mcp` binary with `args` appended. Spawns a
/// child process and waits; the child inherits stdin/stdout/stderr so
/// the MCP framing round-trip reaches its real transport.
pub fn run(args: Vec<String>) -> Result<i32> {
    let bin = resolve_mcp_binary()?;
    let status = Command::new(&bin)
        .args(&args)
        .status()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Locate the `makakoo-mcp` binary next to the current `makakoo`
/// binary. Falls back to `$PATH` lookup if the sibling slot is empty
/// (useful for `cargo install` layouts where binaries land in `~/.cargo/bin`).
fn resolve_mcp_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to resolve current exe")?;
    if let Some(parent) = exe.parent() {
        let sibling = parent.join(mcp_binary_filename());
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    // PATH fallback — let the OS resolver find it.
    Ok(PathBuf::from("makakoo-mcp"))
}

#[cfg(windows)]
fn mcp_binary_filename() -> &'static str {
    "makakoo-mcp.exe"
}

#[cfg(not(windows))]
fn mcp_binary_filename() -> &'static str {
    "makakoo-mcp"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_returns_something() {
        // Just make sure the resolver doesn't panic in a clean env.
        let p = resolve_mcp_binary().unwrap();
        assert!(!p.as_os_str().is_empty());
    }
}
