//! Fallback adapter for target_os values we haven't explicitly addressed.
//! Compiles everywhere, refuses everything at runtime.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{PlatformAdapter, PlatformError};

#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPlatform;

impl PlatformAdapter for UnsupportedPlatform {
    fn name(&self) -> &'static str {
        "unsupported"
    }

    fn default_home(&self) -> PathBuf {
        PathBuf::from(".makakoo")
    }

    fn daemon_install(&self) -> Result<PathBuf> {
        Err(PlatformError::NotSupported {
            operation: "daemon_install",
            platform: "unsupported",
        }
        .into())
    }

    fn daemon_uninstall(&self) -> Result<()> {
        Err(PlatformError::NotSupported {
            operation: "daemon_uninstall",
            platform: "unsupported",
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
        Err(PlatformError::NotSupported {
            operation: "symlink_dir",
            platform: "unsupported",
        }
        .into())
    }

    fn can_symlink(&self) -> bool {
        false
    }
}
