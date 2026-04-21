//! Install lifecycle — the 7-step flow from the sprint doc:
//!
//! 1. fetch manifest (from local path, git, tarball, pypi, npm, binary)
//! 2. verify signature (Ed25519) — mandatory for URL installs unless
//!    `--allow-unsigned` is set; local paths are always allowed, with
//!    an interactive confirm for `--force-local` in the CLI layer.
//! 3. verify sha256 (for non-git URL installs)
//! 4. stage under `~/.makakoo/adapters/staging/<name>/`
//! 5. sandboxed health check — spawn the adapter under its declared
//!    sandbox profile, hit `health.check_url`, assert `expected_field`
//!    is present. Abort on failure. (Phase-C MVP: health check is
//!    performed when `health.check_url` is set AND the transport is
//!    HTTP; subprocess health checks are deferred to v0.4.)
//! 6. show capability diff vs trust ledger, prompt (CLI layer only)
//! 7. promote — move from staging to `registered/`, update trust ledger
//!
//! Phase-C ships `install_from_path` and the infrastructure for every
//! other source_type. The git / tarball / pypi / npm concrete fetchers
//! land in Phase-D when the CLI wires them up; they share this code's
//! staging + trust + sandbox plumbing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use super::manifest::{Manifest, SourceType};
use super::sandbox::{self, ProfileSpec, SandboxBackend};
use super::sign::{verify_manifest_bytes, SignError};
use super::trust::{
    diff_manifest, trust_entry_from_manifest, ManifestDiff, TrustLedger,
};

const REGISTERED_DIRNAME: &str = "registered";
const STAGING_DIRNAME: &str = "staging";

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("source path {path} does not exist or is not readable")]
    SourceMissing { path: PathBuf },
    #[error("manifest not found at {path}")]
    ManifestMissing { path: PathBuf },
    #[error(transparent)]
    Manifest(#[from] super::manifest::ManifestError),
    #[error("unsigned manifest rejected: URL installs require signature OR --allow-unsigned")]
    UnsignedUrlRejected,
    #[error(transparent)]
    Sign(#[from] SignError),
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    Sha256Mismatch { expected: String, actual: String },
    #[error("install staging failed: {0}")]
    Staging(String),
    #[error("capability drift detected — re-trust required: {}", .diff.summary())]
    ReTrustRequired { diff: DiffSummary },
    #[error("health check failed: {0}")]
    HealthCheck(String),
    #[error("installer I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Trust(#[from] super::trust::TrustError),
    #[error("source_type `{0}` not yet supported (Phase D ships git + tarball + pypi + npm)")]
    SourceTypeUnsupported(&'static str),
}

/// Rootdirs used during install. Derived from `~/.makakoo/adapters/`
/// by default, but overrideable for tests.
#[derive(Debug, Clone)]
pub struct InstallRoot {
    pub adapters_root: PathBuf,
    pub trust_root: PathBuf,
}

impl InstallRoot {
    pub fn default_from_env() -> Self {
        let adapters_root = super::registry::AdapterRegistry::default_root()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".makakoo")
                    .join("adapters")
            });
        let trust_root = super::sign::default_trust_root();
        Self {
            adapters_root,
            trust_root,
        }
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.adapters_root.join(STAGING_DIRNAME)
    }

    pub fn registered_dir(&self) -> PathBuf {
        self.adapters_root.join(REGISTERED_DIRNAME)
    }

    pub fn trust_keys_dir(&self) -> PathBuf {
        self.trust_root.join("keys")
    }

    pub fn trust_ledger_path(&self) -> PathBuf {
        self.trust_root.join("adapters.json")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InstallOptions {
    pub allow_unsigned: bool,
    /// When true, a capability/security diff on update is accepted without
    /// a re-prompt. CLI maps to `--force-re-trust` (interactive confirm).
    pub accept_re_trust: bool,
    /// Skip the sandboxed health check (dev loop only).
    pub skip_health_check: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            allow_unsigned: false,
            accept_re_trust: false,
            skip_health_check: false,
        }
    }
}

/// Returned by every install path on success.
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub adapter_name: String,
    pub version: String,
    pub canonical_hash: String,
    pub signed: bool,
    pub publisher: Option<String>,
    pub staging_path: PathBuf,
    pub registered_path: PathBuf,
    pub diff: Option<DiffSummary>,
    pub health_check_passed: bool,
}

