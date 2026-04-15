//! Platform abstraction — resolves cross-platform paths and provides file
//! locking primitives.
//!
//! The rename from Harvey OS → Makakoo OS keeps `$HARVEY_HOME` as a legacy
//! alias for `$MAKAKOO_HOME`. Both resolve here; `$MAKAKOO_HOME` wins if set.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::error::{MakakooError, Result};

/// Resolve the Makakoo home directory.
///
/// Precedence: `$MAKAKOO_HOME` → `$HARVEY_HOME` (legacy alias kept for
/// backwards compatibility) → OS-native default (per D10 in
/// ARCHITECTURE.md): `~/.makakoo` on macOS, `~/.local/share/makakoo`
/// on Linux XDG, `%LOCALAPPDATA%\Makakoo` on Windows.
///
/// Sebastian's pre-v0.1 install keeps `~/MAKAKOO` working through
/// `$MAKAKOO_HOME` env var + a compat symlink added in Phase H; the
/// fallback never points at `~/HARVEY` or `~/MAKAKOO` for fresh installs.
pub fn makakoo_home() -> PathBuf {
    if let Ok(p) = std::env::var("MAKAKOO_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(p) = std::env::var("HARVEY_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    // OS-native default. Matches D10 in the architecture spec.
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".makakoo");
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(x) = std::env::var("XDG_DATA_HOME") {
            if !x.is_empty() {
                return PathBuf::from(x).join("makakoo");
            }
        }
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/share/makakoo");
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = dirs::data_local_dir() {
            return local.join("Makakoo");
        }
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Makakoo");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        // Redox and other targets — fall back to ~/.makakoo, which also
        // matches the Redox adapter stub.
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".makakoo");
    }
}

/// OS-native config dir: `~/Library/Application Support/makakoo` on macOS,
/// `$XDG_CONFIG_HOME/makakoo` on Linux, `%APPDATA%/makakoo` on Windows.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("makakoo")
}

/// Canonical data root — always `{makakoo_home}/data`.
pub fn data_dir() -> PathBuf {
    makakoo_home().join("data")
}

/// Scratch dir under the OS temp root. Never assume `/tmp/`.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("makakoo")
}

/// Create (if missing) and exclusively lock a file. Drop the returned
/// handle to release the lock. Returns `MakakooError::Io` if the lock is
/// already held by another process.
///
/// The lock file's parent directory is created if necessary.
pub fn lock_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    #[allow(unstable_name_collisions)]
    FileExt::try_lock_exclusive(&file).map_err(|e| {
        MakakooError::Io(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!("could not lock {}: {e}", path.display()),
        ))
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_resolves_to_something() {
        let home = makakoo_home();
        assert!(!home.as_os_str().is_empty());
    }

    #[test]
    fn config_dir_resolves() {
        let cfg = config_dir();
        assert!(cfg.ends_with("makakoo"));
    }

    #[test]
    fn data_dir_under_home() {
        let data = data_dir();
        assert!(data.ends_with("data"));
        assert!(data.starts_with(makakoo_home()));
    }

    #[test]
    fn temp_dir_has_makakoo_suffix() {
        let tmp = temp_dir();
        assert!(tmp.ends_with("makakoo"));
    }

    #[test]
    fn env_override_wins() {
        // Use a unique var value so we don't race other tests.
        let sentinel = std::env::temp_dir().join("makakoo_test_home_sentinel");
        std::env::set_var("MAKAKOO_HOME", &sentinel);
        assert_eq!(makakoo_home(), sentinel);
        std::env::remove_var("MAKAKOO_HOME");
    }

    #[test]
    fn lock_file_acquires_and_releases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock.test");
        let f1 = lock_file(&path).unwrap();
        // Second lock on same path should fail while f1 is alive.
        assert!(lock_file(&path).is_err());
        drop(f1);
        // After drop, re-acquire should succeed.
        let _f2 = lock_file(&path).unwrap();
    }
}
