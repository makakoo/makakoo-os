//! Platform-specific error variants.
//!
//! These are lifted out of `anyhow::Error` when a caller needs to
//! distinguish "this platform doesn't support X" from "the operation
//! failed." The most common case is Windows without Developer Mode —
//! the install flow wants to print a friendly pointer at Settings
//! rather than a generic "permission denied."

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlatformError {
    /// The current platform does not support the requested operation
    /// at all (e.g. Redox stub).
    #[error("{operation} is not supported on {platform}")]
    NotSupported {
        operation: &'static str,
        platform: &'static str,
    },

    /// Symlink creation refused because the current Windows session
    /// does not have Developer Mode enabled. Message includes the
    /// exact Settings path the user should visit.
    #[error(
        "symlinks require Windows Developer Mode. \
         Enable it at Settings → For Developers → Developer Mode, \
         then re-run this command."
    )]
    SymlinkNotAllowed,

    /// A platform command (launchctl, systemctl, etc.) exited non-zero.
    #[error("{command} failed with exit code {code}: {stderr}")]
    CommandFailed {
        command: &'static str,
        code: i32,
        stderr: String,
    },
}
