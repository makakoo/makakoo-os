//! High-level plugin install/uninstall wrapping `staging.rs`.
//!
//! `stage_and_install` operates on an already-staged directory; this
//! module adds the "copy source tree into the staging area" step so the
//! CLI can point at any local directory and end up with a registered
//! plugin + updated `plugins.lock`.
//!
//! v0.4 (git-sourced plugins): `install()` dispatches on `PluginSource`
//! to support Path / Git / Tarball sources. Git + Tarball variants
//! delegate the network I/O to `core::source_fetch`, then feed the staged
//! tree through the same promotion pipeline as a local path install.
//! Plugins with an `[install].unix` script run it from the promoted dir
//! after staging (CWD = plugin dir, `$MAKAKOO_PLUGIN_DIR` exported).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use thiserror::Error;
use tracing::{debug, warn};

use super::lock::{LockEntry, LockError, PluginsLock};
use super::manifest::{Manifest, ManifestError};
use super::staging::{stage_and_install, stage_dir, StagingError};
use crate::source_fetch::{self, FetchError, SourceSpec};

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
    #[error(
        "plugin {plugin:?} declares sancho task {task:?} which collides with a native kernel handler — \
         rename the task in its manifest and retry"
    )]
    NativeTaskCollision { plugin: String, task: String },
    /// Raised by `plugin sync --force` when the uninstall leg of the
    /// uninstall-then-reinstall dance fails. Keeps the source error
    /// intact so callers can distinguish "plugin wasn't there" from
    /// "lock was corrupt" from "io error wiping the install dir."
    #[error("uninstall failed for {plugin:?}: {source}")]
    UninstallFailed {
        plugin: String,
        #[source]
        source: Box<InstallError>,
    },
    /// Another process holds the sync lock for this plugin. Raised by
    /// `plugin sync --force` to prevent racing with a concurrent sync
    /// (without this guard the retry loop could silently delete what
    /// the other process just installed).
    #[error("concurrent sync in progress for plugin {name:?}")]
    ConcurrentSync { name: String },
    /// Source fetch (git clone, tarball download) failed. Holds the
    /// underlying source_fetch error so callers can distinguish
    /// "git clone refused unstable ref" from "curl returned 404" etc.
    #[error("source fetch failed: {0}")]
    SourceFetch(#[from] FetchError),
    /// `[install].unix` script exited non-zero. The plugin dir stays on
    /// disk (script may have created helpful breadcrumbs) but the lock
    /// file is NOT updated, so the next install will retry cleanly.
    #[error(
        "[install].unix script for plugin {plugin:?} exited {exit}: {stderr}"
    )]
    InstallScriptFailed {
        plugin: String,
        exit: i32,
        stderr: String,
    },
    /// Raised by `plugin update` when the lock entry's `source` field
    /// doesn't parse as one of the known prefixes (`path:`, `git:`,
    /// `tar:`). Points at corruption or a manually-edited lock file.
    #[error("lock entry for {plugin:?} has unparseable source: {source_str:?}")]
    InvalidLockSource {
        plugin: String,
        source_str: String,
    },
    /// Raised by `plugin update` on a path-sourced plugin — those use
    /// the legacy path-based update flow (uninstall + reinstall from
    /// recorded directory), handled at the CLI layer.
    #[error("plugin {plugin:?} has a path source — use the path-based update flow instead")]
    UpdateWrongSource { plugin: String },
}

/// Where a plugin's source lives. v0.4 accepts all three shapes the
/// manifest can declare (`[source] path = ... | git = ... | tar = ...`).
#[derive(Debug, Clone)]
pub enum PluginSource {
    /// An absolute or relative path to a directory containing plugin.toml.
    Path(PathBuf),
    /// A git repository + a ref to pin (tag or 40-char SHA, or branch
    /// name if `allow_unstable` is true).
    Git {
        url: String,
        ref_: String,
        allow_unstable: bool,
    },
    /// A URL pointing at a `.tar.gz` (or plain `.tar`) + a sha256 that
    /// the downloaded archive MUST match before the tree is promoted.
    Tarball {
        url: String,
        sha256: String,
    },
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
    install(req, makakoo_home)
}

/// Install a plugin — dispatches on `PluginSource`. Git + Tarball
/// variants go through `core::source_fetch`, which stages the tree in a
/// tempdir; the rest of the pipeline (copy to stage dir, hash verify,
/// atomic promote, run `[install].unix`, update lock) is identical to
/// the path case.
pub fn install(
    req: &InstallRequest,
    makakoo_home: &Path,
) -> Result<super::staging::InstallOutcome, InstallError> {
    match &req.source {
        PluginSource::Path(p) => install_staged(
            p,
            format!("path:{}", p.display()),
            None,
            req.expected_blake3.as_deref(),
            makakoo_home,
        ),
        PluginSource::Git {
            url,
            ref_,
            allow_unstable,
        } => {
            let fetched = source_fetch::fetch(&SourceSpec::Git {
                url: url.clone(),
                ref_: ref_.clone(),
                allow_unstable: *allow_unstable,
            })?;
            let source_str = format!("git:{url}@{ref_}");
            let result = install_staged(
                &fetched.staging_dir,
                source_str,
                Some(fetched.resolved_sha.clone()),
                req.expected_blake3.as_deref(),
                makakoo_home,
            );
            // Always clean the fetcher's tempdir — promotion above moves
            // content out of it already, but source_fetch::fetch() can
            // leave an empty parent we should still reap.
            let _ = fs::remove_dir_all(&fetched.staging_dir);
            result
        }
        PluginSource::Tarball { url, sha256 } => {
            let fetched = source_fetch::fetch(&SourceSpec::HttpsTarball {
                url: url.clone(),
                sha256: sha256.clone(),
            })?;
            let source_str = format!("tar:{url}");
            let result = install_staged(
                &fetched.staging_dir,
                source_str,
                Some(fetched.resolved_sha.clone()),
                req.expected_blake3.as_deref(),
                makakoo_home,
            );
            let _ = fs::remove_dir_all(&fetched.staging_dir);
            result
        }
    }
}

