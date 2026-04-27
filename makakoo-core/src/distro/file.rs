//! Distro file parser.
//!
//! Shape per `spec/DISTRO.md §1`:
//!
//! ```toml
//! [distro]
//! name = "core"
//! display_name = "Makakoo Core"
//! version = "0.1.0"
//! description = "..."
//! authors = ["..."]
//! license = "MIT"
//!
//! include = ["minimal.toml"]
//!
//! [kernel]
//! version = "^0.1"
//!
//! [plugins]
//! "plugin-a" = { version = "*" }
//! "plugin-b" = { version = "^0.3", blake3 = "abcd..." }
//!
//! [defaults]
//! voice = "caveman"
//!
//! [excludes]
//! plugins = ["skill-creative-prose"]
//!
//! [post_install]
//! message = "..."
//! ```
//!
//! The parser enforces structural rules at parse time: known field names,
//! valid semver constraints, plugin name regex, no empty include entries.
//! Include resolution itself happens in `resolver.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DistroError {
    #[error("failed to read distro file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse distro file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("distro file {path} is missing required [distro] table")]
    MissingDistroTable { path: PathBuf },
    #[error(
        "distro {distro:?} declares kernel version constraint {req:?} which this kernel cannot satisfy (kernel = {kernel})"
    )]
    KernelMismatch {
        distro: String,
        req: VersionReq,
        kernel: String,
    },
    #[error("distro {distro:?} has invalid name — must match {regex}")]
    InvalidName { distro: String, regex: &'static str },
    #[error("distro {distro:?} has invalid plugin pin for {plugin:?}: {msg}")]
    InvalidPluginPin {
        distro: String,
        plugin: String,
        msg: String,
    },
}

/// A distro file is the on-disk representation at `distros/<name>.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DistroFile {
    pub distro: DistroTable,

    #[serde(default)]
    pub kernel: KernelTable,

    /// Map of plugin name → pin. Parsed from a TOML table where the keys
    /// are plugin names (strings) and values are `PluginPin` structs or
    /// just a version string.
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginPin>,

    #[serde(default)]
    pub defaults: DefaultsTable,

    #[serde(default)]
    pub excludes: ExcludesTable,

    #[serde(default)]
    pub post_install: PostInstallTable,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DistroTable {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    /// Distro files to layer under this one. Paths are resolved relative
    /// to the directory containing the current file. See `spec/DISTRO.md
    /// §2` for merge semantics.
    #[serde(default)]
    pub include: Vec<String>,
}

