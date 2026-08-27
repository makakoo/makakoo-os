//! AgentSpec — declarative YAML/TOML definition of a Makakoo agent.
//!
//! The spec describes the agent's cognitive core (model, instructions,
//! tools, scope) plus its communications interfaces (`channels`) and
//! trigger sources (`triggers`). The renderer (Phase 4) maps a
//! validated spec to a slot TOML + Flue project.
//!
//! Conceptual model:
//!
//! * **AGENT** — `name`, `description`, `model`, `instructions`,
//!   `tools`, `scope`
//! * **CHANNELS** — communications interfaces set ON the agent.
//!   Bidirectional single list. Zero or many.
//! * **TRIGGERS** — when the agent starts without a user message.
//!   Zero or many.

pub mod convert;
pub mod discovery;
pub mod parser;
pub mod validate;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Top-level agent spec — the cognitive core plus its I/O surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec {
    /// Agent identity — matches Flue project naming.
    /// Regex: `^[a-z0-9][a-z0-9-]{0,62}$`.
    pub name: String,

    /// Human-facing description.
    pub description: String,

    /// Model identifier (e.g. `anthropic/claude-sonnet-4-6`).
    pub model: String,

    /// System prompt / persona. Multi-line, markdown allowed.
    pub instructions: String,

    /// Whitelist of `mcp__harvey__*` tool names this agent may invoke.
    /// Validated at create time against the registered MCP tool set.
    pub tools: Vec<String>,

    /// Communications interfaces set ON the agent.
    /// Bidirectional single list. Zero or many.
    #[serde(default)]
    pub channels: Vec<ChannelSpec>,

    /// Trigger sources — when the agent starts without a user message.
    /// Zero or many.
    #[serde(default)]
    pub triggers: Vec<TriggerSpec>,

    /// Filesystem read/write boundaries.
    pub scope: ScopeSpec,
}

/// Communications channel. Bidirectional unless the kind implies
/// otherwise (webhook is inbound-only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ChannelSpec {
    /// Telegram bot. Bidirectional.
    Telegram {
        /// Required. Env var name holding the bot token.
        token_env: String,
        /// Optional. Telegram user IDs allowed to interact.
        #[serde(default)]
        allowed_users: Vec<String>,
    },
    /// Slack bot. Bidirectional (Socket Mode).
    Slack {
        token_env: String,
        app_token_env: String,
        /// Env var name holding the workspace `team_id` (e.g. `T0123ABCD`).
        team_id_env: String,
        #[serde(default)]
        allowed_users: Vec<String>,
    },
    /// Discord bot. Bidirectional.
    Discord {
        token_env: String,
        #[serde(default)]
        allowed_users: Vec<String>,
    },
    /// Webhook. Inbound only.
    Webhook { path: String, secret_env: String },
    /// Email via SMTP/IMAP. Bidirectional.
    Email {
        smtp_host: String,
        imap_host: String,
        secret_env: String,
    },
    /// Voice via Twilio. Bidirectional.
    Voice {
        twilio_account_sid_env: String,
        secret_env: String,
    },
}

/// Trigger — when the agent starts without a user message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum TriggerSpec {
    Cron {
        /// Standard 5-field cron: `* * * * *`.
        schedule: String,
        /// Optional. IANA timezone (e.g. `UTC`, `Europe/Berlin`).
        /// Empty string means `UTC`.
        #[serde(default)]
        timezone: String,
    },
    Webhook {
        path: String,
        secret_env: String,
    },
}

/// Scope — filesystem read/write boundaries.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScopeSpec {
    /// Filesystem prefixes or prefix globs. Absolute paths and `~/...` are supported.
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    /// Globs, always denied even if matched by `allowed_paths`.
    #[serde(default)]
    pub forbidden_paths: Vec<String>,
}

/// Name regex: lowercase letter or digit, then up to 62 of [a-z0-9-].
pub const NAME_REGEX: &str = r"^[a-z0-9][a-z0-9-]{0,62}$";

impl AgentSpec {
    /// Schema-level validation. No I/O, no network.
    pub fn validate(&self) -> Result<()> {
        validate::validate(self)
    }

    /// Parse a spec from a file. Detects format by extension
    /// (`.yaml`, `.yml`, `.toml`).
    pub fn load_from_file(path: &std::path::Path) -> Result<Self> {
        parser::load_from_file(path)
    }

    /// Render this spec back to its source format (YAML).
    pub fn to_yaml(&self) -> Result<String> {
        parser::to_yaml(self)
    }

    /// Render this spec back to TOML.
    pub fn to_toml(&self) -> Result<String> {
        parser::to_toml(self)
    }
}

impl std::fmt::Display for AgentSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AgentSpec({})", self.name)
    }
}
