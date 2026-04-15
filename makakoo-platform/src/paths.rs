//! Cross-OS path helpers used by the platform adapters.
//!
//! Duplicates the minimal path logic from `makakoo_core::platform`
//! deliberately — `makakoo-platform` must stay free of heavy deps
//! (reqwest, teloxide, etc.) so it cross-compiles cleanly to any
//! target, including Redox, without needing system libraries.
//!
//! If the spec definition of these paths ever moves, update both
//! `makakoo_core::platform` and this module in lockstep.

use std::path::PathBuf;

/// Resolve the Makakoo home directory.
///
/// Precedence: `$MAKAKOO_HOME` → `$HARVEY_HOME` (legacy alias) →
/// OS-native default per D10 in the architecture spec.
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
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".makakoo");
    }
}

/// Canonical data root — always `{makakoo_home}/data`.
pub fn data_dir() -> PathBuf {
    makakoo_home().join("data")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_resolves_to_something() {
        let h = makakoo_home();
        assert!(!h.as_os_str().is_empty());
    }

    #[test]
    fn data_dir_is_under_home() {
        let d = data_dir();
        assert!(d.ends_with("data"));
        assert!(d.starts_with(makakoo_home()));
    }

    #[test]
    fn env_override_wins() {
        let sentinel = std::env::temp_dir().join("makakoo_test_platform_paths");
        std::env::set_var("MAKAKOO_HOME", &sentinel);
        assert_eq!(makakoo_home(), sentinel);
        std::env::remove_var("MAKAKOO_HOME");
    }
}
