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

pub mod scaffold;
pub use scaffold::{AgentKind, AgentScaffold, AgentSpec};
