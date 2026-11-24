//! Shared helpers for resolving sibling workspace binaries in `makakoo`
//! integration tests.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Format a binary name with the current platform's executable suffix
/// (`.exe` on Windows, empty on Unix).
#[allow(dead_code)]
pub fn exe_name(bin_name: &str) -> String {
    format!("{bin_name}{}", std::env::consts::EXE_SUFFIX)
}

/// Resolve a sibling workspace binary in the exact profile output directory
/// that contains `anchor` (for example `env!("CARGO_BIN_EXE_makakoo")`).
///
/// Only checks `anchor.parent()` — never walks ancestor directories or
/// `current_exe()`, so an unrelated parent directory named `build` or `deps`
/// cannot hijack binary lookup.
#[allow(dead_code)]
pub fn sibling_bin_from_anchor(anchor: &Path, bin_name: &str) -> Option<PathBuf> {
    let profile_dir = anchor.parent()?;
    let candidate = profile_dir.join(exe_name(bin_name));
    candidate.is_file().then_some(candidate)
}

/// Map a Cargo profile output directory (e.g. `target/debug`, `target/release`,
/// or `target/<custom>`) to the `--profile` argument accepted by `cargo build`.
#[allow(dead_code)]
pub fn cargo_profile_for_dir(profile_dir: &Path) -> Option<&str> {
    let name = profile_dir.file_name()?.to_str()?;
    match name {
        "debug" => Some("dev"),
        other if !other.is_empty() => Some(other),
        _ => None,
    }
}

/// Return true when running in a CI environment (`CI` set to a truthy value).
#[allow(dead_code)]
pub fn is_ci_env() -> bool {
    std::env::var_os("CI").is_some_and(|v| !v.is_empty() && v != "0" && v != "false")
}

/// Locate a sibling workspace binary next to `makakoo_anchor`.
///
/// `PATH` fallback is disabled unless `allow_path_fallback` is true AND
/// `is_ci` is false. Returns `(path, used_path_fallback)` on success.
#[allow(dead_code)]
pub fn resolve_sibling_bin_with_policy(
    makakoo_anchor: &Path,
    bin_name: &str,
    allow_path_fallback: bool,
    is_ci: bool,
    path_env: Option<&OsStr>,
) -> Result<(PathBuf, bool), String> {
    if let Some(bin) = sibling_bin_from_anchor(makakoo_anchor, bin_name) {
        return Ok((bin, false));
    }
    let filename = exe_name(bin_name);
    if is_ci {
        return Err(format!(
            "{filename} not found next to {} (CI disables PATH fallback; build the workspace first)",
            makakoo_anchor.display()
        ));
    }
    if allow_path_fallback {
        if let Some(path) = path_env {
            if let Some(bin) = std::env::split_paths(path)
                .map(|dir| dir.join(&filename))
                .find(|p| p.is_file())
            {
                return Ok((bin, true));
            }
        }
        return Err(format!(
            "{filename} not found next to {} or on PATH",
            makakoo_anchor.display()
        ));
    }
    Err(format!(
        "{filename} not found next to {} — run `cargo build --workspace` (or set MAKAKOO_E2E_USE_PATH=1 outside CI to allow PATH lookup)",
        makakoo_anchor.display()
    ))
}

/// Locate the sibling `makakoo-mcp` binary next to `CARGO_BIN_EXE_makakoo`.
///
/// Reuses the workspace binary when already present (fast path under
/// `cargo test --workspace` / CI after `cargo build --workspace`). On a cold
/// start where `makakoo-mcp` has not been built yet, builds it once per test
/// binary via `OnceLock` using the active Cargo profile derived from
/// `CARGO_BIN_EXE_makakoo`'s parent directory (so `--release` and custom
/// profiles work too).
#[allow(dead_code)]
pub fn ensure_makakoo_mcp_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let makakoo = PathBuf::from(env!("CARGO_BIN_EXE_makakoo"));
        if let Some(bin) = sibling_bin_from_anchor(&makakoo, "makakoo-mcp") {
            return bin;
        }
        let profile_dir = makakoo
            .parent()
            .expect("CARGO_BIN_EXE_makakoo must have a parent profile directory");
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .expect("makakoo manifest must live inside the workspace root");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

        let mut cmd = Command::new(cargo);
        cmd.current_dir(workspace_root)
            .args(["build", "--bin", "makakoo-mcp"]);
        if let Some(profile) = cargo_profile_for_dir(profile_dir) {
            cmd.args(["--profile", profile]);
        }
        let status = cmd
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .expect("failed to invoke cargo build for makakoo-mcp");
        assert!(status.success(), "cargo build --bin makakoo-mcp failed");

        sibling_bin_from_anchor(&makakoo, "makakoo-mcp")
            .expect("makakoo-mcp binary present next to makakoo after cargo build")
    })
    .clone()
}
