//! makakoo-core — foundation library for the Rust rewrite of Makakoo OS.
//!
//! Wave 1 scope: platform abstraction, config loader, LLM client, embeddings
//! client, rusqlite setup with migrations, unified error type.
//!
//! Later waves layer superbrain store, event bus, chat store, MCP server,
//! CLI, SANCHO, nursery, etc. on top of these primitives.

pub mod agents;
pub mod chat;
pub mod config;
pub mod db;
pub mod embeddings;
pub mod error;
pub mod event_bus;
pub mod gimmicks;
pub mod llm;
pub mod nursery;
pub mod outbound;
pub mod platform;
pub mod sancho;
pub mod superbrain;
pub mod swarm;
pub mod telemetry;
pub mod wiki;

pub use error::{MakakooError, Result};
