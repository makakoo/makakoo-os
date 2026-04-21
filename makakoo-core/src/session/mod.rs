//! v0.2 Phase G — JSONL session-tree subsystem.
//!
//! Entry types + append/read/fork plumbing land first (G.1). Label +
//! rewind (G.3), CLI wiring (G.2/G.4), and feature-flag integration
//! with the agent event loop (G.5) follow in subsequent commits.
//!
//! Default: OFF. The kernel only instantiates this subsystem when
//! `kernel.session_tree = true` in config. `rewind` is non-destructive —
//! the original file is kept as `<id>.<ts>.bak.jsonl` so nothing is lost.

pub mod tree;

pub use tree::{find_label, fork, rewind_to_label, Entry, MessageRole, SessionError, SessionTree};