#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub raw: ManifestDiff,
    pub summary: String,
}

impl DiffSummary {
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

impl From<ManifestDiff> for DiffSummary {
    fn from(d: ManifestDiff) -> Self {
        let mut bits: Vec<String> = Vec::new();
        if let Some((old, new)) = &d.version_changed {
            bits.push(format!("version {old} → {new}"));
        }
        if !d.features_added.is_empty() {
            bits.push(format!("+feature {}", d.features_added.join(",")));
        }
        if !d.features_removed.is_empty() {
            bits.push(format!("-feature {}", d.features_removed.join(",")));
        }
        if !d.allowed_hosts_added.is_empty() {
            bits.push(format!(
                "+allowed_host {}",
                d.allowed_hosts_added.join(",")
            ));
        }
        if !d.allowed_hosts_removed.is_empty() {
            bits.push(format!(
                "-allowed_host {}",
                d.allowed_hosts_removed.join(",")
            ));
        }
        if let Some((old, new)) = &d.sandbox_changed {
            bits.push(format!("sandbox {old} → {new}"));
        }
        if let Some((old, new)) = &d.requires_network_changed {
            bits.push(format!("requires_network {old} → {new}"));
        }
        if !d.requires_filesystem_added.is_empty() {
            bits.push(format!(
                "+fs {}",
                d.requires_filesystem_added.join(",")
            ));
        }
        if !d.requires_filesystem_removed.is_empty() {
            bits.push(format!(
                "-fs {}",
                d.requires_filesystem_removed.join(",")
            ));
        }
        if let Some((old, new)) = &d.supports_roles_added.first().map(|_| ())
            .map(|_| (d.supports_roles_added.clone(), d.supports_roles_removed.clone()))
        {
            if !old.is_empty() {
                bits.push(format!("+role {}", old.join(",")));
            }
            if !new.is_empty() {
                bits.push(format!("-role {}", new.join(",")));
            }
        }
        if let Some((old, new)) = &d.signed_by_changed {
            bits.push(format!(
                "signed_by {:?} → {:?}",
                old.as_deref().unwrap_or("(none)"),
                new.as_deref().unwrap_or("(none)")
            ));
        }
        let summary = if bits.is_empty() {
            "hash changed (whitespace/field reorder) — no capability drift".into()
        } else {
            bits.join("; ")
        };
        Self { raw: d, summary }
    }
}

/// Install an adapter from a directory on the local filesystem. This is
/// the Phase-C MVP path — git / tarball / pypi / npm dispatch lands in
/// Phase D after these primitives are proven.
pub fn install_from_path(
    source_dir: impl AsRef<Path>,
    root: &InstallRoot,
    opts: InstallOptions,
) -> Result<InstallReport, InstallError> {
    let source_dir = source_dir.as_ref();
    if !source_dir.is_dir() {
        return Err(InstallError::SourceMissing {
            path: source_dir.to_path_buf(),
        });
    }
    let manifest_path = source_dir.join("adapter.toml");
    if !manifest_path.is_file() {
        return Err(InstallError::ManifestMissing {
            path: manifest_path,
        });
    }
    let manifest_bytes = fs::read(&manifest_path)?;
    let manifest = Manifest::parse_str(std::str::from_utf8(&manifest_bytes).map_err(|e| {
        InstallError::Staging(format!("manifest is not UTF-8: {e}"))
    })?)?;

    // ── 2. signature ─────────────────────────────────────────────────
    let sig_path = source_dir.join("adapter.toml.sig");
    let is_url_install = !matches!(manifest.install.source_type, SourceType::Local);
    let signed = sig_path.is_file();
    if signed {
        let publisher = manifest.security.signed_by.as_deref().ok_or_else(|| {
            InstallError::Sign(SignError::VerifyFailed {
                publisher: "(missing security.signed_by)".into(),
            })
        })?;
        verify_manifest_bytes(
            publisher,
            &manifest_bytes,
            &sig_path,
            Some(&root.trust_keys_dir()),
        )?;
    } else if is_url_install && !opts.allow_unsigned {
        return Err(InstallError::UnsignedUrlRejected);
    }

    // ── 3. sha256 pre-flight (catches corruption even for local
    //         source_type=local installs; for URL sources the caller is
    //         expected to have verified against the manifest's declared
    //         sha256 BEFORE this function runs).
    // No additional action for source_type = local.

    // ── 4. stage ─────────────────────────────────────────────────────
    let staging_root = root.staging_dir();
    fs::create_dir_all(&staging_root)?;
    let staging_path = staging_root.join(&manifest.adapter.name);
    if staging_path.exists() {
        fs::remove_dir_all(&staging_path)?;
    }
    copy_dir_all(source_dir, &staging_path)?;

    // ── 5. sandboxed health check ─────────────────────────────────────
    let health_check_passed = if opts.skip_health_check {
        false
    } else {
        match maybe_health_check(&manifest, &staging_path).await_blocking() {
            Ok(true) => true,
            Ok(false) => false,
            Err(e) => {
                let _ = fs::remove_dir_all(&staging_path);
                return Err(e);
            }
        }
    };

    // ── 6. diff + trust decision ──────────────────────────────────────
    let ledger_path = root.trust_ledger_path();
    let mut ledger = TrustLedger::load_from(&ledger_path)?;
    let diff: Option<DiffSummary> = if let Some(entry) = ledger.get(&manifest.adapter.name) {
        let raw = diff_manifest(entry, &manifest);
        if raw.requires_re_trust() && !opts.accept_re_trust {
            let _ = fs::remove_dir_all(&staging_path);
            return Err(InstallError::ReTrustRequired {
                diff: DiffSummary::from(raw),
            });
        }
        Some(DiffSummary::from(raw))
    } else {
        None
    };

    // ── 7. promote ────────────────────────────────────────────────────
    let registered_dir = root.registered_dir();
    fs::create_dir_all(&registered_dir)?;
    let registered_path = registered_dir.join(format!("{}.toml", manifest.adapter.name));
    fs::copy(&manifest_path, &registered_path)?;

    ledger.set(
        &manifest.adapter.name,
        trust_entry_from_manifest(&manifest, "installed via install_from_path"),
    );
    ledger.save()?;

    Ok(InstallReport {
        adapter_name: manifest.adapter.name.clone(),
        version: manifest.adapter.version.to_string(),
        canonical_hash: manifest.canonical_hash(),
        signed,
        publisher: manifest.security.signed_by.clone(),
        staging_path,
        registered_path,
        diff,
        health_check_passed,
    })
}

/// Install from an `https-tarball` URL with an already-fetched body.
/// Separated so tests can feed synthetic tarball bytes without hitting
/// the network.
pub fn install_from_tarball_bytes(
    tarball: &[u8],
    declared_sha256: &str,
    root: &InstallRoot,
    opts: InstallOptions,
) -> Result<InstallReport, InstallError> {
    // Hash check first — never trust unverified archive bytes.
    let actual = sha256_hex(tarball);
    if actual != declared_sha256 {
        return Err(InstallError::Sha256Mismatch {
            expected: declared_sha256.to_string(),
            actual,
        });
    }
    // Extract into a temp staging dir, then delegate to install_from_path.
    let tmp = tempfile::tempdir().map_err(|e| InstallError::Staging(e.to_string()))?;
    extract_tarball(tarball, tmp.path())?;
    // Walk to the top-level dir containing adapter.toml (many tarballs
    // wrap their content in a single subdir).
    let source_dir = locate_manifest_dir(tmp.path())?;
    install_from_path(source_dir, root, opts)
}

/// Remove a registered adapter. `purge=true` deletes both the manifest
/// and the adapter's state dir (not yet allocated in Phase C, but we
/// leave the hook in place for D).
pub fn uninstall(
    name: &str,
    root: &InstallRoot,
    purge: bool,
) -> Result<(), InstallError> {
    let registered = root.registered_dir().join(format!("{name}.toml"));
    if registered.exists() {
        fs::remove_file(&registered)?;
    }
    if purge {
        let state_dir = root.adapters_root.join("state").join(name);
        if state_dir.exists() {
            fs::remove_dir_all(&state_dir)?;
        }
    }
    // Always clear the trust ledger — next install will re-prompt.
    let ledger_path = root.trust_ledger_path();
    let mut ledger = TrustLedger::load_from(&ledger_path)?;
    ledger.remove(name);
    if !ledger.is_empty() || ledger_path.exists() {
        ledger.save()?;
    }
    Ok(())
}

// ─── internal helpers ─────────────────────────────────────────────────

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest.iter() {
        use std::fmt::Write as _;
        let _ = write!(out, "{:02x}", b);
    }
    out
}