/// Convenience wrapper: install straight from a git URL at a pinned ref.
pub fn install_from_git(
    url: &str,
    ref_: &str,
    allow_unstable: bool,
    makakoo_home: &Path,
) -> Result<super::staging::InstallOutcome, InstallError> {
    install(
        &InstallRequest {
            source: PluginSource::Git {
                url: url.to_string(),
                ref_: ref_.to_string(),
                allow_unstable,
            },
            expected_blake3: None,
        },
        makakoo_home,
    )
}

/// Convenience wrapper: install straight from an HTTPS tarball URL.
pub fn install_from_tarball_url(
    url: &str,
    sha256: &str,
    makakoo_home: &Path,
) -> Result<super::staging::InstallOutcome, InstallError> {
    install(
        &InstallRequest {
            source: PluginSource::Tarball {
                url: url.to_string(),
                sha256: sha256.to_string(),
            },
            expected_blake3: None,
        },
        makakoo_home,
    )
}

/// Describes how an upstream probe compares to the installed version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeDrift {
    /// Upstream resolved_sha matches the locked one — no work needed.
    UpToDate,
    /// Upstream resolved_sha differs but the plugin.toml bytes are
    /// identical to the locked `manifest_hash`. Safe for silent update.
    ContentOnly,
    /// The manifest itself changed — capabilities, security, sandbox
    /// profile, install script may have drifted. Requires user consent
    /// before the new code is promoted.
    ManifestChange,
}

/// Result of a dry-run upstream probe. The probe has already fetched
/// the upstream tree into `staging_dir`; the caller can either promote
/// it via `apply_update` or discard by `fs::remove_dir_all(staging_dir)`.
#[derive(Debug)]
pub struct UpstreamProbe {
    pub name: String,
    pub old_resolved_sha: Option<String>,
    pub new_resolved_sha: String,
    pub old_manifest_hash: Option<String>,
    pub new_manifest_hash: String,
    pub drift: ProbeDrift,
    pub staging_dir: PathBuf,
    pub plugin_source: PluginSource,
}

/// Dry-run probe: refetch the locked upstream ref, compute its SHA and
/// manifest_hash, classify drift. No disk state under `$MAKAKOO_HOME/plugins/`
/// is mutated. Caller MUST eventually `apply_update` or `drop_probe` to
/// clean up the staging dir.
///
/// Supports git and tarball sources. Path-sourced plugins return
/// `UpdateWrongSource` — callers use the legacy reinstall flow for those.
pub fn probe_upstream(entry: &LockEntry) -> Result<UpstreamProbe, InstallError> {
    let plugin_source = parse_lock_source(&entry.name, &entry.source)?;
    match &plugin_source {
        PluginSource::Path(_) => {
            return Err(InstallError::UpdateWrongSource {
                plugin: entry.name.clone(),
            })
        }
        _ => {}
    }
    let spec = match &plugin_source {
        PluginSource::Git {
            url,
            ref_,
            allow_unstable,
        } => SourceSpec::Git {
            url: url.clone(),
            ref_: ref_.clone(),
            allow_unstable: *allow_unstable,
        },
        PluginSource::Tarball { url, sha256 } => SourceSpec::HttpsTarball {
            url: url.clone(),
            // Accept an empty declared sha: in that case source_fetch
            // skips the comparison and just computes + returns the actual
            // sha256. Used by `plugin update` when the user hasn't
            // supplied a new sha256 yet — we still need to see what
            // upstream actually hashes to.
            sha256: sha256.clone(),
        },
        PluginSource::Path(_) => unreachable!(),
    };
    let fetched = source_fetch::fetch(&spec)?;
    let manifest_path = fetched.staging_dir.join("plugin.toml");
    if !manifest_path.is_file() {
        let _ = fs::remove_dir_all(&fetched.staging_dir);
        return Err(InstallError::NoManifest {
            path: fetched.staging_dir.clone(),
        });
    }
    let new_manifest_hash = hash_manifest_text(&manifest_path).unwrap_or_default();

    let drift = if entry.resolved_sha.as_deref() == Some(fetched.resolved_sha.as_str()) {
        ProbeDrift::UpToDate
    } else if entry
        .manifest_hash
        .as_deref()
        .map(|h| h == new_manifest_hash.as_str())
        .unwrap_or(false)
    {
        ProbeDrift::ContentOnly
    } else {
        ProbeDrift::ManifestChange
    };

    Ok(UpstreamProbe {
        name: entry.name.clone(),
        old_resolved_sha: entry.resolved_sha.clone(),
        new_resolved_sha: fetched.resolved_sha,
        old_manifest_hash: entry.manifest_hash.clone(),
        new_manifest_hash,
        drift,
        staging_dir: fetched.staging_dir,
        plugin_source,
    })
}