impl DistroFile {
    /// Shorthand for `self.distro.include` — include lives inside the
    /// `[distro]` table per the spec example.
    pub fn include(&self) -> &[String] {
        &self.distro.include
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KernelTable {
    /// Semver constraint on the kernel. Default `*` (anything goes).
    #[serde(default)]
    pub version: Option<String>,
}

/// Plugin pin — version constraint + optional blake3. Accepts two forms
/// in the TOML source:
///
/// ```toml
/// "plugin-a" = "^0.3"
/// "plugin-b" = { version = "^0.3", blake3 = "..." }
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PluginPin {
    Simple(String),
    Full(PluginPinFull),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPinFull {
    pub version: String,
    #[serde(default)]
    pub blake3: Option<String>,
}

impl PluginPin {
    pub fn version(&self) -> &str {
        match self {
            PluginPin::Simple(v) => v,
            PluginPin::Full(f) => &f.version,
        }
    }

    pub fn blake3(&self) -> Option<&str> {
        match self {
            PluginPin::Simple(_) => None,
            PluginPin::Full(f) => f.blake3.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DefaultsTable {
    #[serde(flatten)]
    pub values: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludesTable {
    #[serde(default)]
    pub plugins: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PostInstallTable {
    #[serde(default)]
    pub message: Option<String>,
}

static NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z][a-z0-9-]{1,62}$").unwrap());

const NAME_REGEX_STR: &str = "^[a-z][a-z0-9-]{1,62}$";

impl DistroFile {
    /// Load a distro file from disk. Applies syntax-level checks on fields
    /// but does not yet resolve includes.
    pub fn load(path: &Path) -> Result<Self, DistroError> {
        let raw = std::fs::read_to_string(path).map_err(|source| DistroError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&raw, path)
    }

    /// Parse from a string. `path` is used only for error messages.
    pub fn parse(raw: &str, path: &Path) -> Result<Self, DistroError> {
        let file: DistroFile = toml::from_str(raw).map_err(|source| DistroError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        file.validate()?;
        Ok(file)
    }

    /// Structural validation. Called after deserialize to tighten the
    /// soft TOML shape into Makakoo's contract.
    pub fn validate(&self) -> Result<(), DistroError> {
        // Name must match the same regex we use for plugin names.
        if !NAME_RE.is_match(&self.distro.name) {
            return Err(DistroError::InvalidName {
                distro: self.distro.name.clone(),
                regex: NAME_REGEX_STR,
            });
        }

        // Kernel version must parse as a semver VersionReq if present.
        if let Some(ref v) = self.kernel.version {
            v.parse::<VersionReq>()
                .map_err(|e| DistroError::InvalidPluginPin {
                    distro: self.distro.name.clone(),
                    plugin: "(kernel)".into(),
                    msg: format!("kernel version constraint: {e}"),
                })?;
        }

        // Every plugin pin must have a parsable version. Blake3 (when
        // present) must be 64 hex chars.
        for (name, pin) in &self.plugins {
            if !NAME_RE.is_match(name) {
                return Err(DistroError::InvalidPluginPin {
                    distro: self.distro.name.clone(),
                    plugin: name.clone(),
                    msg: format!("plugin name must match {NAME_REGEX_STR}"),
                });
            }
            pin.version()
                .parse::<VersionReq>()
                .map_err(|e| DistroError::InvalidPluginPin {
                    distro: self.distro.name.clone(),
                    plugin: name.clone(),
                    msg: format!("version: {e}"),
                })?;
            if let Some(h) = pin.blake3() {
                if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(DistroError::InvalidPluginPin {
                        distro: self.distro.name.clone(),
                        plugin: name.clone(),
                        msg: "blake3 must be 64 hex chars".into(),
                    });
                }
            }
        }

        // Excludes — names must also match the regex.
        for ex in &self.excludes.plugins {
            if !NAME_RE.is_match(ex) {
                return Err(DistroError::InvalidPluginPin {
                    distro: self.distro.name.clone(),
                    plugin: ex.clone(),
                    msg: "exclude name must match the plugin name regex".into(),
                });
            }
        }

        Ok(())
    }

    /// Check the kernel version against the distro's constraint. Returns
    /// `KernelMismatch` if incompatible, `Ok(())` if the constraint is
    /// absent or satisfied.
    pub fn check_kernel(&self, kernel_version: &semver::Version) -> Result<(), DistroError> {
        let Some(ref raw) = self.kernel.version else {
            return Ok(());
        };
        let req: VersionReq = raw.parse().expect("validated earlier");
        if !req.matches(kernel_version) {
            return Err(DistroError::KernelMismatch {
                distro: self.distro.name.clone(),
                req,
                kernel: kernel_version.to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_path() -> PathBuf {
        PathBuf::from("test.toml")
    }

    #[test]
    fn parses_minimal_distro() {
        let src = r#"
[distro]
name = "minimal"
version = "0.1.0"

[kernel]
version = "^0.1"

[plugins]
"#;
        let f = DistroFile::parse(src, &test_path()).unwrap();
        assert_eq!(f.distro.name, "minimal");
        assert!(f.plugins.is_empty());
        assert_eq!(f.kernel.version.as_deref(), Some("^0.1"));
    }

    #[test]
    fn parses_full_distro() {
        let src = r#"
[distro]
name = "trader"
display_name = "Trader"
version = "0.1.0"
description = "..."
authors = ["Sebastian"]
license = "MIT"

include = ["core.toml"]

[kernel]
version = "^0.1"

[plugins]
"agent-arbitrage" = { version = "^0.3", blake3 = "0000000000000000000000000000000000000000000000000000000000000000" }
"skill-market" = "*"

[defaults]
voice = "caveman"

[excludes]
plugins = ["skill-prose"]

[post_install]
message = "hi"
"#;
        let f = DistroFile::parse(src, &test_path()).unwrap();
        assert_eq!(f.include(), &["core.toml".to_string()]);
        assert_eq!(f.plugins.len(), 2);
        assert_eq!(f.plugins["skill-market"].version(), "*");
        assert_eq!(
            f.plugins["agent-arbitrage"].blake3().unwrap().len(),
            64
        );
        assert_eq!(f.excludes.plugins, vec!["skill-prose"]);
        assert_eq!(f.post_install.message.as_deref(), Some("hi"));
    }

    #[test]
    fn rejects_bad_blake3() {
        let src = r#"
[distro]
name = "trader"
[plugins]
"x" = { version = "*", blake3 = "nothex" }
"#;
        let err = DistroFile::parse(src, &test_path()).unwrap_err();
        assert!(matches!(err, DistroError::InvalidPluginPin { .. }));
    }

    #[test]
    fn rejects_bad_version_constraint() {
        let src = r#"
[distro]
name = "xx"
[plugins]
"pp" = "not a version"
"#;
        let err = DistroFile::parse(src, &test_path()).unwrap_err();
        assert!(matches!(err, DistroError::InvalidPluginPin { .. }));
    }

    #[test]
    fn rejects_bad_name() {
        let src = r#"
[distro]
name = "Bad_Name"
"#;
        let err = DistroFile::parse(src, &test_path()).unwrap_err();
        assert!(matches!(err, DistroError::InvalidName { .. }));
    }

    #[test]
    fn check_kernel_enforces_constraint() {
        let src = r#"
[distro]
name = "xx"
[kernel]
version = "^1.0"
"#;
        let f = DistroFile::parse(src, &test_path()).unwrap();
        f.check_kernel(&semver::Version::new(1, 2, 3)).unwrap();
        let err = f
            .check_kernel(&semver::Version::new(0, 9, 0))
            .unwrap_err();
        assert!(matches!(err, DistroError::KernelMismatch { .. }));
    }

    #[test]
    fn validates_the_shipped_core_toml() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .parent()
            .unwrap()
            .join("distros/core.toml");
        if !path.exists() {
            return;
        }
        let f = DistroFile::load(&path).unwrap();
        assert_eq!(f.distro.name, "core");
        assert_eq!(f.include(), &["minimal.toml".to_string()]);
        assert!(f.plugins.len() >= 4);
    }

    #[test]
    fn validates_the_shipped_minimal_toml() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .parent()
            .unwrap()
            .join("distros/minimal.toml");
        if !path.exists() {
            return;
        }
        let f = DistroFile::load(&path).unwrap();
        assert_eq!(f.distro.name, "minimal");
        assert!(f.plugins.is_empty());
    }
}
