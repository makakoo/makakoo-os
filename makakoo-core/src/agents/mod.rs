//! Agents subsystem — install / uninstall / list / info / create.
//!
//! Python source: `core/agents/scaffold.py`. The Python impl bundles
//! heavy Python-specific templates (daemon / cron / cli agent.py
//! boilerplate). The Rust port keeps the structural operations —
//! create a directory, parse an `agent.toml`, reject duplicates,
//! uninstall with a locked-dir safety check — and leaves language-
//! specific template generation to the callers (the Rust CLI wires
//! up a small set of templates in a later task).
//!
//! Each agent lives at `{agents_dir}/<name>/` with:
//!
//! * `agent.toml` — canonical `AgentSpec` config
//! * `README.md` — human-facing description
//! * entry file (`run.py`, `run.sh`, or a Rust `Cargo.toml`-backed
//!   binary depending on `kind`).

pub mod identity;
pub mod lifecycle;
pub mod migrate;
pub mod registry;
pub mod scaffold;
pub mod scope;
pub mod slot;
pub mod status;

pub use identity::{
    load_identity, render_identity_block, resolve, slot_from_env, AgentIdentity, IdentityError,
    ENV_VAR as AGENT_SLOT_ENV_VAR, EX_USAGE as AGENT_SLOT_EXIT_CODE,
};
pub use registry::AgentRegistry;
pub use scope::{check_path, check_tool, ScopeError};
pub use slot::{registry_dir, slot_path, validate_slot_id, AgentSlot};

pub use lifecycle::{
    AgentLaunchSpec, AgentProcess, AgentSupervisor, HealthStatus,
};
pub use scaffold::{AgentKind, AgentScaffold, AgentSpec};
