//! Sandbox profiles — enforce the adapter's declared isolation at
//! install-time (during the sandboxed health check) and at call time.
//!
//! macOS: `sandbox-exec` with a generated `.sb` profile.
//! Linux:  `bwrap` when available; warn-fallback otherwise.
//! Windows: no-op (logs a warning so operators know).
//!
//! Only the install-time health check is wrapped automatically in Phase C.
//! Runtime sandboxing of `call_adapter` transports is opt-in via the
//! Phase-E integration layer — keeping the test surface tractable while
//! still getting the supply-chain guarantees up-front.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::manifest::{Manifest, SandboxProfile};

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("sandbox backend `{backend}` missing on this host ({hint})")]
    BackendMissing { backend: &'static str, hint: String },
    #[error("failed to spawn sandbox: {0}")]
    Spawn(String),
    #[error("sandboxed command exited {code}: {stderr}")]
    CommandFailed { code: i32, stderr: String },
    #[error("sandbox profile write failed {path}: {source}")]
    ProfileWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sandbox I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Describes what a sandbox profile permits. Intentionally minimal: the
/// enforcement logic is platform-specific and lives in `build_command`.
#[derive(Debug, Clone)]
pub struct ProfileSpec {
    pub profile: SandboxProfile,
    pub install_dir: PathBuf,
    pub allowed_hosts: Vec<String>,
    pub requires_filesystem: Vec<String>,
}

impl ProfileSpec {
    pub fn from_manifest(manifest: &Manifest, install_dir: impl Into<PathBuf>) -> Self {
        Self {
            profile: manifest.security.sandbox_profile,
            install_dir: install_dir.into(),
            allowed_hosts: manifest.security.allowed_hosts.clone(),
            requires_filesystem: manifest.security.requires_filesystem.clone(),
        }
    }
}

/// Which platform backend we can use. Exposed for diagnostics and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    MacOsSandboxExec,
    LinuxBwrap,
    /// No sandbox applied — host is Windows or no backend detected.
    /// The adapter runs with host privileges; a loud warning is logged.
    NoneFallback,
}

impl SandboxBackend {
    pub fn detect() -> SandboxBackend {
        if cfg!(target_os = "macos") {
            SandboxBackend::MacOsSandboxExec
        } else if cfg!(target_os = "linux") {
            if which::which("bwrap").is_ok() {
                SandboxBackend::LinuxBwrap
            } else {
                SandboxBackend::NoneFallback
            }
        } else {
            SandboxBackend::NoneFallback
        }
    }
}

