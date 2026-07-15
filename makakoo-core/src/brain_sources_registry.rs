//! Cross-process locking and crash recovery for `brain_sources.json`.
//!
//! Python and Rust writers share the same lock file, recovery artifact names,
//! and ownership-marker format. Readers acquire the same exclusive lock so
//! they never observe a registry between replacement renames.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use fs2::FileExt;

pub const REGISTRY_RECOVERY_MARKER_PREFIX: &str =
    "makakoo-brain-sources-recovery-v1\ntarget=brain_sources.json\ncontent:\n";

pub struct BrainSourcesRegistry {
    home: PathBuf,
    _lock: File,
}

impl BrainSourcesRegistry {
    /// Acquire the Python-compatible registry lock and finish any owned,
    /// interrupted transaction before returning.
    pub fn acquire(home: &Path) -> anyhow::Result<Self> {
        let directory = home.join("config");
        fs::create_dir_all(&directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(directory.join("brain_sources.lock"))?;
        lock.lock_exclusive()?;
        recover_registry_files(home)?;
        Ok(Self {
            home: home.to_path_buf(),
            _lock: lock,
        })
    }

    /// Read the registry while retaining the lock. Missing registry is `None`;
    /// malformed filesystem entries fail closed.
    pub fn read_text(&self) -> anyhow::Result<Option<String>> {
        let path = config_path(&self.home);
        if !path_entry_exists(&path)? {
            return Ok(None);
        }
        if !fs::symlink_metadata(&path)?.file_type().is_file() {
            bail!(
                "brain source config is not a regular file: {}",
                path.display()
            );
        }
        fs::read_to_string(&path)
            .with_context(|| format!("read brain source config {}", path.display()))
            .map(Some)
    }

    /// Atomically replace the registry under the held lock. `body` must be the
    /// exact JSON bytes intended for the primary file and ownership marker.
    pub fn write_text(&self, body: &str) -> anyhow::Result<()> {
        serde_json::from_str::<serde_json::Value>(body)
            .context("brain source registry body is invalid JSON")?;
        let path = config_path(&self.home);
        let parent = path.parent().context("brain source config has no parent")?;
        fs::create_dir_all(parent)?;
        let temporary = config_temporary_path(&self.home);
        let backup = config_backup_path(&self.home);
        let marker = config_recovery_marker_path(&self.home);
        if path_entry_exists(&temporary)?
            || path_entry_exists(&backup)?
            || path_entry_exists(&marker)?
        {
            bail!("brain source recovery artifact appeared during registry update");
        }
        if path_entry_exists(&path)? && !fs::symlink_metadata(&path)?.file_type().is_file() {
            bail!(
                "refusing to replace non-file brain source config: {}",
                path.display()
            );
        }
        write_registry_marker(&self.home, body)?;
        let file_result = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary);
        let mut file = match file_result {
            Ok(file) => file,
            Err(error) => {
                let _ = remove_registry_marker(&self.home);
                return Err(error).context("create brain source config staging file");
            }
        };
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if !path_entry_exists(&path)? {
            fs::rename(&temporary, &path)?;
            sync_directory(parent)?;
            remove_registry_marker(&self.home)?;
            return Ok(());
        }
        fs::rename(&path, &backup)?;
        sync_directory(parent)?;
        if let Err(error) = fs::rename(&temporary, &path) {
            if fs::rename(&backup, &path).is_ok() {
                let _ = sync_directory(parent);
                if path_entry_exists(&temporary).unwrap_or(false) {
                    let _ = fs::remove_file(&temporary);
                    let _ = sync_directory(parent);
                }
                let _ = remove_registry_marker(&self.home);
            }
            return Err(error).context("atomically replace brain source config");
        }
        sync_directory(parent)?;
        fs::remove_file(&backup)?;
        sync_directory(parent)?;
        remove_registry_marker(&self.home)
    }
}

pub fn config_path(home: &Path) -> PathBuf {
    home.join("config/brain_sources.json")
}

pub fn config_backup_path(home: &Path) -> PathBuf {
    home.join("config/.brain_sources.json.backup")
}

pub fn config_temporary_path(home: &Path) -> PathBuf {
    home.join("config/.brain_sources.json.tmp")
}

pub fn config_recovery_marker_path(home: &Path) -> PathBuf {
    home.join("config/.brain_sources.json.owner")
}

