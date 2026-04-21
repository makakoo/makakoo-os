//! Plugin subsystem — manifest loader, registry walker, dependency resolver,
//! atomic staging installer.
//!
//! Phase C of the Makakoo OS Rust rewrite. Every unit of Makakoo capability
//! (skills, agents, SANCHO tasks, MCP tools, mascots, bootstrap fragments) is
//! a plugin described by a `plugin.toml` at the root of its directory. The
//! kernel discovers plugins by walking `$MAKAKOO_HOME/plugins/*/plugin.toml`
//! at daemon start, parses each manifest, resolves dependencies, and hands
//! the topologically-sorted list to the relevant subsystems (SANCHO,
//! MCP gateway, infect renderer).
//!
//! Schema source of truth: `spec/PLUGIN_MANIFEST.md` v0.1. Every validation
//! rule in §17 of that spec is enforced here.

pub mod install;
pub mod lock;
pub mod manifest;
pub mod registry;
pub mod resolver;
pub mod staging;

pub use install::{
    install, install_from_git, install_from_path, install_from_tarball_url, uninstall,
    InstallError, InstallRequest, PluginSource,
};
pub use lock::{lock_path, LockEntry, LockError, LockMeta, PluginsLock};
pub use manifest::{
    AbiTable, CapabilitiesTable, DependsTable, EntrypointTable, InfectTable, InstallTable,
    Manifest, ManifestError, McpTable, McpToolSpec, PluginKind, PluginLanguage, PluginTable,
    SanchoTable, SanchoTaskSpec, SourceTable, StateRetention, StateTable,
};
pub use registry::{LoadedPlugin, PluginRegistry, RegistryError};
pub use resolver::{resolve_load_order, ResolverError, KERNEL_ABI_SUPPORT};
pub use staging::{stage_and_install, StagingError};