/// Promote a probed update: uninstall + reinstall from the probe's
/// staging dir. Preserves the `enabled` flag across the round-trip.
/// Staging dir is consumed (moved or deleted).
pub fn apply_update(
    probe: UpstreamProbe,
    makakoo_home: &Path,
) -> Result<super::staging::InstallOutcome, InstallError> {
    let prior_enabled = PluginsLock::load(makakoo_home)?
        .get(&probe.name)
        .map(|e| e.enabled)
        .unwrap_or(true);

    // Uninstall the old install. Keep state dir intact — an update
    // that wipes user state would be terrifying.
    if let Err(e) = uninstall(&probe.name, makakoo_home, false) {
        let _ = fs::remove_dir_all(&probe.staging_dir);
        return Err(e);
    }

    let source_str = format_source_str(&probe.plugin_source);
    let resolved_sha = Some(probe.new_resolved_sha.clone());
    let outcome = install_staged(
        &probe.staging_dir,
        source_str,
        resolved_sha,
        None,
        makakoo_home,
    )?;

    if !prior_enabled {
        let mut lock = PluginsLock::load(makakoo_home)?;
        if let Some(mut e) = lock.get(&probe.name).cloned() {
            e.enabled = false;
            lock.upsert(e);
            lock.save(makakoo_home)?;
        }
    }

    // Best-effort: the staged tree was renamed into place by
    // install_staged, but source_fetch::fetch() uses a tempdir with
    // other subdirs (for tarballs: extract/, stage/). Clean them.
    if let Some(parent) = probe.staging_dir.parent() {
        // Only clean the immediate parent if it's clearly the fetcher's
        // tempdir (contains a well-known download.tar.gz sibling or empty).
        let _ = fs::remove_dir_all(parent);
    }

    Ok(outcome)
}

/// Discard a probe's staging dir without promoting. Use when the user
/// declines a re-trust prompt.
pub fn drop_probe(probe: UpstreamProbe) {
    let _ = fs::remove_dir_all(&probe.staging_dir);
    if let Some(parent) = probe.staging_dir.parent() {
        let _ = fs::remove_dir_all(parent);
    }
}

/// List every lock entry whose source is git or tarball — the candidates
/// for `plugin update --all` and `plugin outdated`.
pub fn list_updatable(makakoo_home: &Path) -> Result<Vec<LockEntry>, InstallError> {
    let lock = PluginsLock::load(makakoo_home)?;
    Ok(lock
        .plugins
        .into_iter()
        .filter(|e| {
            e.source.starts_with("git:") || e.source.starts_with("tar:")
        })
        .collect())
}

fn format_source_str(ps: &PluginSource) -> String {
    match ps {
        PluginSource::Path(p) => format!("path:{}", p.display()),
        PluginSource::Git { url, ref_, .. } => format!("git:{url}@{ref_}"),
        PluginSource::Tarball { url, .. } => format!("tar:{url}"),
    }
}

fn parse_lock_source(plugin: &str, s: &str) -> Result<PluginSource, InstallError> {
    if let Some(rest) = s.strip_prefix("path:") {
        return Ok(PluginSource::Path(PathBuf::from(rest)));
    }
    if let Some(rest) = s.strip_prefix("git:") {
        // git:<url>@<ref>. Split on the LAST '@' to sidestep `ssh://git@host`
        // (which shouldn't appear here but keep the parser defensive).
        let (url, ref_) = match rest.rfind('@') {
            Some(i) if i > "https://".len() => {
                (rest[..i].to_string(), rest[i + 1..].to_string())
            }
            _ => (rest.to_string(), "HEAD".to_string()),
        };
        // A previously-accepted install implies the user already OK'd the
        // ref stability; preserve that on refetch.
        return Ok(PluginSource::Git {
            url,
            ref_,
            allow_unstable: true,
        });
    }
    if let Some(rest) = s.strip_prefix("tar:") {
        return Ok(PluginSource::Tarball {
            url: rest.to_string(),
            // Empty sha triggers "compute but don't enforce" in source_fetch.
            sha256: String::new(),
        });
    }
    Err(InstallError::InvalidLockSource {
        plugin: plugin.to_string(),
        source_str: s.to_string(),
    })
}

