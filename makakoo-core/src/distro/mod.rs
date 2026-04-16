//! Distro subsystem — opinionated plugin bundles.
//!
//! A distro is a single `distros/<name>.toml` file that declares a kernel
//! version constraint, a plugin ship list, defaults, and optional includes
//! of other distros. Installing a distro batch-installs every plugin in
//! its effective list.
//!
//! Spec source of truth: `spec/DISTRO.md` v0.1.
//!
//! Phase D scope: parse distro files, resolve include chains (with cycle
//! detection), produce an effective plugin list that the CLI layer can
//! hand to the plugin installer. `distro save`, `distro update`, and
//! `distro switch` are deferred to later phases.

pub mod file;
pub mod resolver;

pub use file::{
    DefaultsTable, DistroError, DistroFile, DistroTable, ExcludesTable, KernelTable,
    PluginPin, PostInstallTable,
};
pub use resolver::{resolve_distro, DistroResolverError, EffectivePlugin, ResolvedDistro};
