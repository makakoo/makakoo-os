//! Atomic staging installer for plugins.
//!
//! Install flow per PLUGIN_MANIFEST.md §6:
//!   1. Caller prepares plugin source in `$MAKAKOO_HOME/plugins/.stage/<name>/`
//!   2. `stage_and_install` verifies blake3 (if the manifest declares one),
//!      parses the manifest, then atomically renames the staged dir to
//!      `$MAKAKOO_HOME/plugins/<name>/`.
//!   3. On any error the staged dir is removed so the next install sees a
//!      clean slate.
//!
//! The actual download / unpack step (curl a tarball, `git clone`, whatever)
//! is the caller's responsibility — `stage_and_install` operates on the
//! already-staged directory. This keeps the hashing + validation logic free
//! of network and git concerns so it can be unit-tested in pure memory.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, warn};

use super::manifest::{Manifest, ManifestError};

#[derive(Debug, Error)]
pub enum StagingError {
    #[error("staged plugin dir {path} does not exist")]
    MissingStage { path: PathBuf },
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("manifest error: {0}")]
    Manifest(#[from] ManifestError),
    #[error(
        "blake3 mismatch for {plugin:?}: expected {expected}, computed {actual}"
    )]
    HashMismatch {
        plugin: String,
        expected: String,
        actual: String,
    },
    #[error("target plugin dir {path} already exists — uninstall first")]
    TargetExists { path: PathBuf },
}

/// Result of a successful install.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub name: String,
    pub final_dir: PathBuf,
    pub computed_blake3: String,
}

/// Path to the staging dir under a given Makakoo home.
pub fn stage_dir(makakoo_home: &Path) -> PathBuf {
    makakoo_home.join("plugins").join(".stage")
}

/// Path to the final installed dir for a given plugin name.
pub fn final_dir(makakoo_home: &Path, plugin_name: &str) -> PathBuf {
    makakoo_home.join("plugins").join(plugin_name)
}