fn path_entry_exists(path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn sync_directory(directory: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(directory)
        .with_context(|| format!("open directory for sync: {}", directory.display()))?
        .sync_all()
        .with_context(|| format!("sync directory: {}", directory.display()))?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

fn registry_marker_expected_body(home: &Path) -> anyhow::Result<Option<String>> {
    let marker = config_recovery_marker_path(home);
    if !path_entry_exists(&marker)? {
        return Ok(None);
    }
    if !fs::symlink_metadata(&marker)?.file_type().is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(marker)?;
    let Some(body) = raw.strip_prefix(REGISTRY_RECOVERY_MARKER_PREFIX) else {
        return Ok(None);
    };
    if serde_json::from_str::<serde_json::Value>(body).is_err() {
        return Ok(None);
    }
    Ok(Some(body.to_string()))
}

fn registry_marker_is_owned(home: &Path) -> anyhow::Result<bool> {
    Ok(registry_marker_expected_body(home)?.is_some())
}

fn require_owned_registry_artifacts(home: &Path) -> anyhow::Result<()> {
    if !registry_marker_is_owned(home)? {
        bail!(
            "refusing unowned brain source recovery artifacts in {}; move them aside or remove them manually",
            home.join("config").display()
        );
    }
    for artifact in [config_temporary_path(home), config_backup_path(home)] {
        if path_entry_exists(&artifact)? && !fs::symlink_metadata(&artifact)?.file_type().is_file()
        {
            bail!(
                "refusing non-file brain source recovery artifact {}; move it aside or remove it manually",
                artifact.display()
            );
        }
    }
    Ok(())
}

fn write_registry_marker(home: &Path, body: &str) -> anyhow::Result<()> {
    let marker = config_recovery_marker_path(home);
    if path_entry_exists(&marker)? {
        bail!(
            "brain source recovery marker collision at {}; move it aside or remove it manually",
            marker.display()
        );
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&marker)?;
    file.write_all(REGISTRY_RECOVERY_MARKER_PREFIX.as_bytes())?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    drop(file);
    sync_directory(marker.parent().context("registry marker has no parent")?)
}

fn remove_registry_marker(home: &Path) -> anyhow::Result<()> {
    let marker = config_recovery_marker_path(home);
    if !path_entry_exists(&marker)? {
        return Ok(());
    }
    if !registry_marker_is_owned(home)? {
        bail!(
            "refusing unowned brain source recovery marker {}; move it aside or remove it manually",
            marker.display()
        );
    }
    fs::remove_file(&marker)?;
    sync_directory(marker.parent().context("registry marker has no parent")?)
}

fn recover_registry_files(home: &Path) -> anyhow::Result<()> {
    let path = config_path(home);
    let backup = config_backup_path(home);
    let temporary = config_temporary_path(home);
    let marker = config_recovery_marker_path(home);
    let directory = home.join("config");
    let backup_exists = path_entry_exists(&backup)?;
    let temporary_exists = path_entry_exists(&temporary)?;
    if backup_exists || temporary_exists {
        require_owned_registry_artifacts(home)?;
    } else if path_entry_exists(&marker)? {
        let expected = registry_marker_expected_body(home)?
            .context("brain source recovery marker has no intended config body")?;
        if !path_entry_exists(&path)? {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)?;
            file.write_all(expected.as_bytes())?;
            file.sync_all()?;
            drop(file);
            sync_directory(&directory)?;
        } else {
            if !fs::symlink_metadata(&path)?.file_type().is_file() {
                bail!(
                    "refusing marker-only brain source recovery because primary is not a regular file: {}",
                    path.display()
                );
            }
            if fs::read_to_string(&path)? != expected {
                remove_registry_marker(home)?;
                return Ok(());
            }
        }
        remove_registry_marker(home)?;
        return Ok(());
    }

    let mut changed = false;
    let mut temporary_consumed = false;
    if !path_entry_exists(&path)? && !backup_exists && temporary_exists {
        let expected = registry_marker_expected_body(home)?
            .context("brain source recovery marker has no intended config body")?;
        if fs::read_to_string(&temporary).ok().as_deref() == Some(expected.as_str()) {
            File::open(&temporary)?.sync_all()?;
            fs::rename(&temporary, &path)
                .context("promote recovered initial brain source config")?;
        } else {
            fs::remove_file(&temporary)?;
            sync_directory(&directory)?;
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)?;
            file.write_all(expected.as_bytes())?;
            file.sync_all()?;
        }
        temporary_consumed = true;
        changed = true;
    } else if !path_entry_exists(&path)? && backup_exists {
        fs::rename(&backup, &path).context("recover interrupted brain source config write")?;
        changed = true;
    } else if path_entry_exists(&path)? && backup_exists {
        if !fs::symlink_metadata(&path)?.file_type().is_file() {
            bail!(
                "refusing to discard brain source backup because primary is not a regular file: {}",
                path.display()
            );
        }
        let expected = registry_marker_expected_body(home)?
            .context("brain source recovery marker has no intended config body")?;
        let actual = fs::read_to_string(&path)?;
        if actual != expected {
            bail!(
                "refusing to discard brain source backup because primary does not match the owned transaction"
            );
        }
        sync_directory(&directory)?;
        fs::remove_file(&backup)?;
        changed = true;
    }
    if temporary_exists && !temporary_consumed {
        fs::remove_file(&temporary)?;
        changed = true;
    }
    if changed {
        sync_directory(&directory)?;
    }
    remove_registry_marker(home)
}