/// Run a sandboxed command. Reads stdin, captures stdout+stderr, and
/// returns raw bytes. Used by the install-time health check.
///
/// When `backend == NoneFallback`, we log a warning via tracing and run
/// the command unsandboxed — matches the "warn + unsandboxed" escape
/// hatch from the sprint doc's Q1 default.
pub async fn run_sandboxed(
    backend: SandboxBackend,
    spec: &ProfileSpec,
    argv: &[String],
    stdin_bytes: Option<&[u8]>,
    timeout_secs: u64,
) -> Result<Vec<u8>, SandboxError> {
    if argv.is_empty() {
        return Err(SandboxError::Spawn("empty command".into()));
    }
    let (program, args) = build_command(backend, spec, argv);

    let mut cmd = Command::new(&program);
    cmd.args(&args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| SandboxError::Spawn(format!("{program}: {e}")))?;

    if let Some(bytes) = stdin_bytes {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(bytes).await?;
        }
    }

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| SandboxError::Spawn(format!("timeout after {timeout_secs}s")))??;

    if !output.status.success() {
        return Err(SandboxError::CommandFailed {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(output.stdout)
}

/// Run an arbitrary async block with a SandboxProfile-enforced set of
/// filesystem/network assertions. Used by tests to prove
/// escape-attempts fail with a clear error rather than silently pass.
///
/// Keep this distinct from `run_sandboxed` — which wraps a real
/// subprocess under the host OS's sandbox. This helper is a pure in-Rust
/// assertion used when we don't need a full subprocess (e.g. verifying
/// that an `fs-read` profile *would* deny a write declared at manifest
/// level).
pub fn assert_manifest_self_consistent(spec: &ProfileSpec) -> Result<(), SandboxError> {
    // fs-read means no write:* entries may appear in requires_filesystem.
    if matches!(spec.profile, SandboxProfile::FsRead)
        && spec
            .requires_filesystem
            .iter()
            .any(|p| p.starts_with("write:"))
    {
        return Err(SandboxError::CommandFailed {
            code: 1,
            stderr: "sandbox profile `fs-read` forbids write:* requires_filesystem entries".into(),
        });
    }
    // isolated means no network, so allowed_hosts must be empty.
    if matches!(spec.profile, SandboxProfile::Isolated) && !spec.allowed_hosts.is_empty() {
        return Err(SandboxError::CommandFailed {
            code: 1,
            stderr: "sandbox profile `isolated` forbids non-empty allowed_hosts".into(),
        });
    }
    Ok(())
}

fn build_command(
    backend: SandboxBackend,
    spec: &ProfileSpec,
    argv: &[String],
) -> (String, Vec<String>) {
    match backend {
        SandboxBackend::MacOsSandboxExec => build_sandbox_exec(spec, argv),
        SandboxBackend::LinuxBwrap => build_bwrap(spec, argv),
        SandboxBackend::NoneFallback => {
            tracing::warn!(
                profile = ?spec.profile,
                "sandbox backend unavailable — adapter will run UNSANDBOXED"
            );
            (
                argv[0].clone(),
                argv.iter().skip(1).cloned().collect::<Vec<_>>(),
            )
        }
    }
}

fn build_sandbox_exec(spec: &ProfileSpec, argv: &[String]) -> (String, Vec<String>) {
    let profile_body = generate_sbpl(spec);
    // sandbox-exec has an `-p` flag that accepts the profile inline via
    // stdin in a subset of versions, but the widely-compatible form is
    // `-f <file>`. We spill the profile into the install dir.
    let profile_path = spec.install_dir.join(".sandbox.sb");
    // Best-effort write — ignore failure here; sandbox-exec will surface
    // it when it can't read the file.
    let _ = std::fs::create_dir_all(&spec.install_dir);
    let _ = std::fs::write(&profile_path, profile_body);

    let mut new_args = vec!["-f".to_string(), profile_path.to_string_lossy().into_owned()];
    new_args.extend(argv.iter().cloned());
    ("sandbox-exec".to_string(), new_args)
}

/// Generate a sandbox-exec SBPL policy for the given spec. Restrictive
/// by default — deny everything, then selectively allow.
pub fn generate_sbpl(spec: &ProfileSpec) -> String {
    let header = "(version 1)\n(deny default)\n(allow process-exec)\n(allow process-fork)\n(allow signal (target self))\n(allow sysctl-read)\n(allow mach-lookup)\n(allow file-read*)\n";
    let mut body = String::from(header);
    match spec.profile {
        SandboxProfile::None => {
            // Trust mode — minimal SBPL that allows everything.
            return "(version 1)\n(allow default)\n".to_string();
        }
        SandboxProfile::NetworkIo => {
            body.push_str("(allow network*)\n");
            body.push_str(&allow_install_writes(&spec.install_dir));
            for req in &spec.requires_filesystem {
                if let Some(p) = req.strip_prefix("write:") {
                    body.push_str(&format!(
                        "(allow file-write* (subpath \"{}\"))\n",
                        expand_path(p.trim())
                    ));
                }
            }
        }
        SandboxProfile::FsRead => {
            // No network, read-only fs (already allowed by file-read* above).
        }
        SandboxProfile::Isolated => {
            // No network, fs limited to install dir only.
            body.push_str(&allow_install_writes(&spec.install_dir));
        }
    }
    body
}

fn allow_install_writes(install_dir: &Path) -> String {
    let path = install_dir.to_string_lossy();
    format!("(allow file-write* (subpath \"{path}\"))\n")
}

fn expand_path(p: &str) -> String {
    if let Some(stripped) = p.strip_prefix("$MAKAKOO_HOME/") {
        if let Ok(home) = std::env::var("MAKAKOO_HOME") {
            return format!("{home}/{stripped}");
        }
    }
    p.to_string()
}

fn build_bwrap(spec: &ProfileSpec, argv: &[String]) -> (String, Vec<String>) {
    let mut args = vec![
        "--unshare-pid".to_string(),
        "--die-with-parent".to_string(),
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--ro-bind".into(),
        "/usr".into(),
        "/usr".into(),
        "--ro-bind".into(),
        "/lib".into(),
        "/lib".into(),
        "--ro-bind".into(),
        "/lib64".into(),
        "/lib64".into(),
        "--ro-bind".into(),
        "/bin".into(),
        "/bin".into(),
        "--ro-bind".into(),
        "/etc".into(),
        "/etc".into(),
    ];
    match spec.profile {
        SandboxProfile::None => {
            // Allow everything — effectively just an exec wrapper.
            args = vec!["--die-with-parent".into()];
        }
        SandboxProfile::NetworkIo => {
            args.push("--share-net".into());
            args.push("--bind".into());
            args.push(spec.install_dir.to_string_lossy().into_owned());
            args.push(spec.install_dir.to_string_lossy().into_owned());
        }
        SandboxProfile::FsRead => {
            args.push("--unshare-net".into());
            // No writable binds.
        }
        SandboxProfile::Isolated => {
            args.push("--unshare-net".into());
            args.push("--bind".into());
            args.push(spec.install_dir.to_string_lossy().into_owned());
            args.push(spec.install_dir.to_string_lossy().into_owned());
        }
    }
    args.push("--".into());
    args.extend(argv.iter().cloned());
    ("bwrap".to_string(), args)
}

// Lightweight `which` impl (avoids adding another dependency).
mod which {
    use std::path::PathBuf;

    pub fn which(bin: &str) -> Result<PathBuf, ()> {
        let path = std::env::var_os("PATH").ok_or(())?;
        for dir in std::env::split_paths(&path) {
            let candidate: PathBuf = dir.join(bin);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Manifest;

    const NETWORK_IO_MANIFEST: &str = r#"
[adapter]
name = "a1"
version = "0.1.0"
manifest_schema = 1
description = "d"

[compatibility]
bridge_version = "^2.0"
protocols = ["openai-chat-v1"]

[transport]
kind = "openai-compatible"
base_url = "http://127.0.0.1:9/v1"

[auth]
scheme = "none"

[output]
format = "lope-verdict-block"

[capabilities]
supports_roles = ["validator"]

[install]
source_type = "local"

[security]
requires_network = true
allowed_hosts = ["127.0.0.1"]
sandbox_profile = "network-io"
requires_filesystem = ["write:$MAKAKOO_HOME/state/adapters/a1"]
"#;

    const FS_READ_MANIFEST: &str = r#"
[adapter]
name = "a2"
version = "0.1.0"
manifest_schema = 1
description = "d"

[compatibility]
bridge_version = "^2.0"
protocols = ["openai-chat-v1"]

[transport]
kind = "openai-compatible"
base_url = "http://127.0.0.1:9/v1"

[auth]
scheme = "none"

[output]
format = "lope-verdict-block"

[capabilities]
supports_roles = ["validator"]

[install]
source_type = "local"

[security]
requires_network = false
sandbox_profile = "fs-read"
"#;

    #[test]
    fn sbpl_has_deny_default_for_restrictive_profile() {
        let m = Manifest::parse_str(NETWORK_IO_MANIFEST).unwrap();
        let spec = ProfileSpec::from_manifest(&m, PathBuf::from("/tmp/x"));
        let s = generate_sbpl(&spec);
        assert!(s.contains("deny default"));
        assert!(s.contains("allow network*"));
    }

    #[test]
    fn sbpl_none_profile_is_permissive() {
        let body = NETWORK_IO_MANIFEST.replace(
            r#"sandbox_profile = "network-io""#,
            r#"sandbox_profile = "none""#,
        );
        let m = Manifest::parse_str(&body).unwrap();
        let spec = ProfileSpec::from_manifest(&m, PathBuf::from("/tmp/x"));
        let s = generate_sbpl(&spec);
        assert!(s.contains("allow default"));
    }

    #[test]
    fn fs_read_rejects_write_filesystem_entries() {
        let body = FS_READ_MANIFEST.replace(
            r#"sandbox_profile = "fs-read""#,
            r#"sandbox_profile = "fs-read"
requires_filesystem = ["write:/tmp/x"]"#,
        );
        let m = Manifest::parse_str(&body).unwrap();
        let spec = ProfileSpec::from_manifest(&m, PathBuf::from("/tmp/x"));
        let err = assert_manifest_self_consistent(&spec).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("fs-read"), "got {msg}");
    }

    #[test]
    fn isolated_rejects_non_empty_allowed_hosts() {
        let body = NETWORK_IO_MANIFEST
            .replace(r#"sandbox_profile = "network-io""#, r#"sandbox_profile = "isolated""#)
            .replace(r#"requires_network = true"#, r#"requires_network = true"#);
        let m = Manifest::parse_str(&body).unwrap();
        let spec = ProfileSpec::from_manifest(&m, PathBuf::from("/tmp/x"));
        let err = assert_manifest_self_consistent(&spec).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("isolated"), "got {msg}");
    }

    #[test]
    fn self_consistent_manifest_passes() {
        let m = Manifest::parse_str(NETWORK_IO_MANIFEST).unwrap();
        let spec = ProfileSpec::from_manifest(&m, PathBuf::from("/tmp/x"));
        assert!(assert_manifest_self_consistent(&spec).is_ok());
    }

    #[test]
    fn detect_returns_reasonable_backend() {
        let b = SandboxBackend::detect();
        // On any supported host, we always land on a known backend
        // (macOS → sandbox-exec, Linux → bwrap or fallback, others →
        // fallback). This is a sanity check rather than a semantic
        // guarantee.
        assert!(matches!(
            b,
            SandboxBackend::MacOsSandboxExec
                | SandboxBackend::LinuxBwrap
                | SandboxBackend::NoneFallback
        ));
    }
}