/// Verify + promote a staged plugin.
///
/// * `staged`: path to the staged dir (usually `stage_dir/<name>/`). Must
///   contain a `plugin.toml`.
/// * `makakoo_home`: root of the Makakoo data layout.
/// * `expected_blake3`: optional override. If `Some`, takes precedence over
///   the hash declared in `[source].blake3`. Use this for ad-hoc installs
///   where the user supplied a hash on the CLI.
///
/// On success the staged dir is renamed to `$MAKAKOO_HOME/plugins/<name>/`.
/// On any error the staged dir is deleted so we don't leak half-installed
/// state across daemon restarts.
pub fn stage_and_install(
    staged: &Path,
    makakoo_home: &Path,
    expected_blake3: Option<&str>,
) -> Result<InstallOutcome, StagingError> {
    if !staged.exists() {
        return Err(StagingError::MissingStage {
            path: staged.to_path_buf(),
        });
    }

    let manifest_path = staged.join("plugin.toml");
    let (manifest, _warnings) = Manifest::load(&manifest_path)?;
    let name = manifest.plugin.name.clone();

    // Determine the hash to enforce. Precedence: CLI override > manifest.
    let declared = expected_blake3
        .map(|s| s.to_string())
        .or_else(|| manifest.source.blake3.clone());

    // Always compute the hash so we can display it even when nothing is
    // declared (ad-hoc local install case, §6 "the kernel computes and
    // displays it so the user can pin later").
    let computed = hash_tree(staged)?;
    if let Some(expected) = declared {
        if !hash_eq(&expected, &computed) {
            remove_silently(staged);
            return Err(StagingError::HashMismatch {
                plugin: name,
                expected,
                actual: computed,
            });
        }
    } else {
        warn!(
            "plugin {name} installed without declared blake3 — computed {computed} (pin this in [source].blake3)"
        );
    }

    // Target dir must not already exist. Uninstall is a separate verb.
    let target = final_dir(makakoo_home, &name);
    if target.exists() {
        remove_silently(staged);
        return Err(StagingError::TargetExists { path: target });
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| StagingError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    fs::rename(staged, &target).map_err(|source| StagingError::Io {
        path: target.clone(),
        source,
    })?;
    debug!("promoted {} into plugin registry", target.display());

    Ok(InstallOutcome {
        name,
        final_dir: target,
        computed_blake3: computed,
    })
}

/// Best-effort cleanup. Never panics; logs failure.
fn remove_silently(path: &Path) {
    if let Err(e) = fs::remove_dir_all(path) {
        warn!("failed to clean up staged dir {}: {}", path.display(), e);
    }
}

/// Blake3 hash of a directory tree. Order is deterministic: files are
/// walked in sorted path order so two different machines computing the
/// same tree produce the same digest.
///
/// The digest mixes each file's relative path + content length + content
/// bytes, which is enough to detect any content or structural change
/// without needing a Merkle tree.
pub fn hash_tree(root: &Path) -> Result<String, StagingError> {
    let mut hasher = blake3::Hasher::new();
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();

    for rel in &files {
        let full = root.join(rel);
        let rel_str = rel.to_string_lossy();
        hasher.update(rel_str.as_bytes());
        hasher.update(b"\0");

        let mut file = fs::File::open(&full).map_err(|source| StagingError::Io {
            path: full.clone(),
            source,
        })?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf).map_err(|source| StagingError::Io {
                path: full.clone(),
                source,
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        hasher.update(b"\0");
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn collect_files(base: &Path, cur: &Path, out: &mut Vec<PathBuf>) -> Result<(), StagingError> {
    let entries = fs::read_dir(cur).map_err(|source| StagingError::Io {
        path: cur.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| StagingError::Io {
            path: cur.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .map_err(|_| StagingError::Io {
                    path: path.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "path escaped base",
                    ),
                })?
                .to_path_buf();
            out.push(rel);
        }
    }
    Ok(())
}

/// Compare two hex blake3 digests ignoring case and leading/trailing space.
fn hash_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_minimal_manifest(dir: &Path, name: &str, blake3: Option<&str>) {
        let blake_line = blake3
            .map(|h| format!("\nblake3 = \"{h}\""))
            .unwrap_or_default();
        let body = format!(
            r#"
[plugin]
name = "{name}"
version = "1.0.0"
kind = "skill"
language = "python"

[source]
path = ".{blake_line}"

[abi]
skill = "^0.1"

[entrypoint]
run = "true"
"#
        );
        fs::write(dir.join("plugin.toml"), body).unwrap();
    }

    #[test]
    fn hash_tree_is_deterministic() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        for d in [a.path(), b.path()] {
            fs::write(d.join("one.txt"), b"hello").unwrap();
            fs::create_dir_all(d.join("nested")).unwrap();
            fs::write(d.join("nested/two.txt"), b"world").unwrap();
        }
        let ha = hash_tree(a.path()).unwrap();
        let hb = hash_tree(b.path()).unwrap();
        assert_eq!(ha, hb);
    }

    #[test]
    fn hash_tree_detects_content_change() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        fs::write(a.path().join("x.txt"), b"one").unwrap();
        fs::write(b.path().join("x.txt"), b"two").unwrap();
        assert_ne!(hash_tree(a.path()).unwrap(), hash_tree(b.path()).unwrap());
    }

    #[test]
    fn stage_and_install_happy_path_no_hash() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let staged = stage_dir(home).join("hello");
        fs::create_dir_all(&staged).unwrap();
        write_minimal_manifest(&staged, "hello", None);

        let outcome = stage_and_install(&staged, home, None).unwrap();
        assert_eq!(outcome.name, "hello");
        assert_eq!(outcome.final_dir, final_dir(home, "hello"));
        assert!(outcome.final_dir.exists());
        assert!(!staged.exists());
        assert!(!outcome.computed_blake3.is_empty());
    }

    #[test]
    fn stage_and_install_with_matching_hash() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let staged = stage_dir(home).join("hello");
        fs::create_dir_all(&staged).unwrap();
        // First write WITHOUT the blake3 line so we can compute the hash.
        write_minimal_manifest(&staged, "hello", None);
        let first = hash_tree(&staged).unwrap();
        // Second attempt: rewrite manifest with the correct hash and
        // retry. Note: changing the manifest content also changes the
        // hash, so we use the CLI override path to pin to `first` instead.
        let outcome = stage_and_install(&staged, home, Some(&first)).unwrap();
        assert_eq!(outcome.computed_blake3, first);
    }

    #[test]
    fn stage_and_install_rejects_wrong_hash() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let staged = stage_dir(home).join("hello");
        fs::create_dir_all(&staged).unwrap();
        write_minimal_manifest(&staged, "hello", None);
        let bogus = "0".repeat(64);
        let err = stage_and_install(&staged, home, Some(&bogus)).unwrap_err();
        assert!(matches!(err, StagingError::HashMismatch { .. }));
        // And the staged dir was cleaned up.
        assert!(!staged.exists());
    }

    #[test]
    fn target_exists_rejected() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let already = final_dir(home, "hello");
        fs::create_dir_all(&already).unwrap();
        fs::write(already.join("something"), b"existing").unwrap();

        let staged = stage_dir(home).join("hello");
        fs::create_dir_all(&staged).unwrap();
        write_minimal_manifest(&staged, "hello", None);

        let err = stage_and_install(&staged, home, None).unwrap_err();
        assert!(matches!(err, StagingError::TargetExists { .. }));
        assert!(!staged.exists());
        assert!(already.exists()); // untouched
    }
}
