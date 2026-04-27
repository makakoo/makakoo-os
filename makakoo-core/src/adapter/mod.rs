//! Adapter subsystem — universal bridge for external AI agents.
//!
//! Schema source of truth: `spec/ADAPTER_MANIFEST.md` v1. One declarative
//! manifest, three consumers: lope validation, Harvey delegation, SANCHO
//! swarm dispatch. Adding a new external agent = dropping a `.toml` in
//! `~/.makakoo/adapters/registered/`; no per-provider Rust or Python code.
//!
//! Phase A ships: manifest parser, deterministic canonical hash, and a
//! read-only registry over the registered-adapters dir. Phases B–F layer
//! transport dispatch, install lifecycle, CLI surface, consumer wiring,
//! and the adapters-core pack on top.

pub mod call;
pub mod install;
pub mod manifest;
pub mod output;
pub mod peer;
pub mod registry;
pub mod result;
pub mod sandbox;
pub mod sign;
pub mod transport;
pub mod trust;

pub use call::{call_adapter, call_adapter_with_default_timeout, AdapterCallError};
pub use install::{
    install_from_git, install_from_path, install_from_tarball_bytes, install_from_tarball_url,
    uninstall, DiffSummary, InstallError, InstallOptions, InstallReport, InstallRoot,
};
pub use manifest::{
    AdapterKind, AdapterRole, AdapterTable, AuthScheme, AuthTable, CapabilitiesTable,
    CompatibilityTable, HealthTable, InstallTable, Manifest, ManifestError, OutputFormat,
    OutputTable, Protocol, SandboxProfile, SecurityTable, SourceType, TransportKind,
    TransportTable,
};
pub use output::{parse_response, OutputError};
pub use peer::{
    default_peers_dir, default_signing_key_path, default_signing_pub_path, default_trust_file,
    fingerprint, load_or_create_signing_key, load_trust_file, now_millis, sign_request,
    trust_add, trust_remove, verify_request, PeerError, DRIFT_WINDOW_MS, PEER_HEADER, SIG_HEADER,
    SIG_PREFIX, TS_HEADER,
};
pub use registry::{AdapterRegistry, RegisteredAdapter, RegistryError};
pub use result::{PhaseVerdict, ValidatorResult, VerdictStatus};
pub use sandbox::{
    assert_manifest_self_consistent, generate_sbpl, ProfileSpec, SandboxBackend, SandboxError,
};
pub use sign::{
    default_trust_root, keys_dir, load_publisher_key, verify_manifest_bytes, SignError,
};
pub use transport::{
    call_transport, expand_env, CallContext, HttpTransport, McpHttpSignedTransport,
    McpHttpTransport, McpStdioTransport, ResponseMeta, SubprocessTransport, Transport,
    TransportError, TransportResponse,
};
pub use trust::{
    diff_manifest, trust_entry_from_manifest, CapSnapshot, ManifestDiff, SecSnapshot, TrustEntry,
    TrustError, TrustLedger,
};
