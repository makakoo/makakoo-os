//! Kernel feature-flag config.
//!
//! v0.2 Phase G.5. Reads `$MAKAKOO_HOME/config/kernel.toml`. Every
//! flag is OFF by default — callers that want to opt into a new
//! subsystem (session trees, in-process plugin ABI, …) flip the
//! corresponding key to `true`.
//!
//! Format (v1):
//!
//! ```toml
//! [kernel]
//! session_tree = true        # enable JSONL session trees (G.1–G.5)
//! rust_dylib_abi = false     # future: in-process Rust plugins
//! ```
//!
//! Missing file → all flags default-false. Malformed file → warn,
//! return all-false (same semantics as persona.json to keep boot from
//! crashing on bad config).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform::makakoo_home;

/// Canonical on-disk location of `kernel.toml` for a given home dir.
pub fn kernel_config_path_for(home: &Path) -> PathBuf {
    home.join("config").join("kernel.toml")
}

pub fn kernel_config_path() -> PathBuf {
    kernel_config_path_for(&makakoo_home())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelFeatures {
    /// v0.2 Phase G: JSONL session trees. Gates `session` CLI subcommand
    /// and any future agent event-loop integration that writes into
    /// `data/sessions/`.
    #[serde(default)]
    pub session_tree: bool,

    /// Future (Phase A.5 follow-up): load plugins via stable Rust dylib
    /// ABI instead of out-of-process subprocess. Stays off until the
    /// ABI spec ships + at least one reference plugin uses it.
    #[serde(default)]
    pub rust_dylib_abi: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KernelConfig {
    #[serde(default)]
    pub kernel: KernelFeatures,
}

impl KernelConfig {
    /// Load + parse `$MAKAKOO_HOME/config/kernel.toml`. Missing file
    /// or malformed TOML both resolve to defaults (every flag = false).
    pub fn load() -> Self {
        Self::load_from(&kernel_config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        let raw = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "kernel.toml at {} unreadable: {} — using defaults",
                    path.display(),
                    e,
                );
                return Self::default();
            }
        };
        match toml::from_str::<Self>(&raw) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "kernel.toml at {} malformed: {} — using defaults",
                    path.display(),
                    e,
                );
                Self::default()
            }
        }
    }

    pub fn session_tree_enabled(&self) -> bool {
        self.kernel.session_tree
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_are_all_false() {
        let c = KernelConfig::default();
        assert!(!c.session_tree_enabled());
        assert!(!c.kernel.rust_dylib_abi);
    }

    #[test]
    fn load_from_missing_file_returns_default() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("kernel.toml");
        let c = KernelConfig::load_from(&p);
        assert!(!c.session_tree_enabled());
    }

    #[test]
    fn load_from_parses_toml() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("kernel.toml");
        fs::write(&p, "[kernel]\nsession_tree = true\n").unwrap();
        let c = KernelConfig::load_from(&p);
        assert!(c.session_tree_enabled());
    }

    #[test]
    fn malformed_toml_falls_back_to_default() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("kernel.toml");
        fs::write(&p, "this is not toml {{{").unwrap();
        let c = KernelConfig::load_from(&p);
        assert!(!c.session_tree_enabled());
    }

    #[test]
    fn unknown_keys_tolerated() {
        // Unknown fields should be silently ignored — don't panic when
        // a future config key appears in a user's file after downgrade.
        let dir = tempdir().unwrap();
        let p = dir.path().join("kernel.toml");
        fs::write(
            &p,
            "[kernel]\nsession_tree = true\nsome_future_flag = \"yes\"\n",
        )
        .unwrap();
        let c = KernelConfig::load_from(&p);
        assert!(c.session_tree_enabled());
    }
}
