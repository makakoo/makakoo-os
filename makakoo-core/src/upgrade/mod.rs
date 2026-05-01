//! Self-upgrade machinery for the `makakoo` + `makakoo-mcp` binaries.
//!
//! SPRINT-MAKAKOO-UPGRADE-VERB. Detects the install method by inspecting
//! the running binary's path, dispatches to the matching update command,
//! and helps the caller capture before/after version for verification.
//!
//! v1 deliberately does NOT auto-restart the daemon — the existing
//! `makakoo daemon` surface only has install/uninstall/status/logs/run,
//! no restart. The CLI verb prints a platform-specific restart hint
//! instead. Adding `daemon restart` is queued as a follow-up sprint.

pub mod detect;
pub mod dispatch;
pub mod verify;

pub use detect::{detect_install_method, CargoSource, InstallMethod};
pub use dispatch::{
    plan_upgrade, run_upgrade, BinaryTarget, UpgradeAction, UpgradeError,
};
pub use verify::{capture_version, daemon_restart_hint};
