//! Redox platform adapter — compile-only stub.
//!
//! This exists so `cargo check --target x86_64-unknown-redox -p
//! makakoo-platform` passes from day one (D1 non-negotiable). The real
//! Redox daemon install will land in Phase H after:
//!
//! - `rusqlite` bundled C build is verified against Redox relibc (or
//!   we swap to `limbo`/`libsql` pure-Rust per ARCHITECTURE.md §9.5)
//! - The Redox init system integration is spec'd (scheme + init.d entry)
//! - A Redox VM is available for smoke-testing
//!
//! Until then, every method returns `PlatformError::NotSupported`. The
//! trait impl is present and compiles; the runtime behavior is
//! explicit about what's missing.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{PlatformAdapter, PlatformError};

#[derive(Debug, Default, Clone, Copy)]
pub struct RedoxPlatform;

impl PlatformAdapter for RedoxPlatform {
    fn name(&self) -> &'static str {
        "redox"
    }

    fn default_home(&self) -> PathBuf {
        // Redox file scheme is `/home/<user>/.makakoo`. Redox `dirs`
        // crate support is spotty; fall back to env + hardcoded path
        // since this is a stub anyway.
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".makakoo");
        }
        PathBuf::from("/home/user/.makakoo")
    }

    fn daemon_install(&self) -> Result<PathBuf> {
        Err(PlatformError::NotSupported {
            operation: "daemon_install",
            platform: "redox",
        }
        .into())
    }

    fn daemon_uninstall(&self) -> Result<()> {
        Err(PlatformError::NotSupported {
            operation: "daemon_uninstall",
            platform: "redox",
        }
        .into())
    }

    fn daemon_is_installed(&self) -> bool {
        false
    }

    fn daemon_is_running(&self) -> bool {
        false
    }

    fn symlink_dir(&self, _target: &Path, _link: &Path) -> Result<()> {
        // Redox's VFS supports symlinks via schemes; we just haven't wired
        // the impl yet. This is the Phase H entry point.
        Err(PlatformError::NotSupported {
            operation: "symlink_dir",
            platform: "redox",
        }
        .into())
    }

    fn can_symlink(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_redox() {
        assert_eq!(RedoxPlatform.name(), "redox");
    }

    #[test]
    fn daemon_ops_return_not_supported() {
        let p = RedoxPlatform;
        assert!(p.daemon_install().is_err());
        assert!(p.daemon_uninstall().is_err());
        assert!(!p.daemon_is_installed());
        assert!(!p.daemon_is_running());
    }

    #[test]
    fn can_symlink_is_false_until_phase_h() {
        assert!(!RedoxPlatform.can_symlink());
    }
}