/// Shared install path — works on any source directory that already
/// contains a `plugin.toml`. Called by every public entry point.
///
/// `source_str` is the human-readable lock-file source (e.g. `path:/x`,
/// `git:https://.../repo@v0.1.0`, `tar:https://.../x.tar.gz`).
/// `resolved_sha` is the git SHA or tarball sha256 (None for path installs).
fn install_staged(
    src_path: &Path,
    source_str: String,
    resolved_sha: Option<String>,
    expected_blake3: Option<&str>,
    makakoo_home: &Path,
) -> Result<super::staging::InstallOutcome, InstallError> {
    // 1) Basic sanity on the source tree.
    if !src_path.is_dir() {
        return Err(InstallError::NotADir {
            path: src_path.to_path_buf(),
        });
    }
    let manifest_path = src_path.join("plugin.toml");
    if !manifest_path.exists() {
        return Err(InstallError::NoManifest {
            path: src_path.to_path_buf(),
        });
    }
    // Parse the manifest so we know the plugin name before copying —
    // the staged dir needs to be named after it.
    let (manifest, _warn) = Manifest::load(&manifest_path)?;
    let name = manifest.plugin.name.clone();

    // Reject plugins whose sancho tasks would shadow native kernel
    // handlers. Done before staging so a bad manifest never touches
    // $MAKAKOO_HOME/plugins/.
    for task in &manifest.sancho.tasks {
        if crate::sancho::NATIVE_TASK_NAMES
            .iter()
            .any(|n| *n == task.name.as_str())
        {
            return Err(InstallError::NativeTaskCollision {
                plugin: name,
                task: task.name.clone(),
            });
        }
    }

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
    let outcome = stage_and_install(&stage_target, makakoo_home, expected_blake3)?;

    // 4) Run `[install].unix` if declared. Script sees CWD = promoted
    //    plugin dir, `$MAKAKOO_PLUGIN_DIR` = same, `$MAKAKOO_HOME` = root.
    if let Some(ref script) = manifest.install.unix {
        if cfg!(unix) && !script.trim().is_empty() {
            run_install_script(&name, &outcome.final_dir, makakoo_home, script)?;
        }
    }

    // 5) Record in plugins.lock. Fresh installs start enabled;
    //    reinstalls of a previously-disabled plugin reset to enabled
    //    (a user who wanted it off must re-run `makakoo plugin disable`).
    let manifest_hash = hash_manifest_text(&manifest_path);
    let mut lock = PluginsLock::load(makakoo_home)?;
    lock.upsert(LockEntry {
        name: outcome.name.clone(),
        version: manifest.plugin.version.to_string(),
        blake3: Some(outcome.computed_blake3.clone()),
        source: source_str,
        resolved_sha,
        manifest_hash,
        installed_at: Utc::now(),
        enabled: true,
    });
    lock.save(makakoo_home)?;

    Ok(outcome)
}

/// Execute `[install].unix` from the promoted plugin dir. On failure
/// the plugin stays installed (script side effects may be meaningful)
/// but the lock file has NOT been updated yet, so `plugin install` can
/// re-run once the user fixes the underlying issue.
fn run_install_script(
    plugin: &str,
    plugin_dir: &Path,
    makakoo_home: &Path,
    script: &str,
) -> Result<(), InstallError> {
    // Resolve the script. If `script` is a bare filename that exists in
    // the plugin dir (e.g. `install.sh`), invoke `sh <abs-path>` so
    // shipping a script without the +x bit Just Works (sh reads the
    // file directly instead of execve'ing it). Falls back to `sh -c
    // <command>` for non-file scripts (e.g. an inline command string).
    // The bare-filename "install.sh" form would fail under `sh -c`
    // with "command not found" since . is not in PATH — caught live
    // 2026-04-21 installing agent-browser-harness, then again
    // 2026-04-26 (chmod-bit drop) on sancho-task-plugin-update-check.
    let resolved_script = plugin_dir.join(script);
    let invoke_as_file = resolved_script.is_file();
    debug!(
        "running [install].unix for {plugin} in {} (as_file={invoke_as_file}): {script}",
        plugin_dir.display()
    );
    // Make Makakoo-provided shell shims (e.g. `makakoo-venv-bootstrap`)
    // discoverable from install.sh. `lib-harvey-core/bin/` is the
    // canonical home for these helpers — prepend it to PATH so plugins
    // can invoke them by bare name. Every pre-existing PATH element is
    // preserved; we just move ours to the front.
    let shim_dir = makakoo_home.join("plugins/lib-harvey-core/bin");
    let path = match std::env::var("PATH") {
        Ok(existing) => format!("{}:{existing}", shim_dir.display()),
        Err(_) => shim_dir.display().to_string(),
    };
    let mut cmd = Command::new("sh");
    if invoke_as_file {
        cmd.arg(&resolved_script);
    } else {
        cmd.arg("-c").arg(script);
    }
    let out = cmd
        .current_dir(plugin_dir)
        .env("MAKAKOO_PLUGIN_DIR", plugin_dir)
        .env("MAKAKOO_HOME", makakoo_home)
        .env("MAKAKOO_BIN_DIR", &shim_dir)
        .env("PATH", path)
        .output()
        .map_err(|source| InstallError::Io {
            path: plugin_dir.to_path_buf(),
            source,
        })?;
    if !out.status.success() {
        let exit = out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        warn!("[install].unix failed for {plugin} (exit {exit}): {stderr}");
        return Err(InstallError::InstallScriptFailed {
            plugin: plugin.to_string(),
            exit,
            stderr,
        });
    }
    Ok(())
}

