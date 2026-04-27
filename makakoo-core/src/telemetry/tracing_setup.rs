//! Shared tracing initializer for every Makakoo binary.
//!
//! v0.2 Phase E.1. Until now `makakoo` and `makakoo-mcp` each built a
//! [`tracing_subscriber`] from scratch. Duplication meant env-var
//! handling drifted: `makakoo` defaulted to `warn`, `makakoo-mcp` to
//! `info`, and neither knew how to emit JSON logs. This module owns
//! the initializer both binaries call.
//!
//! Env vars respected (in priority order):
//!
//! * `MAKAKOO_LOG_FORMAT` — `json` | `pretty` | `compact` (default).
//! * `RUST_LOG` — standard EnvFilter directive.
//!
//! All output goes to stderr. stdout is reserved for the binary's
//! actual interface — JSON-RPC for `makakoo-mcp`, human tables / JSON
//! for `makakoo` subcommands.
//!
//! Calling [`init_stderr`] is idempotent — the underlying
//! `try_init` call short-circuits on a global subscriber that's
//! already set, so a test binary that installs its own subscriber
//! won't be overwritten.

use tracing_subscriber::EnvFilter;

/// Default EnvFilter when neither RUST_LOG nor the fallback arg is set.
pub const DEFAULT_ENV_FILTER: &str = "info";

/// Log format selector. Defaults to `Compact`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// `tracing_subscriber::fmt().json()` — one structured JSON line
    /// per event. Preferred for production + log shipping.
    Json,
    /// `tracing_subscriber::fmt().pretty()` — multi-line human output.
    /// Handy when tailing a debug session in a terminal.
    Pretty,
    /// `tracing_subscriber::fmt().compact()` — the traditional default.
    Compact,
}

impl LogFormat {
    /// Parse `MAKAKOO_LOG_FORMAT`. Unknown values fall back to Compact
    /// rather than crashing the boot path.
    pub fn from_env() -> Self {
        match std::env::var("MAKAKOO_LOG_FORMAT")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "json" => LogFormat::Json,
            "pretty" => LogFormat::Pretty,
            _ => LogFormat::Compact,
        }
    }
}

/// Install the global tracing subscriber with stderr output.
///
/// `default_directive` is used only when `RUST_LOG` is unset. Pass
/// e.g. `"warn"` for CLI binaries, `"info"` for long-running daemons.
///
/// Safe to call multiple times — the first caller wins, later callers
/// silently no-op (matches `try_init` semantics).
pub fn init_stderr(default_directive: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_directive));

    match LogFormat::from_env() {
        LogFormat::Json => {
            let _ = tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .with_target(false)
                .try_init();
        }
        LogFormat::Pretty => {
            let _ = tracing_subscriber::fmt()
                .pretty()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .with_target(false)
                .try_init();
        }
        LogFormat::Compact => {
            let _ = tracing_subscriber::fmt()
                .compact()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .with_target(false)
                .try_init();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_lock::lock_env;

    #[test]
    fn default_format_is_compact() {
        let _g = lock_env();
        let prev = std::env::var("MAKAKOO_LOG_FORMAT").ok();
        std::env::remove_var("MAKAKOO_LOG_FORMAT");
        let got = LogFormat::from_env();
        if let Some(p) = prev {
            std::env::set_var("MAKAKOO_LOG_FORMAT", p);
        }
        assert_eq!(got, LogFormat::Compact);
    }

    #[test]
    fn unknown_format_falls_back_to_compact() {
        let _g = lock_env();
        std::env::set_var("MAKAKOO_LOG_FORMAT", "banana");
        assert_eq!(LogFormat::from_env(), LogFormat::Compact);
        std::env::remove_var("MAKAKOO_LOG_FORMAT");
    }

    #[test]
    fn case_insensitive_format_selection() {
        let _g = lock_env();
        std::env::set_var("MAKAKOO_LOG_FORMAT", "JSON");
        assert_eq!(LogFormat::from_env(), LogFormat::Json);
        std::env::set_var("MAKAKOO_LOG_FORMAT", "Pretty");
        assert_eq!(LogFormat::from_env(), LogFormat::Pretty);
        std::env::remove_var("MAKAKOO_LOG_FORMAT");
    }

    #[test]
    fn init_stderr_is_idempotent() {
        // First call wins; subsequent calls silently no-op.
        init_stderr("warn");
        init_stderr("info");
        init_stderr("trace");
        // If try_init panicked on second invocation the test would have
        // aborted; arriving here is the assertion.
    }
}
