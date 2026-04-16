//! Capability subsystem — the kernel ⇄ plugin honesty boundary.
//!
//! Spec source of truth: `spec/CAPABILITIES.md` v0.1. Every plugin
//! declares the verbs it needs in `[capabilities].grants`. The kernel
//! parses those declarations into a typed `GrantTable` and enforces
//! them when the plugin makes RPC calls through the per-plugin Unix
//! domain socket.
//!
//! **Phase E/1 scope** (this module): verb vocabulary + scope parsing
//! and matching, grant resolver with auto-defaults, append-only audit
//! log. The Unix socket + PID-verified handshake that _calls_ this
//! grant table lands in Phase E/2; the Rust/Python/Node client
//! libraries in Phase E/3.
//!
//! Until E/2, the grant table is not load-bearing — plugins still run
//! unsandboxed as before. But shipping the manifest ⇄ grant-table
//! parse path now lets us (a) validate shipped plugins against the
//! real vocabulary, (b) write `makakoo plugin info` output that shows
//! the parsed grants, and (c) give reviewers a machine-readable
//! manifest audit trail.

pub mod audit;
pub mod grants;
pub mod verb;

pub use audit::{AuditEntry, AuditLog, AuditResult, RotationError};
pub use grants::{resolve_grants, GrantCheck, GrantTable, ResolveError};
pub use verb::{
    normalize_grant, parse_grant, scope_matches, Verb, VerbError, KNOWN_VERBS,
    SCOPE_REQUIRED_VERBS,
};