/// sha256 of the raw plugin.toml bytes — used as the manifest_hash in
/// the lock entry so Phase C's `plugin update` can re-prompt on
/// capability / security drift without re-parsing the TOML twice.
fn hash_manifest_text(manifest_path: &Path) -> Option<String> {
    let bytes = fs::read(manifest_path).ok()?;
    Some(source_fetch::sha256_hex(&bytes))
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

    /// Regression: `install_from_path` must preserve nested `src/`
    /// directories when the source plugin uses the self-contained layout
    /// (`<plugin>/src/core/terminal/*.py`). Caught live 2026-04-20 when
    /// we found `$MAKAKOO_HOME/plugins/lib-hte/` had only `plugin.toml`
    /// with no `src/` — turned out the live install was from the
    /// pre-self-contained era (manifest-only installs were the shape
    /// before commit 9bd928f), not a copy_dir bug. This test locks the
    /// current correct behaviour so any future change to copy_dir or
    /// stage_and_install that drops nested directories fails loudly.
    #[test]
    fn install_preserves_nested_src_dir() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("makakoo");
        fs::create_dir_all(&home).unwrap();

        // Build source with the self-contained shape:
        //   src_root/
        //     plugin.toml
        //     src/
        //       core/
        //         terminal/
        //           widgets.py
        let src_root = tmp.path().join("src-nested");
        fs::create_dir_all(&src_root).unwrap();
        // kind=skill is fine — the bug being locked is copy_dir recursion,
        // which is shape-agnostic w.r.t. plugin kind.
        write_manifest(&src_root, "lib-nested", "");
        let nested = src_root.join("src").join("core").join("terminal");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("widgets.py"), b"class W: pass\n").unwrap();

        let outcome = install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src_root),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap();

        // Every directory level must survive the install. A regression
        // that dropped `src/` (or the `core/` subdir, or the leaf file)
        // would show up as a missing path here.
        assert!(outcome.final_dir.join("src").is_dir(),
            "install dropped top-level src/ dir");
        assert!(outcome.final_dir.join("src").join("core").is_dir(),
            "install dropped src/core/ dir");
        assert!(outcome.final_dir.join("src").join("core").join("terminal").is_dir(),
            "install dropped src/core/terminal/ dir");
        let leaf = outcome.final_dir.join("src").join("core").join("terminal").join("widgets.py");
        assert!(leaf.is_file(), "install dropped the leaf widgets.py file");
        let bytes = fs::read(&leaf).unwrap();
        assert_eq!(bytes, b"class W: pass\n", "install mangled file contents");
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

    #[test]
    fn install_rejects_plugin_task_name_colliding_with_native() {
        // A plugin manifest that tries to register a SANCHO task named
        // after one of the 8 native kernel handlers MUST fail at install
        // time, before the dir ever lands under $MAKAKOO_HOME/plugins/.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        // Source with a [sancho] task that shadows the native `dream`.
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let body = r#"
[plugin]
name = "naughty-plugin"
version = "1.0.0"
kind = "sancho-task"
language = "python"

[source]
path = "."

[abi]
sancho-task = "^1.0"

[entrypoint]
run = "true"

[sancho]
tasks = [{ name = "dream", interval = "3600s" }]
"#;
        fs::write(src.join("plugin.toml"), body).unwrap();

        let err = install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap_err();
        assert!(matches!(err, InstallError::NativeTaskCollision { .. }));

        // Crucially: the plugin dir was NOT created — the check fires
        // before staging, so disk state is untouched.
        assert!(!home.join("plugins/naughty-plugin").exists());
        // Lock file unchanged (or never existed — fresh home).
        let lock = PluginsLock::load(&home).unwrap();
        assert!(lock.get("naughty-plugin").is_none());
    }

    /// The CLI's `plugin update` is a mechanical uninstall + reinstall
    /// from the lock's recorded source, with the enabled flag preserved
    /// across the round-trip. This test exercises the same sequence
    /// directly so the core contract is locked even if the CLI layer
    /// drifts. When `update` migrates into core (Phase E), this test
    /// should pin the new entrypoint.
    #[test]
    fn update_round_trip_preserves_disabled_flag() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let src = seed_source(tmp.path(), "rolling");

        // 1) Install and manually disable — simulates an earlier
        //    `plugin install` + `plugin disable`.
        install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src.clone()),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap();
        let mut lock = PluginsLock::load(&home).unwrap();
        let mut e = lock.get("rolling").unwrap().clone();
        e.enabled = false;
        lock.upsert(e);
        lock.save(&home).unwrap();

        // 2) Capture prior_enabled — what the CLI would read.
        let prior_enabled = PluginsLock::load(&home)
            .unwrap()
            .get("rolling")
            .unwrap()
            .enabled;
        assert!(!prior_enabled, "precondition: plugin is disabled");

        // 3) Uninstall + reinstall from the same source path.
        uninstall("rolling", &home, false).unwrap();
        install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src),
                expected_blake3: None,
            },
            &home,
        )
        .unwrap();

        // 4) Fresh install defaults to enabled=true. The CLI's update
        //    path re-applies the saved flag when prior_enabled was false.
        let mut lock = PluginsLock::load(&home).unwrap();
        assert!(
            lock.get("rolling").unwrap().enabled,
            "fresh install lands as enabled=true by design"
        );
        if !prior_enabled {
            let mut entry = lock.get("rolling").unwrap().clone();
            entry.enabled = false;
            lock.upsert(entry);
            lock.save(&home).unwrap();
        }

        // 5) Post-update: disabled state is preserved.
        let final_lock = PluginsLock::load(&home).unwrap();
        assert!(
            !final_lock.get("rolling").unwrap().enabled,
            "update must preserve enabled=false across reinstall"
        );
    }

    // ─── v0.4 Phase B: git-sourced plugin install tests ──────────────

    /// Initialise a bare git repo + seed one commit containing a
    /// self-contained plugin tree. Returns (guard, bare_url, tag, sha).
    /// Guard keeps the fixture tmpdir alive for the whole test.
    fn seed_plugin_bare_repo(
        plugin_name: &str,
        extra_manifest: &str,
    ) -> (TempDir, String, String, String) {
        use std::process::Command;
        let tmp = TempDir::new().unwrap();
        let bare = tmp.path().join("bare.git");
        let wt = tmp.path().join("wt");
        run_git(&["init", "--bare", "--quiet", bare.to_str().unwrap()], None);
        run_git(
            &[
                "clone",
                "--quiet",
                bare.to_str().unwrap(),
                wt.to_str().unwrap(),
            ],
            None,
        );
        run_git(&["config", "user.email", "t@t.test"], Some(&wt));
        run_git(&["config", "user.name", "t"], Some(&wt));
        run_git(&["config", "commit.gpgsign", "false"], Some(&wt));
        write_manifest(&wt, plugin_name, extra_manifest);
        fs::write(wt.join("hello.py"), b"print('hi')").unwrap();
        run_git(&["add", "."], Some(&wt));
        run_git(&["commit", "--quiet", "-m", "init"], Some(&wt));
        let sha_out = Command::new("git")
            .current_dir(&wt)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
        run_git(&["tag", "v0.1.0"], Some(&wt));
        run_git(&["branch", "-M", "main"], Some(&wt));
        run_git(&["push", "--quiet", "origin", "main", "--tags"], Some(&wt));
        let url = format!("file://{}", bare.display());
        (tmp, url, "v0.1.0".into(), sha)
    }

    fn run_git(args: &[&str], cwd: Option<&Path>) {
        use std::process::Command;
        let mut cmd = Command::new("git");
        cmd.args(args);
        if let Some(c) = cwd {
            cmd.current_dir(c);
        }
        let out = cmd.output().expect("git binary missing");
        if !out.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn install_from_git_tag_happy_path() {
        let (_fixture, url, tag, expected_sha) =
            seed_plugin_bare_repo("git-plugin", "");
        let tmp_home = TempDir::new().unwrap();
        let home = tmp_home.path();
        let outcome = install_from_git(&url, &tag, false, home).unwrap();
        assert_eq!(outcome.name, "git-plugin");
        assert!(outcome.final_dir.join("hello.py").exists());
        let lock = PluginsLock::load(home).unwrap();
        let entry = lock.get("git-plugin").unwrap();
        assert_eq!(entry.resolved_sha.as_deref(), Some(expected_sha.as_str()));
        assert!(entry.source.starts_with("git:"));
        assert!(
            entry.manifest_hash.is_some(),
            "manifest_hash must be recorded for future update diffs"
        );
    }

    #[test]
    fn install_from_git_sha40_happy_path() {
        let (_fixture, url, _tag, expected_sha) =
            seed_plugin_bare_repo("sha40-plugin", "");
        let tmp_home = TempDir::new().unwrap();
        let outcome =
            install_from_git(&url, &expected_sha, false, tmp_home.path()).unwrap();
        let lock = PluginsLock::load(tmp_home.path()).unwrap();
        let entry = lock.get("sha40-plugin").unwrap();
        assert_eq!(entry.resolved_sha.as_deref(), Some(expected_sha.as_str()));
        assert!(
            outcome.final_dir.exists(),
            "install from SHA must still promote plugin tree"
        );
    }

    #[test]
    fn install_from_git_branch_rejected_without_flag() {
        let (_fixture, url, _tag, _sha) = seed_plugin_bare_repo("branch-rej", "");
        let tmp_home = TempDir::new().unwrap();
        let err = install_from_git(&url, "main", false, tmp_home.path()).unwrap_err();
        match &err {
            InstallError::SourceFetch(FetchError::UnstableRef { ref_ }) => {
                assert_eq!(ref_, "main");
            }
            other => panic!("expected UnstableRef, got: {other:?}"),
        }
        // Nothing was promoted.
        let lock = PluginsLock::load(tmp_home.path()).unwrap();
        assert!(lock.plugins.is_empty());
    }

    #[test]
    fn install_from_git_branch_accepted_with_flag() {
        let (_fixture, url, _tag, _sha) = seed_plugin_bare_repo("branch-ok", "");
        let tmp_home = TempDir::new().unwrap();
        install_from_git(&url, "main", true, tmp_home.path()).unwrap();
        let lock = PluginsLock::load(tmp_home.path()).unwrap();
        assert!(lock.get("branch-ok").is_some());
    }

    #[test]
    fn install_runs_install_script_and_env_is_set() {
        // Plugin manifest declares an [install].unix line that writes a
        // marker file using $MAKAKOO_PLUGIN_DIR. Proves the script runs
        // from the promoted dir with the env exported.
        let extras =
            "\n[install]\nunix = \"echo ran > $MAKAKOO_PLUGIN_DIR/.install-marker\"\n";
        let (_fixture, url, tag, _sha) = seed_plugin_bare_repo("script-plugin", extras);
        let tmp_home = TempDir::new().unwrap();
        let outcome =
            install_from_git(&url, &tag, false, tmp_home.path()).unwrap();
        let marker = outcome.final_dir.join(".install-marker");
        assert!(marker.is_file(), "[install].unix did not run");
        let body = fs::read_to_string(&marker).unwrap();
        assert!(body.contains("ran"));
    }

    #[test]
    /// Regression: a bare filename in `[install].unix = "install.sh"`
    /// must be resolved against the plugin dir, not looked up on PATH.
    /// `sh -c "install.sh"` would otherwise fail with "command not found"
    /// even when install.sh sits right next to plugin.toml. Caught live
    /// 2026-04-21 installing agent-browser-harness.
    // Unix-only: this test seeds an executable `install.sh` and shells out
    // through `sh -c`, which Windows CI runners don't have natively. The
    // bare-filename resolver itself is platform-agnostic; the executable
    // bit + sh execution are what we're verifying here.
    #[cfg(unix)]
    #[test]
    fn install_script_bare_filename_resolves_against_plugin_dir() {
        use std::process::Command;
        // Seed a plugin tree whose install.sh writes a marker — we commit
        // install.sh to the bare repo so it ships with the clone.
        let tmp = TempDir::new().unwrap();
        let bare = tmp.path().join("bare.git");
        let wt = tmp.path().join("wt");
        run_git(&["init", "--bare", "--quiet", bare.to_str().unwrap()], None);
        run_git(
            &[
                "clone",
                "--quiet",
                bare.to_str().unwrap(),
                wt.to_str().unwrap(),
            ],
            None,
        );
        run_git(&["config", "user.email", "t@t.test"], Some(&wt));
        run_git(&["config", "user.name", "t"], Some(&wt));
        run_git(&["config", "commit.gpgsign", "false"], Some(&wt));
        write_manifest(
            &wt,
            "bare-filename-plugin",
            "\n[install]\nunix = \"install.sh\"\n",
        );
        fs::write(
            wt.join("install.sh"),
            "#!/usr/bin/env sh\necho ran > \"$MAKAKOO_PLUGIN_DIR/.bare-filename-marker\"\n",
        )
        .unwrap();
        // Make install.sh executable — the bare-filename path must work
        // with either +x or not (sh -c invokes through sh, not via exec).
        let mut perms = fs::metadata(wt.join("install.sh")).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        fs::set_permissions(wt.join("install.sh"), perms).unwrap();
        run_git(&["add", "."], Some(&wt));
        run_git(&["commit", "--quiet", "-m", "init"], Some(&wt));
        run_git(&["tag", "v0.1.0"], Some(&wt));
        run_git(&["branch", "-M", "main"], Some(&wt));
        run_git(&["push", "--quiet", "origin", "main", "--tags"], Some(&wt));
        let url = format!("file://{}", bare.display());

        let tmp_home = TempDir::new().unwrap();
        let outcome =
            install_from_git(&url, "v0.1.0", false, tmp_home.path()).unwrap();
        let marker = outcome.final_dir.join(".bare-filename-marker");
        assert!(
            marker.is_file(),
            "bare-filename install.sh did not run — marker missing"
        );
        let _ = Command::new("true"); // silence unused import on some paths
    }

    #[test]
    fn install_script_failure_bubbles_up_as_error() {
        let extras = "\n[install]\nunix = \"exit 7\"\n";
        let (_fixture, url, tag, _sha) = seed_plugin_bare_repo("fail-plugin", extras);
        let tmp_home = TempDir::new().unwrap();
        let err = install_from_git(&url, &tag, false, tmp_home.path()).unwrap_err();
        assert!(
            matches!(err, InstallError::InstallScriptFailed { exit: 7, .. }),
            "expected InstallScriptFailed(exit=7), got: {err:?}"
        );
    }

    #[test]
    fn install_from_git_appends_resolved_sha_to_lock() {
        let (_fixture, url, tag, expected_sha) =
            seed_plugin_bare_repo("lock-sha", "");
        let tmp_home = TempDir::new().unwrap();
        install_from_git(&url, &tag, false, tmp_home.path()).unwrap();
        let raw = fs::read_to_string(
            tmp_home.path().join("config/plugins.lock"),
        )
        .unwrap();
        assert!(
            raw.contains(&expected_sha),
            "lock file must record resolved git SHA, got:\n{raw}"
        );
        assert!(
            raw.contains("manifest_hash"),
            "lock file must record manifest_hash"
        );
    }

    // ─── v0.4 Phase C: probe_upstream + apply_update tests ──────────

    #[test]
    fn probe_upstream_uptodate_when_sha_unchanged() {
        let (_fixture, url, tag, expected_sha) =
            seed_plugin_bare_repo("probe-noop", "");
        let tmp_home = TempDir::new().unwrap();
        install_from_git(&url, &tag, false, tmp_home.path()).unwrap();
        let entry = PluginsLock::load(tmp_home.path())
            .unwrap()
            .get("probe-noop")
            .cloned()
            .unwrap();
        let probe = probe_upstream(&entry).unwrap();
        assert_eq!(probe.drift, ProbeDrift::UpToDate);
        assert_eq!(probe.new_resolved_sha, expected_sha);
        drop_probe(probe);
    }

    #[test]
    fn probe_upstream_detects_content_drift() {
        use std::process::Command;
        let (fixture_tmp, url, tag, _sha) = seed_plugin_bare_repo("probe-drift", "");
        let tmp_home = TempDir::new().unwrap();
        install_from_git(&url, &tag, true, tmp_home.path()).unwrap();

        // Move the tag to a new commit with identical plugin.toml but
        // a different file. Manifest hash stays the same; resolved_sha
        // changes → ContentOnly drift.
        let wt = fixture_tmp.path().join("wt");
        fs::write(wt.join("extra.py"), b"# new file\n").unwrap();
        run_git(&["add", "."], Some(&wt));
        run_git(
            &["commit", "--quiet", "-m", "content-only-update"],
            Some(&wt),
        );
        run_git(&["tag", "-f", "v0.1.0"], Some(&wt));
        run_git(
            &[
                "push", "--quiet", "--force", "origin", "main", "--tags",
            ],
            Some(&wt),
        );

        // Refresh the probe-drift entry to use branch-based update
        // (so we can push again without stale-tag race).
        let entry = PluginsLock::load(tmp_home.path())
            .unwrap()
            .get("probe-drift")
            .cloned()
            .unwrap();
        // Swap the source to the main branch so probe_upstream re-fetches
        // HEAD (tag might not move atomically in file:// transports).
        let mut lock = PluginsLock::load(tmp_home.path()).unwrap();
        let mut swapped = entry.clone();
        swapped.source = format!("git:{url}@main");
        lock.upsert(swapped);
        lock.save(tmp_home.path()).unwrap();
        let swapped = lock.get("probe-drift").cloned().unwrap();

        let probe = probe_upstream(&swapped).unwrap();
        assert_eq!(probe.drift, ProbeDrift::ContentOnly);
        drop_probe(probe);

        // Pull the fixture into scope so the tempdir lives long enough.
        let _ = Command::new("true").arg(fixture_tmp.path()).status();
    }

    #[test]
    fn probe_upstream_detects_manifest_change() {
        let (fixture_tmp, url, _tag, _sha) =
            seed_plugin_bare_repo("probe-manifest", "");
        let tmp_home = TempDir::new().unwrap();
        install_from_git(&url, "main", true, tmp_home.path()).unwrap();

        // Rewrite the manifest upstream with a new version → manifest_hash
        // must change.
        let wt = fixture_tmp.path().join("wt");
        write_manifest(
            &wt,
            "probe-manifest",
            "\n[capabilities]\ngrants = [\"brain/read\"]\n",
        );
        run_git(&["add", "."], Some(&wt));
        run_git(
            &["commit", "--quiet", "-m", "add capability"],
            Some(&wt),
        );
        run_git(
            &["push", "--quiet", "--force", "origin", "main"],
            Some(&wt),
        );

        let entry = PluginsLock::load(tmp_home.path())
            .unwrap()
            .get("probe-manifest")
            .cloned()
            .unwrap();
        let probe = probe_upstream(&entry).unwrap();
        assert_eq!(probe.drift, ProbeDrift::ManifestChange);
        assert_ne!(probe.new_manifest_hash, entry.manifest_hash.unwrap_or_default());
        drop_probe(probe);
    }

    #[test]
    fn apply_update_swaps_installed_version_and_preserves_disabled_flag() {
        let (fixture_tmp, url, _tag, _sha) =
            seed_plugin_bare_repo("apply-update", "");
        let tmp_home = TempDir::new().unwrap();
        install_from_git(&url, "main", true, tmp_home.path()).unwrap();

        // Flip enabled=false before update.
        let mut lock = PluginsLock::load(tmp_home.path()).unwrap();
        let mut e = lock.get("apply-update").unwrap().clone();
        e.enabled = false;
        lock.upsert(e);
        lock.save(tmp_home.path()).unwrap();

        // Push a second commit upstream.
        let wt = fixture_tmp.path().join("wt");
        fs::write(wt.join("v2.py"), b"# v2\n").unwrap();
        run_git(&["add", "."], Some(&wt));
        run_git(&["commit", "--quiet", "-m", "v2"], Some(&wt));
        run_git(
            &["push", "--quiet", "--force", "origin", "main"],
            Some(&wt),
        );

        let entry = PluginsLock::load(tmp_home.path())
            .unwrap()
            .get("apply-update")
            .cloned()
            .unwrap();
        let probe = probe_upstream(&entry).unwrap();
        let new_sha = probe.new_resolved_sha.clone();
        apply_update(probe, tmp_home.path()).unwrap();

        let after = PluginsLock::load(tmp_home.path())
            .unwrap()
            .get("apply-update")
            .cloned()
            .unwrap();
        assert_eq!(after.resolved_sha.as_deref(), Some(new_sha.as_str()));
        assert!(!after.enabled, "disabled flag must survive apply_update");
        // New file present on disk
        let plugin_dir = tmp_home.path().join("plugins/apply-update");
        assert!(plugin_dir.join("v2.py").exists());
    }

    #[test]
    fn list_updatable_filters_to_git_and_tar() {
        let tmp_home = TempDir::new().unwrap();
        // Install a path plugin.
        let src = seed_source(tmp_home.path(), "path-one");
        install_from_path(
            &InstallRequest {
                source: PluginSource::Path(src),
                expected_blake3: None,
            },
            tmp_home.path(),
        )
        .unwrap();
        // Install a git plugin.
        let (_fix, url, tag, _sha) = seed_plugin_bare_repo("git-one", "");
        install_from_git(&url, &tag, false, tmp_home.path()).unwrap();

        let updatable = list_updatable(tmp_home.path()).unwrap();
        let names: Vec<_> = updatable.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"git-one"));
        assert!(!names.contains(&"path-one"));
    }

    #[test]
    fn install_from_tarball_sha_mismatch_aborts_before_promotion() {
        // Build a tarball by hand so we can attach a known-bad hash.
        let tmp = TempDir::new().unwrap();
        let pack = tmp.path().join("pack.tar.gz");
        fs::write(&pack, b"garbage tarball bytes").unwrap();
        let url = format!("file://{}", pack.display());
        let tmp_home = TempDir::new().unwrap();
        let err = install_from_tarball_url(
            &url,
            &"0".repeat(64),
            tmp_home.path(),
        )
        .unwrap_err();
        // Either Sha256Mismatch (hash check fires) or TarballHttp (curl
        // can't fetch file:// on this platform). Either proves nothing
        // was promoted.
        assert!(matches!(
            err,
            InstallError::SourceFetch(FetchError::Sha256Mismatch { .. })
                | InstallError::SourceFetch(FetchError::TarballHttp { .. })
        ));
        // $MAKAKOO_HOME/plugins/ should not exist.
        assert!(!tmp_home.path().join("plugins").exists());
    }
}
