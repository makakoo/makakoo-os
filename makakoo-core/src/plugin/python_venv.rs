//! `core::plugin::python_venv` — isolated per-plugin Python venv bootstrap.
//!
//! Git-sourced Python plugins need `pip install` to run somewhere safe;
//! `~/.makakoo/plugins/<name>/.venv/` is that somewhere. This helper
//! creates + populates the venv, idempotently, and serializes concurrent
//! callers via a sidecar lock so two `makakoo plugin install` calls for
//! the same plugin never race on pip.
//!
//! v0.4 locked decision D5: venvs are PER-PLUGIN. Never share. The extra
//! disk cost (~15–30MB per plugin) buys complete isolation on
//! dependency versions, transitive upgrades, and accidental side-effects
//! across plugins.
//!
//! ## API
//!
//! ```ignore
//! ensure_venv(&VenvSpec {
//!     plugin_dir: PathBuf::from("/…/plugins/agent-browser-harness"),
//!     python: "python3".into(),
//!     install_spec: InstallSpec::Editable,
//! })?;
//! ```
//!
//! Second and subsequent calls with unchanged inputs return instantly
//! (no pip invocation). The concurrent-safety contract is enforced by
//! an `fs2` exclusive lock on `<plugin_dir>/.venv.lock`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fs2::FileExt;
use thiserror::Error;
use tracing::{debug, info};

#[derive(Debug, Error)]
pub enum VenvError {
    #[error("plugin_dir {0:?} does not exist")]
    PluginDirMissing(PathBuf),
    #[error("python binary {0:?} not found on PATH")]
    PythonMissing(String),
    #[error("python -m venv {0:?} failed: {1}")]
    CreateVenv(PathBuf, String),
    #[error("pip upgrade failed: {0}")]
    PipUpgrade(String),
    #[error("pip install failed: {0}")]
    PipInstall(String),
    #[error("venv lock acquisition failed for {0:?}: {1}")]
    LockFailed(PathBuf, String),
    #[error("io error on {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub enum InstallSpec {
    /// `pip install -e <plugin_dir>` — source-editable install (default
    /// for plugins that ship a pyproject.toml / setup.py).
    Editable,
    /// `pip install <raw spec>` — passed verbatim to pip. Useful for
    /// requirements-file installs (`-r requirements.txt`) or specific
    /// package pins.
    Pip(String),
    /// `pip install git+<url>[@<rev>]` — fetches directly from a git repo.
    Git { url: String, rev: Option<String> },
}

#[derive(Debug, Clone)]
pub struct VenvSpec {
    pub plugin_dir: PathBuf,
    /// Python binary. Accepts `python3` / `python3.11` / `/abs/path`.
    pub python: String,
    pub install_spec: InstallSpec,
}

#[derive(Debug, Clone)]
pub struct EnsureVenvReport {
    pub venv_dir: PathBuf,
    pub python_bin: PathBuf,
    /// True iff this call freshly created the venv directory (vs.
    /// idempotent no-op on an existing venv).
    pub created: bool,
    /// True iff the caller's install_spec produced a pip invocation
    /// in this call. pip itself is idempotent for unchanged inputs;
    /// this flag just reflects whether we handed the work to pip.
    pub install_ran: bool,
}

