//! High-level plugin install/uninstall wrapping `staging.rs`.
//!
//! `stage_and_install` operates on an already-staged directory; this
//! module adds the "copy source tree into the staging area" step so the
//! CLI can point at any local directory and end up with a registered
//! plugin + updated `plugins.lock`.
//!
//! v0.1 scope: local filesystem source only. Git URL / tarball sources
//! come in Phase F alongside the cross-OS installer.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;
use tracing::debug;

use super::lock::{LockEntry, LockError, PluginsLock};
use super::manifest::{Manifest, ManifestError};
use super::staging::{stage_and_install, stage_dir, StagingError};

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("source {path} is not a directory")]
    NotADir { path: PathBuf },
    #[error("source {path} is missing plugin.toml")]
    NoManifest { path: PathBuf },
    #[error("manifest error: {0}")]
    Manifest(#[from] ManifestError),
    #[error("staging error: {0}")]
    Staging(#[from] StagingError),
    #[error("lock error: {0}")]
    Lock(#[from] LockError),
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("plugin {name:?} is not installed")]
    NotInstalled { name: String },
}

/// Where a plugin's source lives. v0.1 only implements `Path`.
#[derive(Debug, Clone)]
pub enum PluginSource {
    /// An absolute or relative path to a directory containing plugin.toml.
    Path(PathBuf),
}

/// Arguments for a single install.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub source: PluginSource,
    /// Optional blake3 override. If `Some`, takes precedence over the
    /// manifest's declared hash (which takes precedence over nothing).
    pub expected_blake3: Option<String>,
}

