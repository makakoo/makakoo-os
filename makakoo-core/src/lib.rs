//! makakoo-core — foundation library for the Rust rewrite of Makakoo OS.
//!
//! Wave 1 scope: platform abstraction, config loader, LLM client, embeddings
//! client, rusqlite setup with migrations, unified error type.
//!
//! Later waves layer superbrain store, event bus, chat store, MCP server,
//! CLI, SANCHO, nursery, etc. on top of these primitives.

pub mod agents;
pub mod capability;
pub mod chat;
pub mod config;
pub mod db;
pub mod distro;
pub mod embeddings;
pub mod error;
pub mod event_bus;
pub mod gimmicks;
pub mod gym;
pub mod llm;
pub mod memory;
pub mod nursery;
pub mod outbound;
pub mod platform;
pub mod plugin;
pub mod sancho;
pub mod superbrain;
pub mod swarm;
pub mod telemetry;
pub mod wiki;

pub use error::{MakakooError, Result};

/// Crate-wide test helpers. Shared mutex serializes any test that
/// mutates `MAKAKOO_HOME` (a process-global env var) to prevent
/// flakes when cargo runs unit tests in parallel.
#[cfg(test)]
pub(crate) mod test_lock {
    use std::sync::Mutex;
    pub(crate) static ENV_GUARD: Mutex<()> = Mutex::new(());
    pub(crate) fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }
}