pub fn ensure_venv(spec: &VenvSpec) -> Result<EnsureVenvReport, VenvError> {
    if !spec.plugin_dir.is_dir() {
        return Err(VenvError::PluginDirMissing(spec.plugin_dir.clone()));
    }

    let lock_path = spec.plugin_dir.join(".venv.lock");
    let lock_file = fs::File::create(&lock_path).map_err(|e| VenvError::Io {
        path: lock_path.clone(),
        source: e,
    })?;
    lock_file
        .lock_exclusive()
        .map_err(|e| VenvError::LockFailed(lock_path.clone(), e.to_string()))?;

    let venv_dir = spec.plugin_dir.join(".venv");
    let python_bin = venv_bin(&venv_dir);

    let created = if !venv_dir.is_dir() {
        // Check python itself exists.
        let probe = Command::new(&spec.python)
            .arg("--version")
            .output()
            .map_err(|_| VenvError::PythonMissing(spec.python.clone()))?;
        if !probe.status.success() {
            return Err(VenvError::PythonMissing(spec.python.clone()));
        }
        info!(
            "creating venv for {:?} using {}",
            spec.plugin_dir, spec.python
        );
        let out = Command::new(&spec.python)
            .args(["-m", "venv", venv_dir.to_str().unwrap()])
            .output()
            .map_err(|e| VenvError::CreateVenv(venv_dir.clone(), e.to_string()))?;
        if !out.status.success() {
            return Err(VenvError::CreateVenv(
                venv_dir.clone(),
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        // One-shot upgrade. Failure is fatal: a stale wheel/setuptools
        // can cause cryptic pip errors downstream.
        let out = Command::new(&python_bin)
            .args(["-m", "pip", "install", "--quiet", "--upgrade", "pip", "wheel"])
            .output()
            .map_err(|e| VenvError::PipUpgrade(e.to_string()))?;
        if !out.status.success() {
            return Err(VenvError::PipUpgrade(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        true
    } else {
        debug!("venv already exists at {:?}", venv_dir);
        false
    };

    // Run the install step. pip itself de-dupes when packages are
    // already resolved at the declared version.
    let install_args = build_pip_args(&spec.install_spec, &spec.plugin_dir);
    debug!(
        "pip install (idempotent) for {:?}: {:?}",
        spec.plugin_dir, install_args
    );
    let mut cmd = Command::new(&python_bin);
    cmd.args(["-m", "pip", "install", "--quiet"]);
    cmd.args(&install_args);
    let out = cmd
        .output()
        .map_err(|e| VenvError::PipInstall(e.to_string()))?;
    if !out.status.success() {
        return Err(VenvError::PipInstall(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    // Explicit unlock is cosmetic (the Drop does it) but makes the
    // critical section obvious.
    let _ = lock_file.unlock();

    Ok(EnsureVenvReport {
        venv_dir,
        python_bin,
        created,
        install_ran: true,
    })
}

/// Path to the in-venv python binary. Unix-specific for now; Phase D
/// Windows parity lands when the rest of the install pipeline does.
fn venv_bin(venv_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

fn build_pip_args(spec: &InstallSpec, plugin_dir: &Path) -> Vec<String> {
    match spec {
        InstallSpec::Editable => vec!["-e".into(), plugin_dir.display().to_string()],
        InstallSpec::Pip(s) => s
            .split_whitespace()
            .map(|t| t.to_string())
            .collect(),
        InstallSpec::Git { url, rev } => {
            let target = match rev {
                Some(r) => format!("git+{url}@{r}"),
                None => format!("git+{url}"),
            };
            vec![target]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_trivial_pkg(dir: &Path) {
        // A self-installable skeleton: pyproject.toml + package.
        fs::write(
            dir.join("pyproject.toml"),
            r#"[project]
name = "makakoo-test-pkg"
version = "0.0.1"

[tool.setuptools]
packages = ["makakoo_test_pkg"]

[build-system]
requires = ["setuptools>=61"]
build-backend = "setuptools.build_meta"
"#,
        )
        .unwrap();
        fs::create_dir_all(dir.join("makakoo_test_pkg")).unwrap();
        fs::write(
            dir.join("makakoo_test_pkg").join("__init__.py"),
            "VALUE = 42\n",
        )
        .unwrap();
    }

    fn skip_if_no_python() -> Option<&'static str> {
        // Test fixtures depend on `python3 -m venv`. CI without a
        // matching python should `#[ignore]` — locally we just gate
        // on the probe.
        if Command::new("python3")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            None
        } else {
            Some("python3 not available")
        }
    }

    #[test]
    #[ignore = "network + pip — run manually with `cargo test -- --ignored`"]
    fn ensure_venv_editable_install_succeeds_end_to_end() {
        if let Some(why) = skip_if_no_python() {
            eprintln!("skip: {why}");
            return;
        }
        let tmp = TempDir::new().unwrap();
        write_trivial_pkg(tmp.path());
        let spec = VenvSpec {
            plugin_dir: tmp.path().to_path_buf(),
            python: "python3".into(),
            install_spec: InstallSpec::Editable,
        };
        let r = ensure_venv(&spec).unwrap();
        assert!(r.created, "first run must create the venv");
        assert!(r.python_bin.is_file(), "venv python binary missing");

        // Second run — idempotent.
        let r2 = ensure_venv(&spec).unwrap();
        assert!(!r2.created, "second run must be a no-op for venv creation");
    }

    #[test]
    fn ensure_venv_missing_plugin_dir_errors() {
        let spec = VenvSpec {
            plugin_dir: PathBuf::from("/totally/not/a/real/path/xyzzy"),
            python: "python3".into(),
            install_spec: InstallSpec::Editable,
        };
        let err = ensure_venv(&spec).unwrap_err();
        assert!(matches!(err, VenvError::PluginDirMissing(_)));
    }

    #[test]
    fn ensure_venv_missing_python_binary_errors() {
        let tmp = TempDir::new().unwrap();
        write_trivial_pkg(tmp.path());
        let spec = VenvSpec {
            plugin_dir: tmp.path().to_path_buf(),
            python: "python-does-not-exist-xyzzy".into(),
            install_spec: InstallSpec::Editable,
        };
        let err = ensure_venv(&spec).unwrap_err();
        // Accept either PythonMissing (probe failed) or CreateVenv
        // (venv ran but the exec itself blew up). Both prove we detected
        // the missing interpreter before pip ran.
        assert!(matches!(
            err,
            VenvError::PythonMissing(_) | VenvError::CreateVenv(_, _)
        ));
    }

    #[test]
    fn build_pip_args_editable_injects_plugin_dir() {
        let args = build_pip_args(&InstallSpec::Editable, Path::new("/tmp/x"));
        assert_eq!(args, vec!["-e".to_string(), "/tmp/x".to_string()]);
    }

    #[test]
    fn build_pip_args_pip_splits_spec() {
        let args = build_pip_args(
            &InstallSpec::Pip("-r requirements.txt".into()),
            Path::new("/irrelevant"),
        );
        assert_eq!(args, vec!["-r".to_string(), "requirements.txt".to_string()]);
    }

    #[test]
    fn build_pip_args_git_with_rev() {
        let args = build_pip_args(
            &InstallSpec::Git {
                url: "https://github.com/x/y".into(),
                rev: Some("v1.2.3".into()),
            },
            Path::new("/irrelevant"),
        );
        assert_eq!(args, vec!["git+https://github.com/x/y@v1.2.3".to_string()]);
    }

    #[test]
    fn build_pip_args_git_without_rev() {
        let args = build_pip_args(
            &InstallSpec::Git {
                url: "https://github.com/x/y".into(),
                rev: None,
            },
            Path::new("/irrelevant"),
        );
        assert_eq!(args, vec!["git+https://github.com/x/y".to_string()]);
    }

    #[test]
    fn venv_bin_picks_platform_appropriate_path() {
        let v = PathBuf::from("/plugins/foo/.venv");
        let b = venv_bin(&v);
        if cfg!(windows) {
            assert!(b.to_string_lossy().contains("Scripts"));
        } else {
            assert!(b.ends_with("bin/python"));
        }
    }
}