/// Install one plugin from a local source path into `$MAKAKOO_HOME`.
///
/// Updates `plugins.lock` on success. The caller is responsible for the
/// wider transaction — if installing N plugins as a batch (`distro
/// install`), the caller decides whether to roll back already-installed
/// plugins on later failure, or leave them in place.
pub fn install_from_path(
    req: &InstallRequest,
    makakoo_home: &Path,
) -> Result<super::staging::InstallOutcome, InstallError> {
    let PluginSource::Path(src_path) = &req.source;

    // 1) Basic sanity on the source tree.
    if !src_path.is_dir() {
        return Err(InstallError::NotADir {
            path: src_path.clone(),
        });
    }
    let manifest_path = src_path.join("plugin.toml");
    if !manifest_path.exists() {
        return Err(InstallError::NoManifest {
            path: src_path.clone(),
        });
    }
    // Parse the manifest so we know the plugin name before copying —
    // the staged dir needs to be named after it.
    let (manifest, _warn) = Manifest::load(&manifest_path)?;
    let name = manifest.plugin.name.clone();

    // 2) Copy source tree into $MAKAKOO_HOME/plugins/.stage/<name>/
    let stage_target = stage_dir(makakoo_home).join(&name);
    if stage_target.exists() {
        // Leftover from a previous crashed install. Wipe it.
        fs::remove_dir_all(&stage_target).map_err(|source| InstallError::Io {
            path: stage_target.clone(),
            source,
        })?;
    }
    if let Some(parent) = stage_target.parent() {
        fs::create_dir_all(parent).map_err(|source| InstallError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    copy_dir(src_path, &stage_target)?;
    debug!(
        "staged plugin source from {} to {}",
        src_path.display(),
        stage_target.display()
    );

    // 3) Hand off to stage_and_install: verifies blake3, atomic rename.
    let outcome = stage_and_install(
        &stage_target,
        makakoo_home,
        req.expected_blake3.as_deref(),
    )?;

    // 4) Record in plugins.lock. Fresh installs start enabled;
    //    reinstalls of a previously-disabled plugin reset to enabled
    //    (a user who wanted it off must re-run `makakoo plugin disable`).
    let mut lock = PluginsLock::load(makakoo_home)?;
    lock.upsert(LockEntry {
        name: outcome.name.clone(),
        version: manifest.plugin.version.to_string(),
        blake3: Some(outcome.computed_blake3.clone()),
        source: format!("path:{}", src_path.display()),
        installed_at: Utc::now(),
        enabled: true,
    });
    lock.save(makakoo_home)?;

    Ok(outcome)
}

/// Uninstall a plugin by name.
///
/// * Stops + removes the plugin directory.
/// * If `purge` is true, also wipes the state dir declared in `[state].dir`
///   of the manifest (best-effort — missing dir is fine). State dirs with
///   `retention = "keep"` are preserved unless `purge` is set.
/// * Updates `plugins.lock` (removes the entry).
pub fn uninstall(
    name: &str,
    makakoo_home: &Path,
    purge: bool,
) -> Result<UninstallOutcome, InstallError> {
    let plugin_dir = super::staging::final_dir(makakoo_home, name);
    if !plugin_dir.exists() {
        return Err(InstallError::NotInstalled {
            name: name.to_string(),
        });
    }

    // Read manifest to discover state dir before we remove the plugin.
    let manifest_path = plugin_dir.join("plugin.toml");
    let manifest_info = if manifest_path.exists() {
        Manifest::load(&manifest_path).ok().map(|(m, _)| m)
    } else {
        None
    };

    let state_dir: Option<PathBuf> = manifest_info
        .as_ref()
        .and_then(|m| m.state.as_ref())
        .map(|s| resolve_state_dir(&s.dir, makakoo_home));

    fs::remove_dir_all(&plugin_dir).map_err(|source| InstallError::Io {
        path: plugin_dir.clone(),
        source,
    })?;
    debug!("removed plugin dir {}", plugin_dir.display());

    let state_wiped = if purge {
        if let Some(ref dir) = state_dir {
            if dir.exists() {
                let _ = fs::remove_dir_all(dir);
                true
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    // Update lock file.
    let mut lock = PluginsLock::load(makakoo_home)?;
    lock.remove(name);
    lock.save(makakoo_home)?;

    Ok(UninstallOutcome {
        name: name.to_string(),
        removed_from: plugin_dir,
        state_wiped,
    })
}

#[derive(Debug, Clone)]
pub struct UninstallOutcome {
    pub name: String,
    pub removed_from: PathBuf,
    pub state_wiped: bool,
}

/// Expand `$MAKAKOO_HOME` tokens in a state-dir string. The manifest schema
/// allows a literal `$MAKAKOO_HOME/...` placeholder — we resolve it here
/// rather than forcing the plugin to know the install path.
fn resolve_state_dir(raw: &str, makakoo_home: &Path) -> PathBuf {
    let home_str = makakoo_home.to_string_lossy();
    let expanded = raw
        .replace("$MAKAKOO_HOME", &home_str)
        .replace("${MAKAKOO_HOME}", &home_str);
    PathBuf::from(expanded)
}

/// Recursively copy `src` to `dst`. Creates `dst` if missing. Rejects
/// symlinks (v0.1: plugins must be self-contained, no symlink tricks).
fn copy_dir(src: &Path, dst: &Path) -> Result<(), InstallError> {
    fs::create_dir_all(dst).map_err(|source| InstallError::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    let entries = fs::read_dir(src).map_err(|source| InstallError::Io {
        path: src.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| InstallError::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let ty = entry.file_type().map_err(|source| InstallError::Io {
            path: entry.path(),
            source,
        })?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&from, &to)?;
        } else if ty.is_file() {
            fs::copy(&from, &to).map_err(|source| InstallError::Io {
                path: to.clone(),
                source,
            })?;
        }
        // Symlinks silently skipped — keeps the staged tree hermetic.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_manifest(dir: &Path, name: &str, extras: &str) {
        let body = format!(
            r#"
[plugin]
name = "{name}"
version = "1.0.0"
kind = "skill"
language = "python"

[source]
path = "."

[abi]
skill = "^1.0"

[entrypoint]
run = "true"
{extras}
"#
        );
        fs::write(dir.join("plugin.toml"), body).unwrap();
    }

    fn seed_source(root: &Path, name: &str) -> PathBuf {
        let src = root.join(format!("src-{name}"));
        fs::create_dir_all(&src).unwrap();
        write_manifest(&src, name, "");
        fs::write(src.join("hello.py"), b"print('hi')").unwrap();
        src
    }

    #[test]
    fn install_from_path_happy() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("makakoo");
        fs::create_dir_all(&home).unwrap();
        let src = seed_source(tmp.path(), "hello-world");

        let outcome = install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap();

        assert_eq!(outcome.name, "hello-world");
        assert!(outcome.final_dir.exists());
        assert!(outcome.final_dir.join("hello.py").exists());

        // Lock file recorded it.
        let lock = PluginsLock::load(&home).unwrap();
        assert_eq!(lock.plugins.len(), 1);
        assert_eq!(lock.plugins[0].name, "hello-world");
        assert!(lock.plugins[0].blake3.is_some());
    }

    #[test]
    fn install_rejects_missing_manifest() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let empty_src = tmp.path().join("empty");
        fs::create_dir_all(&empty_src).unwrap();
        let err = install_from_path(
            &InstallRequest {
                source: PluginSource::Path(empty_src),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap_err();
        assert!(matches!(err, InstallError::NoManifest { .. }));
    }

    #[test]
    fn install_rejects_nondir_source() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let bogus = tmp.path().join("not-a-dir");
        let err = install_from_path(
            &InstallRequest {
                source: PluginSource::Path(bogus),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap_err();
        assert!(matches!(err, InstallError::NotADir { .. }));
    }

    #[test]
    fn uninstall_removes_plugin_and_lock_entry() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let src = seed_source(tmp.path(), "goodbye");
        install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap();

        let outcome = uninstall("goodbye", &home, false).unwrap();
        assert_eq!(outcome.name, "goodbye");
        assert!(!outcome.removed_from.exists());
        let lock = PluginsLock::load(&home).unwrap();
        assert!(lock.plugins.is_empty());
    }

    #[test]
    fn uninstall_missing_plugin_errors() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let err = uninstall("ghost-plugin", &home, false).unwrap_err();
        assert!(matches!(err, InstallError::NotInstalled { .. }));
    }

    #[test]
    fn uninstall_purge_wipes_state_dir() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        // Source with a [state].dir declaration.
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        write_manifest(
            &src,
            "stateful",
            "[state]\ndir = \"$MAKAKOO_HOME/state/stateful\"\nretention = \"purge_on_uninstall\"",
        );

        install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap();

        // Pretend the plugin wrote some state.
        let state = home.join("state/stateful");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("journal.jsonl"), b"hi").unwrap();

        let outcome = uninstall("stateful", &home, true).unwrap();
        assert!(outcome.state_wiped);
        assert!(!state.exists());
    }

    #[test]
    fn install_then_reinstall_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let src = seed_source(tmp.path(), "dup");

        install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src.clone()),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap();

        let err = install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap_err();
        // staging.rs refuses when target already exists.
        assert!(matches!(
            err,
            InstallError::Staging(StagingError::TargetExists { .. })
        ));
    }
}
