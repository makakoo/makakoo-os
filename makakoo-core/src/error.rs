//! Unified error type for makakoo-core.
//!
//! Every fallible function in this crate returns `Result<T>`, where
//! `MakakooError` covers IO, SQLite, HTTP, JSON, config, LLM, and generic
//! internal errors. This is the single conversion boundary for `?` in the
//! crate — higher-level layers (MCP server, CLI) can re-wrap it with
//! `anyhow` if they prefer the dynamic error style.

use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MakakooError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("sqlite error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("llm error: {0}")]
    Llm(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, MakakooError>;

impl MakakooError {
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn llm(msg: impl Into<String>) -> Self {
        Self::Llm(msg.into())
    }

    /// Caller-misuse errors — wrong shape, violated precondition, etc.
    /// Use this (not `internal`) so surface layers can translate to the
    /// right MCP error code / HTTP 400 instead of 500.
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }
}
