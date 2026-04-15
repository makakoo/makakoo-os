//! Makakoo OS platform adapter — the single abstraction between the
//! kernel and OS-specific primitives.
//!
//! Every piece of code in Makakoo that would otherwise reach for `#[cfg]`
//! against `target_os` goes through the [`PlatformAdapter`] trait instead.
//! The concrete implementation is selected at compile time via
//! [`CurrentPlatform`].
//!
//! **Why a trait, not just cfg attributes?** Because we need to
//! - mock platform behavior in tests without recompiling per-target
//! - support Redox in the same codebase as macOS/Linux/Windows without
//!   turning every caller into a cfg ladder
//! - have one place to add a new operation (e.g. `symlink_dir`,
//!   `dev_mode_enabled`) and have every target implement or stub it
//!
//! The trait is deliberately small. Each method covers one platform-
//! sensitive operation: daemon install/uninstall/status, symlink
//! creation, Developer-Mode detection on Windows. Future operations
//! join here.
//!
//! The four shipping impls live in submodules:
//!   - [`macos::MacOsPlatform`]     (real — launchd)
//!   - [`linux::LinuxPlatform`]     (real — systemd user)
//!   - [`windows::WindowsPlatform`] (real — auto-launch + Dev Mode)
//!   - [`redox::RedoxPlatform`]     (stub — compiles, returns NotSupported)
//!
//! Plus an `unsupported` impl for target_os values we haven't explicitly
//! addressed (compiles everywhere, fails at runtime).

use std::path::{Path, PathBuf};

use anyhow::Result;

pub mod error;
pub mod paths;

pub use error::PlatformError;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "redox")]
pub mod redox;

// Bring the current platform's concrete type into scope under a stable name.
#[cfg(target_os = "macos")]
pub type CurrentPlatform = macos::MacOsPlatform;
#[cfg(target_os = "linux")]
pub type CurrentPlatform = linux::LinuxPlatform;
#[cfg(target_os = "windows")]
pub type CurrentPlatform = windows::WindowsPlatform;
#[cfg(target_os = "redox")]
pub type CurrentPlatform = redox::RedoxPlatform;
#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows",
    target_os = "redox"
)))]
pub type CurrentPlatform = unsupported::UnsupportedPlatform;

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows",
    target_os = "redox"
)))]
pub mod unsupported;

/// The platform adapter trait.
///
/// Every operation that has meaningfully different behavior across OSes
/// lives here. Callers never `#[cfg]` — they construct a [`CurrentPlatform`]
/// and call through the trait.
///
/// ## Error semantics
///
/// Methods return `anyhow::Result` for convenience; platform-specific
/// error variants are in [`PlatformError`]. A `NotSupported` error on a
/// stub impl (e.g. Redox) is a normal runtime outcome, not a bug.
pub trait PlatformAdapter: Send + Sync {
    /// Human-readable platform id: `"macos"`, `"linux"`, `"windows"`,
    /// `"redox"`, or `"unsupported"`.
    fn name(&self) -> &'static str;

    /// Default `$MAKAKOO_HOME` path when no env override is set. This
    /// is what fresh installs hit. Must be idempotent and side-effect
    /// free — no dir creation here.
    fn default_home(&self) -> PathBuf;

    /// Install the Makakoo daemon as an auto-starting background service
    /// for the current user. Returns the path to the service-descriptor
    /// file (plist on macOS, systemd unit on Linux, registry entry path
    /// on Windows).
    fn daemon_install(&self) -> Result<PathBuf>;

    /// Uninstall the daemon service. Idempotent — no error if already
    /// uninstalled.
    fn daemon_uninstall(&self) -> Result<()>;

    /// Is the service descriptor present on disk / in the registry?
    fn daemon_is_installed(&self) -> bool;

    /// Is the daemon currently running?
    fn daemon_is_running(&self) -> bool;

    /// Create a directory symlink from `link` to `target`. On POSIX this
    /// is `std::os::unix::fs::symlink`; on Windows it's
    /// `std::os::windows::fs::symlink_dir` which requires either
    /// Developer Mode or admin privileges (see [`Self::can_symlink`]).
    ///
    /// Returns [`PlatformError::SymlinkNotAllowed`] on Windows without
    /// Developer Mode, with a clear message pointing at
    /// Settings → For Developers.
    fn symlink_dir(&self, target: &Path, link: &Path) -> Result<()>;

    /// Can the current process create symlinks for the current user?
    /// Always `true` on POSIX, checks Developer Mode on Windows, always
    /// `false` on Redox stub.
    fn can_symlink(&self) -> bool;
}

// --- Tests that apply to every platform ---------------------------------

#[cfg(test)]
mod common_tests {
    use super::*;

    #[test]
    fn current_platform_name_is_nonempty() {
        let p = CurrentPlatform::default();
        let n = p.name();
        assert!(!n.is_empty());
        assert!(
            matches!(n, "macos" | "linux" | "windows" | "redox" | "unsupported"),
            "unexpected platform name: {n}"
        );
    }

    #[test]
    fn default_home_is_absolute_or_current() {
        let p = CurrentPlatform::default();
        let h = p.default_home();
        // We don't assert it exists — fresh installs haven't made it yet.
        // We just assert it's not empty.
        assert!(!h.as_os_str().is_empty());
    }
}