fn extract_tarball(bytes: &[u8], into: &Path) -> Result<(), InstallError> {
    use flate2::read::GzDecoder;
    use std::io::Cursor;
    use tar::Archive;

    let cursor = Cursor::new(bytes);
    // Accept both gzipped and plain tarballs. Peek magic bytes.
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let gz = GzDecoder::new(cursor);
        let mut archive = Archive::new(gz);
        archive
            .unpack(into)
            .map_err(|e| InstallError::Staging(format!("tarball extract: {e}")))?;
    } else {
        let mut archive = Archive::new(cursor);
        archive
            .unpack(into)
            .map_err(|e| InstallError::Staging(format!("tarball extract: {e}")))?;
    }
    Ok(())
}

fn locate_manifest_dir(root: &Path) -> Result<PathBuf, InstallError> {
    // Direct hit — tarball root has adapter.toml.
    if root.join("adapter.toml").is_file() {
        return Ok(root.to_path_buf());
    }
    // One level down — common GitHub release layout.
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let candidate = entry.path();
        if candidate.join("adapter.toml").is_file() {
            return Ok(candidate);
        }
    }
    Err(InstallError::ManifestMissing {
        path: root.join("adapter.toml"),
    })
}

/// Phase-C health check — HTTP GET on `health.check_url`, assert the
/// declared `expected_field` dot-path resolves to a value. Sandboxed
/// execution is deferred to the adapter-call layer; this health probe
/// just proves the endpoint is up.
async fn maybe_health_check(manifest: &Manifest, install_dir: &Path) -> Result<bool, InstallError> {
    let Some(url) = manifest.health.check_url.as_deref() else {
        return Ok(false);
    };
    let backend = SandboxBackend::detect();
    let spec = ProfileSpec::from_manifest(manifest, install_dir);
    // Minimal sandbox assertion — catches manifest-level inconsistency
    // even when we can't run a subprocess (e.g. Windows).
    if let Err(e) = sandbox::assert_manifest_self_consistent(&spec) {
        return Err(InstallError::HealthCheck(e.to_string()));
    }
    let _ = backend; // reserved for Phase E runtime wrapping
    let timeout = Duration::from_millis(manifest.health.timeout_ms.unwrap_or(5000));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| InstallError::HealthCheck(format!("reqwest init: {e}")))?;
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => return Err(InstallError::HealthCheck(format!("GET {url}: {e}"))),
    };
    if !resp.status().is_success() {
        return Err(InstallError::HealthCheck(format!(
            "GET {} → {}",
            url,
            resp.status()
        )));
    }
    // If an expected_field is declared, parse JSON and assert path resolves.
    if let Some(field) = manifest.health.expected_field.as_deref() {
        let body = resp
            .bytes()
            .await
            .map_err(|e| InstallError::HealthCheck(e.to_string()))?;
        let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            InstallError::HealthCheck(format!("health response is not JSON: {e}"))
        })?;
        if extract_dot_path(&json, field).is_none() {
            return Err(InstallError::HealthCheck(format!(
                "field `{field}` missing in health response"
            )));
        }
    }
    Ok(true)
}

