//! `makakoo octopus` — thin passthrough to the Python bootstrap wizard.
//!
//! Every subcommand (`bootstrap`, `invite`, `join`, `trust list/revoke`,
//! `doctor`) is implemented in `core.octopus.bootstrap_wizard` so the
//! Python-side modules that already own the trust store and onboarding
//! tokens stay the single source of truth. Keeping the Rust side a
//! trailing-arg passthrough avoids duplicating clap definitions on both
//! sides and keeps the wire protocol with pods / SME teammates
//! language-agnostic.
//!
//! PYTHONPATH is set to the installed `lib-harvey-core/src/` so
//! `python3 -m core.octopus.bootstrap_wizard` resolves regardless of the
//! caller's cwd.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

use makakoo_core::platform::makakoo_home;

/// Run the Octopus wizard with `args` (e.g. `["bootstrap", "--force"]`).
/// Returns the child process's exit code.
pub fn run(args: Vec<String>) -> Result<i32> {
    let src_dir = resolve_lib_harvey_core_src()?;
    let python = python_binary();

    let mut cmd = Command::new(&python);
    cmd.arg("-m").arg("core.octopus.bootstrap_wizard");
    cmd.args(&args);

    // Prepend our src dir to PYTHONPATH so the caller's existing
    // PYTHONPATH (if any) stays intact.
    let prev_pythonpath: OsString = std::env::var_os("PYTHONPATH").unwrap_or_default();
    let new_pythonpath = prepend_pythonpath(&src_dir, &prev_pythonpath);
    cmd.env("PYTHONPATH", new_pythonpath);

    // Forward MAKAKOO_HOME explicitly. The Python wizard reads this env
    // var at module import to resolve every on-disk path; in test mode
    // (env pointing at a tmpdir) we want the subprocess to see the same.
    cmd.env("MAKAKOO_HOME", makakoo_home());

    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn {} -m core.octopus.bootstrap_wizard", python))?;
    Ok(status.code().unwrap_or(1))
}

/// Locate `plugins-core/lib-harvey-core/src/` on disk.
///
/// Resolution order:
///   1. `$MAKAKOO_PLUGINS_DIR/lib-harvey-core/src/` — explicit override.
///   2. `$MAKAKOO_HOME/plugins/lib-harvey-core/src/` — standard install
///      layout (what `makakoo plugin install --core lib-harvey-core`
///      produces).
///   3. Source-tree fallback: walk up from the current exe looking for
///      `plugins-core/lib-harvey-core/src/`. Handles `cargo run` + the
///      CI test harness + local `cargo install --path makakoo` without
///      requiring the plugin to be installed via the plugin system.
///
/// Exits with a clear error if none of these exist — better to fail
/// loudly than to hand Python a nonexistent PYTHONPATH and let the
/// `ModuleNotFoundError` propagate as an opaque CalledProcessError.
fn resolve_lib_harvey_core_src() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("MAKAKOO_PLUGINS_DIR") {
        let candidate = PathBuf::from(dir).join("lib-harvey-core").join("src");
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    let home_plugins = makakoo_home().join("plugins").join("lib-harvey-core").join("src");
    if home_plugins.is_dir() {
        return Ok(home_plugins);
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut cur: Option<&std::path::Path> = Some(exe.as_path());
        while let Some(p) = cur {
            let candidate = p
                .join("plugins-core")
                .join("lib-harvey-core")
                .join("src");
            if candidate.is_dir() {
                return Ok(candidate);
            }
            cur = p.parent();
        }
    }
    anyhow::bail!(
        "cannot locate lib-harvey-core/src/.\n\
         Install with: makakoo plugin install --core lib-harvey-core\n\
         Or set MAKAKOO_PLUGINS_DIR to a source-tree plugins-core/ dir."
    )
}

#[cfg(not(windows))]
fn python_binary() -> String {
    std::env::var("MAKAKOO_PYTHON").unwrap_or_else(|_| "python3".to_string())
}

#[cfg(windows)]
fn python_binary() -> String {
    std::env::var("MAKAKOO_PYTHON").unwrap_or_else(|_| "python".to_string())
}

fn prepend_pythonpath(src_dir: &std::path::Path, prev: &OsString) -> OsString {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut joined = OsString::new();
    joined.push(src_dir.as_os_str());
    if !prev.is_empty() {
        joined.push(sep);
        joined.push(prev);
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn prepend_joins_with_correct_separator() {
        let p = Path::new("/tmp/src");
        let prev = OsString::from("/other");
        let out = prepend_pythonpath(p, &prev);
        let out_str = out.to_string_lossy();
        // On Unix the joined result must contain both, separated by ":".
        #[cfg(not(windows))]
        assert_eq!(out_str, "/tmp/src:/other");
    }

    #[test]
    fn prepend_empty_prev() {
        let p = Path::new("/tmp/src");
        let prev = OsString::new();
        let out = prepend_pythonpath(p, &prev);
        assert_eq!(out.to_string_lossy(), "/tmp/src");
    }
}
