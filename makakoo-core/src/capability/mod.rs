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
pub mod audit_escape;
pub mod grants;
pub mod rate_limit;
pub mod service;
pub mod socket;
pub mod user_grants;
pub mod verb;

pub use audit::{AuditEntry, AuditLog, AuditResult, RotationError};
pub use audit_escape::escape_audit_field;
pub use grants::{resolve_grants, GrantCheck, GrantTable, ResolveError};
pub use rate_limit::{
    check_and_increment as rate_limit_check_and_increment, RateLimitError,
    MAX_ACTIVE_GRANTS, MAX_CREATES_PER_HOUR,
};
pub use service::{
    CompositeHandler, EnvSecretBackend, InMemorySecretBackend, SecretBackend,
    SecretError, SecretHandler, StateError, StateHandler,
};
pub use socket::{
    socket_path, CapabilityError, CapabilityHandler, CapabilityRequest,
    CapabilityResponse, CapabilityServer, EchoHandler, ServerHandle, SocketError,
};
pub use user_grants::{
    glob_match as user_grant_glob_match, new_grant_id, UserGrant, UserGrants,
    UserGrantsError, SCHEMA_VERSION,
};
pub use verb::{
    normalize_grant, parse_grant, scope_matches, Verb, VerbError, KNOWN_VERBS,
    SCOPE_REQUIRED_VERBS,
};

use std::path::Path;
use std::sync::Arc;

/// Build a ready-to-serve `CompositeHandler` + `GrantTable` for a
/// plugin manifest. This is the factory that turns a plugin.toml into
/// a live capability sandbox — call it when spawning a plugin, then
/// pass the handler + grants to `CapabilityServer::new()`.
pub fn build_plugin_handler(
    manifest: &crate::plugin::manifest::Manifest,
    home: &Path,
    store: Arc<crate::superbrain::SuperbrainStore>,
    llm: Arc<crate::llm::LlmClient>,
    emb: Arc<crate::embeddings::EmbeddingClient>,
) -> Result<(Arc<dyn CapabilityHandler>, Arc<GrantTable>), grants::ResolveError> {
    let grant_table = Arc::new(resolve_grants(manifest, home)?);

    let plugin_name = &manifest.plugin.name;
    let state_dir = home.join("state").join(plugin_name);
    std::fs::create_dir_all(&state_dir).ok();

    // Secret backend reads from env — plugins get whatever the kernel
    // process has. Scope enforcement happens at the grant layer, not
    // the backend.
    let secret_backend: Arc<dyn SecretBackend> =
        Arc::new(service::secrets::EnvSecretBackend);

    let composite: Arc<dyn CapabilityHandler> = Arc::new(
        CompositeHandler::new()
            .register("state", Arc::new(StateHandler::new(state_dir)))
            .register("secrets", Arc::new(SecretHandler::new(secret_backend)))
            .register(
                "brain",
                Arc::new(service::brain::BrainHandler::new(
                    store,
                    home.join("data/Brain"),
                )),
            )
            .register(
                "llm",
                Arc::new(service::llm::LlmHandler::new(llm, emb)),
            ),
    );

    Ok((composite, grant_table))
}