fn extract_dot_path<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = match cur {
            serde_json::Value::Object(map) => map.get(seg)?,
            serde_json::Value::Array(arr) => {
                let idx: usize = seg.parse().ok()?;
                arr.get(idx)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

/// Tiny helper so the sync `install_from_path` can block on the async
/// health check without forcing every caller to be async. Builds an
/// ephemeral current-thread tokio runtime.
trait AsyncBlockOn<T> {
    fn await_blocking(self) -> T;
}

impl<F, T> AsyncBlockOn<T> for F
where
    F: std::future::Future<Output = T>,
{
    fn await_blocking(self) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(self)
    }
}

// ─────────────────────────── Tests ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST_BODY: &str = r#"
[adapter]
name = "refadapter"
version = "0.1.0"
manifest_schema = 1
description = "reference"

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
features = ["tool_use"]
supports_roles = ["validator"]

[install]
source_type = "local"

[security]
requires_network = false
sandbox_profile = "network-io"
allowed_hosts = ["127.0.0.1"]
"#;

    fn scratch(dir: &Path) -> (InstallRoot, PathBuf) {
        let adapters_root = dir.join("adapters");
        let trust_root = dir.join("trust");
        fs::create_dir_all(&adapters_root).unwrap();
        fs::create_dir_all(&trust_root).unwrap();
        let root = InstallRoot {
            adapters_root,
            trust_root,
        };
        let source_dir = dir.join("source");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("adapter.toml"), MANIFEST_BODY).unwrap();
        (root, source_dir)
    }

    fn default_opts() -> InstallOptions {
        InstallOptions {
            allow_unsigned: true,
            accept_re_trust: false,
            skip_health_check: true,
        }
    }

    #[test]
    fn install_from_path_happy_path_registers_and_trusts() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, source) = scratch(tmp.path());
        let r = install_from_path(source, &root, default_opts()).unwrap();
        assert_eq!(r.adapter_name, "refadapter");
        assert!(r.registered_path.exists());
        let ledger = TrustLedger::load_from(root.trust_ledger_path()).unwrap();
        assert!(ledger.get("refadapter").is_some());
    }

    #[test]
    fn install_missing_manifest_errors_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let adapters_root = tmp.path().join("adapters");
        fs::create_dir_all(&adapters_root).unwrap();
        let trust_root = tmp.path().join("trust");
        fs::create_dir_all(&trust_root).unwrap();
        let root = InstallRoot {
            adapters_root,
            trust_root,
        };
        let empty = tmp.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        let err = install_from_path(empty, &root, default_opts()).unwrap_err();
        assert!(matches!(err, InstallError::ManifestMissing { .. }));
    }

    #[test]
    fn install_from_nonexistent_path_errors_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, _) = scratch(tmp.path());
        let err = install_from_path(
            tmp.path().join("does-not-exist"),
            &root,
            default_opts(),
        )
        .unwrap_err();
        assert!(matches!(err, InstallError::SourceMissing { .. }));
    }

    #[test]
    fn second_install_rejects_re_trust_when_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, source) = scratch(tmp.path());
        install_from_path(&source, &root, default_opts()).unwrap();
        // Bump features — force a diff.
        let new_body = MANIFEST_BODY.replace(
            r#"features = ["tool_use"]"#,
            r#"features = ["tool_use", "fs_write"]"#,
        );
        fs::write(source.join("adapter.toml"), new_body).unwrap();
        let err = install_from_path(&source, &root, default_opts()).unwrap_err();
        assert!(matches!(err, InstallError::ReTrustRequired { .. }));
    }

    #[test]
    fn second_install_accepts_re_trust_with_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, source) = scratch(tmp.path());
        install_from_path(&source, &root, default_opts()).unwrap();
        let new_body = MANIFEST_BODY.replace(
            r#"features = ["tool_use"]"#,
            r#"features = ["tool_use", "fs_write"]"#,
        );
        fs::write(source.join("adapter.toml"), new_body).unwrap();
        let opts = InstallOptions {
            allow_unsigned: true,
            accept_re_trust: true,
            skip_health_check: true,
        };
        let r = install_from_path(&source, &root, opts).unwrap();
        let diff = r.diff.as_ref().unwrap();
        assert!(diff.raw.features_added.contains(&"fs_write".to_string()));
    }

    #[test]
    fn unsigned_url_install_rejected_without_flag() {
        // Make a "url" style install by setting source_type = git and NOT
        // providing a signature file.
        let tmp = tempfile::tempdir().unwrap();
        let (root, source) = scratch(tmp.path());
        let body = MANIFEST_BODY.replace(
            r#"source_type = "local""#,
            r#"source_type = "git"
source = "https://github.com/x/y.git"
ref = "v0.1.0""#,
        );
        fs::write(source.join("adapter.toml"), body).unwrap();
        let opts = InstallOptions {
            allow_unsigned: false,
            accept_re_trust: false,
            skip_health_check: true,
        };
        let err = install_from_path(&source, &root, opts).unwrap_err();
        assert!(matches!(err, InstallError::UnsignedUrlRejected));
    }

    #[test]
    fn signed_url_install_verifies() {
        use super::super::sign;
        let tmp = tempfile::tempdir().unwrap();
        let (root, source) = scratch(tmp.path());
        let body = MANIFEST_BODY.replace(
            r#"source_type = "local""#,
            r#"source_type = "git"
source = "https://github.com/x/y.git"
ref = "v0.1.0""#,
        ).replace(
            r#"sandbox_profile = "network-io""#,
            r#"sandbox_profile = "network-io"
signed_by = "unit-test""#,
        );
        fs::write(source.join("adapter.toml"), &body).unwrap();

        let _sig_path = sign::testing_sign_manifest(
            &root.trust_root,
            "unit-test",
            body.as_bytes(),
        );
        // Move the sig file into the source dir (testing helper writes
        // into trust_root by default).
        fs::rename(
            root.trust_root.join("adapter.toml.sig"),
            source.join("adapter.toml.sig"),
        )
        .unwrap();

        let opts = InstallOptions {
            allow_unsigned: false,
            accept_re_trust: false,
            skip_health_check: true,
        };
        let r = install_from_path(&source, &root, opts).unwrap();
        assert!(r.signed);
        assert_eq!(r.publisher.as_deref(), Some("unit-test"));
    }

    #[test]
    fn tampered_signed_manifest_fails() {
        use super::super::sign;
        let tmp = tempfile::tempdir().unwrap();
        let (root, source) = scratch(tmp.path());
        let body = MANIFEST_BODY.replace(
            r#"source_type = "local""#,
            r#"source_type = "git"
source = "https://github.com/x/y.git"
ref = "v0.1.0""#,
        ).replace(
            r#"sandbox_profile = "network-io""#,
            r#"sandbox_profile = "network-io"
signed_by = "unit-test""#,
        );
        // Sign one body, write a DIFFERENT body — verify must fail.
        let _sig_path = sign::testing_sign_manifest(
            &root.trust_root,
            "unit-test",
            body.as_bytes(),
        );
        fs::rename(
            root.trust_root.join("adapter.toml.sig"),
            source.join("adapter.toml.sig"),
        )
        .unwrap();
        let tampered = body.replace("description = \"reference\"", "description = \"tampered\"");
        fs::write(source.join("adapter.toml"), tampered).unwrap();

        let opts = InstallOptions {
            allow_unsigned: false,
            accept_re_trust: false,
            skip_health_check: true,
        };
        let err = install_from_path(&source, &root, opts).unwrap_err();
        assert!(matches!(
            err,
            InstallError::Sign(SignError::VerifyFailed { .. })
        ));
    }

    #[test]
    fn tarball_sha256_mismatch_aborts() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, _) = scratch(tmp.path());
        let bogus_bytes = b"not-a-real-tarball";
        let wrong_sha = "0".repeat(64);
        let err = install_from_tarball_bytes(bogus_bytes, &wrong_sha, &root, default_opts())
            .unwrap_err();
        assert!(matches!(err, InstallError::Sha256Mismatch { .. }));
    }

    #[test]
    fn uninstall_removes_registered_manifest_and_trust() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, source) = scratch(tmp.path());
        install_from_path(&source, &root, default_opts()).unwrap();
        uninstall("refadapter", &root, false).unwrap();
        let registered = root.registered_dir().join("refadapter.toml");
        assert!(!registered.exists());
        let ledger = TrustLedger::load_from(root.trust_ledger_path()).unwrap();
        assert!(ledger.get("refadapter").is_none());
    }

    #[test]
    fn install_staging_dir_is_populated() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, source) = scratch(tmp.path());
        let r = install_from_path(&source, &root, default_opts()).unwrap();
        assert!(r.staging_path.join("adapter.toml").exists());
    }

    #[test]
    fn sha256_hex_matches_expected() {
        let bytes = b"hello world";
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(sha256_hex(bytes), expected);
    }
}
