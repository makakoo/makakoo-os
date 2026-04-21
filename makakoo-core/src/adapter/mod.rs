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

pub mod manifest;
pub mod registry;

pub use manifest::{
    AdapterKind, AdapterRole, AdapterTable, AuthScheme, AuthTable, CapabilitiesTable,
    CompatibilityTable, HealthTable, InstallTable, Manifest, ManifestError, OutputFormat,
    OutputTable, Protocol, SandboxProfile, SecurityTable, SourceType, TransportKind,
    TransportTable,
};
pub use registry::{AdapterRegistry, RegisteredAdapter, RegistryError};
