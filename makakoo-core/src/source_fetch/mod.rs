//! `core::source_fetch` — shared git + tarball + local source fetcher.
//!
//! One module, three fetchers, every consumer in the workspace uses it:
//! - adapter installer (`adapter::install`) wraps it for `install_from_git`
//!   and `install_from_tarball_url`.
//! - plugin installer (`plugin::install`) dispatches git / tarball / path
//!   sources through it in v0.4 (Phase B).
//!
//! ## Design
//!
//! Pure side-effects contract: fetch a source spec into a staging directory
//! on disk, return the absolute path + the resolved SHA or content hash.
//! Nothing here knows about manifests, sandboxes, or trust ledgers — those
//! are caller concerns.
//!
//! Shell-outs rather than Rust-native git libraries: `git` and `curl` are
//! already required on every Makakoo host (install docs mandate both), so
//! a shell-out is zero new deps, identical behavior to what users would
//! type by hand, and robust against weird server-side edge cases (redirects,
//! GitHub's shallow-fetch-by-sha quirks, `.gitattributes` filters).
//!
//! ## Ref validation
//!
//! Locked decision D1 (sprint doc §3): git refs MUST be pinned. Accept:
//! - semver tag: `/^v?\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?(\+[a-zA-Z0-9.]+)?$/`
//! - 40-char SHA: `/^[a-f0-9]{40}$/`
//!
//! Anything else (`main`, `master`, `develop`, short SHAs) is treated as a
//! bare branch: rejected unless the caller sets `allow_unstable = true`
//! (wired to `--allow-unstable-ref` in the CLI).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use once_cell::sync::Lazy;
use regex::Regex;
use tempfile::TempDir;
use thiserror::Error;

static TAG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^v?\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?(\+[a-zA-Z0-9.]+)?$").unwrap()
});
static SHA40_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-f0-9]{40}$").unwrap());

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("unstable git ref {ref_:?} — pass allow_unstable or use a semver tag / 40-char SHA")]
    UnstableRef { ref_: String },
    #[error("git {operation} failed for {url} @ {ref_}: {stderr}")]
    Git {
        operation: &'static str,
        url: String,
        ref_: String,
        stderr: String,
    },
    #[error("tarball download failed for {url}: {reason}")]
    TarballHttp { url: String, reason: String },
    #[error("sha256 mismatch for {url}: expected {expected}, got {actual}")]
    Sha256Mismatch {
        url: String,
        expected: String,
        actual: String,
    },
    #[error("tarball extract failed: {0}")]
    TarballExtract(String),
    #[error("local source path {0:?} does not exist")]
    LocalMissing(PathBuf),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub enum SourceSpec {
    Git {
        url: String,
        ref_: String,
        allow_unstable: bool,
    },
    HttpsTarball {
        url: String,
        sha256: String,
    },
    Local {
        path: PathBuf,
    },
}

/// Outcome of a fetch. The `staging_dir` is an on-disk directory the caller
/// owns — promote it, copy out of it, then delete. `source_fetch` does not
/// auto-clean on drop; the `TempDir` guard is consumed during fetch so the
/// caller sees a plain `PathBuf`.
#[derive(Debug)]
pub struct FetchedSource {
    pub staging_dir: PathBuf,
    pub resolved_sha: String,
    pub source_str: String,
}

pub fn fetch(spec: &SourceSpec) -> Result<FetchedSource, FetchError> {
    match spec {
        SourceSpec::Git {
            url,
            ref_,
            allow_unstable,
        } => fetch_git(url, ref_, *allow_unstable),
        SourceSpec::HttpsTarball { url, sha256 } => fetch_tarball(url, sha256),
        SourceSpec::Local { path } => fetch_local(path),
    }
}

/// True iff `ref_` matches the locked v0.4 stable-ref rule (tag OR 40-char SHA).
pub fn is_stable_ref(ref_: &str) -> bool {
    TAG_RE.is_match(ref_) || SHA40_RE.is_match(ref_)
}

pub fn is_sha40(ref_: &str) -> bool {
    SHA40_RE.is_match(ref_)
}

fn fetch_git(url: &str, ref_: &str, allow_unstable: bool) -> Result<FetchedSource, FetchError> {
    if !is_stable_ref(ref_) && !allow_unstable {
        return Err(FetchError::UnstableRef {
            ref_: ref_.to_string(),
        });
    }

    let tmp = TempDir::new()?;
    // Take ownership of the tempdir path NOW — we'll fill it in-place.
    // Drop the guard without auto-cleaning so caller keeps the dir.
    let staging_dir = tmp.keep();

    if is_sha40(ref_) {
        // Shallow fetch-by-SHA — works against GitHub, GitLab, and any
        // server with uploadpack.allowAnySHA1InWant (default on modern git).
        run_git(
            &["init", "--quiet", staging_dir.to_str().unwrap()],
            None,
            url,
            ref_,
            "init",
        )?;
        run_git(
            &["remote", "add", "origin", url],
            Some(&staging_dir),
            url,
            ref_,
            "remote-add",
        )?;
        run_git(
            &["fetch", "--depth", "1", "origin", ref_],
            Some(&staging_dir),
            url,
            ref_,
            "fetch",
        )?;
        run_git(
            &["checkout", "--quiet", ref_],
            Some(&staging_dir),
            url,
            ref_,
            "checkout",
        )?;
    } else {
        // Tag or (with allow_unstable) branch. `--branch` accepts both.
        let status = Command::new("git")
            .args([
                "clone",
                "--quiet",
                "--depth",
                "1",
                "--branch",
                ref_,
                url,
                staging_dir.to_str().unwrap(),
            ])
            .output()
            .map_err(|e| FetchError::Git {
                operation: "clone",
                url: url.into(),
                ref_: ref_.into(),
                stderr: format!("spawn: {e}"),
            })?;
        if !status.status.success() {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(FetchError::Git {
                operation: "clone",
                url: url.into(),
                ref_: ref_.into(),
                stderr: String::from_utf8_lossy(&status.stderr).trim().to_string(),
            });
        }
    }

    let resolved_sha = git_head_sha(&staging_dir, url, ref_)?;

    // Scrub .git so the staged tree is a pure source copy — the install
    // dir never contains git history (smaller, no accidental leaks).
    let git_dir = staging_dir.join(".git");
    if git_dir.exists() {
        fs::remove_dir_all(&git_dir)?;
    }

    Ok(FetchedSource {
        staging_dir,
        resolved_sha,
        source_str: format!("git:{url}@{ref_}"),
    })
}

fn run_git(
    args: &[&str],
    cwd: Option<&Path>,
    url: &str,
    ref_: &str,
    operation: &'static str,
) -> Result<(), FetchError> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    let out = cmd.output().map_err(|e| FetchError::Git {
        operation,
        url: url.into(),
        ref_: ref_.into(),
        stderr: format!("spawn: {e}"),
    })?;
    if !out.status.success() {
        return Err(FetchError::Git {
            operation,
            url: url.into(),
            ref_: ref_.into(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(())
}

fn git_head_sha(dir: &Path, url: &str, ref_: &str) -> Result<String, FetchError> {
    let out = Command::new("git")
        .args(["-C", dir.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .map_err(|e| FetchError::Git {
            operation: "rev-parse",
            url: url.into(),
            ref_: ref_.into(),
            stderr: format!("spawn: {e}"),
        })?;
    if !out.status.success() {
        return Err(FetchError::Git {
            operation: "rev-parse",
            url: url.into(),
            ref_: ref_.into(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn fetch_tarball(url: &str, expected_sha: &str) -> Result<FetchedSource, FetchError> {
    let tmp = TempDir::new()?;
    let tmp_path = tmp.keep();
    let archive = tmp_path.join("download.tar.gz");

    let out = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "-o",
            archive.to_str().unwrap(),
            url,
        ])
        .output()
        .map_err(|e| FetchError::TarballHttp {
            url: url.into(),
            reason: format!("spawn: {e}"),
        })?;
    if !out.status.success() {
        let _ = fs::remove_dir_all(&tmp_path);
        return Err(FetchError::TarballHttp {
            url: url.into(),
            reason: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }

    let bytes = fs::read(&archive)?;
    let actual = sha256_hex(&bytes);
    if !expected_sha.is_empty() && actual != expected_sha {
        let _ = fs::remove_dir_all(&tmp_path);
        return Err(FetchError::Sha256Mismatch {
            url: url.into(),
            expected: expected_sha.into(),
            actual,
        });
    }

    let extract_dir = tmp_path.join("extract");
    fs::create_dir_all(&extract_dir)?;
    extract_tarball(&bytes, &extract_dir).map_err(FetchError::TarballExtract)?;
    let _ = fs::remove_file(&archive);

    // Unwrap single-subdir layout (common GitHub-release shape).
    let inner = locate_unwrapped_root(&extract_dir)?;
    let final_dir = tmp_path.join("stage");
    if inner != extract_dir {
        fs::rename(&inner, &final_dir)?;
        let _ = fs::remove_dir_all(&extract_dir);
    } else {
        fs::rename(&extract_dir, &final_dir)?;
    }

    Ok(FetchedSource {
        staging_dir: final_dir,
        resolved_sha: actual,
        source_str: format!("tar:{url}"),
    })
}

fn fetch_local(path: &Path) -> Result<FetchedSource, FetchError> {
    if !path.exists() {
        return Err(FetchError::LocalMissing(path.to_path_buf()));
    }
    let canonical = path.canonicalize()?;
    let source_str = format!("path:{}", canonical.display());
    Ok(FetchedSource {
        staging_dir: canonical,
        resolved_sha: String::new(),
        source_str,
    })
}

// ─── shared helpers (also consumed by adapter::install) ──────────────

pub fn sha256_hex(bytes: &[u8]) -> String {
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

pub fn extract_tarball(bytes: &[u8], into: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use std::io::Cursor;
    use tar::Archive;

    let cursor = Cursor::new(bytes);
    // Peek magic bytes so we transparently handle .tar and .tar.gz.
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let gz = GzDecoder::new(cursor);
        let mut archive = Archive::new(gz);
        archive.unpack(into).map_err(|e| format!("extract: {e}"))?;
    } else {
        let mut archive = Archive::new(cursor);
        archive.unpack(into).map_err(|e| format!("extract: {e}"))?;
    }
    Ok(())
}

fn locate_unwrapped_root(extract_dir: &Path) -> Result<PathBuf, FetchError> {
    let entries: Vec<_> = fs::read_dir(extract_dir)?
        .filter_map(|e| e.ok())
        .collect();
    if entries.len() == 1 {
        let only = entries[0].path();
        if only.is_dir() {
            return Ok(only);
        }
    }
    Ok(extract_dir.to_path_buf())
}

// ─────────────────────────── tests ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Initialize a bare git repo + seed one commit + a `v0.1.0` tag. Returns
    /// (tempdir, bare_url, tag, resolved_sha).
    fn seed_bare_repo() -> (TempDir, String, String, String) {
        let tmp = TempDir::new().unwrap();
        let bare = tmp.path().join("bare.git");
        let wt = tmp.path().join("wt");
        must_git(&["init", "--bare", "--quiet", bare.to_str().unwrap()], None);
        must_git(
            &[
                "clone",
                "--quiet",
                bare.to_str().unwrap(),
                wt.to_str().unwrap(),
            ],
            None,
        );
        must_git(&["config", "user.email", "t@t.test"], Some(&wt));
        must_git(&["config", "user.name", "t"], Some(&wt));
        must_git(&["config", "commit.gpgsign", "false"], Some(&wt));
        fs::write(wt.join("hello.txt"), "world\n").unwrap();
        must_git(&["add", "."], Some(&wt));
        must_git(&["commit", "--quiet", "-m", "init"], Some(&wt));
        let sha_out = Command::new("git")
            .current_dir(&wt)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
        must_git(&["tag", "v0.1.0"], Some(&wt));
        must_git(&["branch", "-M", "main"], Some(&wt));
        must_git(&["push", "--quiet", "origin", "main", "--tags"], Some(&wt));
        let url = format!("file://{}", bare.display());
        (tmp, url, "v0.1.0".into(), sha)
    }

    fn must_git(args: &[&str], cwd: Option<&Path>) {
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
    fn fetch_git_tag_happy_path() {
        let (_fixture, url, tag, expected_sha) = seed_bare_repo();
        let r = fetch(&SourceSpec::Git {
            url: url.clone(),
            ref_: tag.clone(),
            allow_unstable: false,
        })
        .unwrap();
        assert!(r.staging_dir.join("hello.txt").exists());
        assert!(!r.staging_dir.join(".git").exists(), "must scrub .git");
        assert_eq!(r.resolved_sha, expected_sha);
        assert_eq!(r.source_str, format!("git:{url}@v0.1.0"));
        let _ = fs::remove_dir_all(&r.staging_dir);
    }

    #[test]
    fn fetch_git_bare_branch_rejected_without_flag() {
        let err = fetch(&SourceSpec::Git {
            url: "file:///does/not/matter".into(),
            ref_: "main".into(),
            allow_unstable: false,
        })
        .unwrap_err();
        assert!(matches!(err, FetchError::UnstableRef { ref_ } if ref_ == "main"));
    }

    #[test]
    fn fetch_git_bare_branch_allowed_with_flag() {
        let (_fixture, url, _tag, expected_sha) = seed_bare_repo();
        let r = fetch(&SourceSpec::Git {
            url,
            ref_: "main".into(),
            allow_unstable: true,
        })
        .unwrap();
        assert_eq!(r.resolved_sha, expected_sha);
        let _ = fs::remove_dir_all(&r.staging_dir);
    }

    #[test]
    fn fetch_git_sha40_happy_path() {
        let (_fixture, url, _tag, expected_sha) = seed_bare_repo();
        let r = fetch(&SourceSpec::Git {
            url: url.clone(),
            ref_: expected_sha.clone(),
            allow_unstable: false,
        })
        .unwrap();
        assert_eq!(r.resolved_sha, expected_sha);
        let _ = fs::remove_dir_all(&r.staging_dir);
    }

    #[test]
    fn ref_validation_matches_spec() {
        assert!(is_stable_ref("v1.2.3"));
        assert!(is_stable_ref("1.2.3"));
        assert!(is_stable_ref("v1.2.3-alpha.1"));
        assert!(is_stable_ref("v1.0.0-rc1"));
        assert!(is_stable_ref(&"a".repeat(40)));
        assert!(is_stable_ref(&"0".repeat(40)));
        assert!(!is_stable_ref("main"));
        assert!(!is_stable_ref("master"));
        assert!(!is_stable_ref("develop"));
        assert!(!is_stable_ref("abc1234")); // short SHA
        assert!(!is_stable_ref(""));
        assert!(!is_stable_ref("v1.2")); // not semver
    }

    #[test]
    fn fetch_tarball_sha256_mismatch_rejected() {
        let tmp = TempDir::new().unwrap();
        let pack = tmp.path().join("junk.tar.gz");
        // Write a harmless, not-really-a-tar blob. We only care that curl
        // can retrieve it via file:// and the hash check runs.
        fs::write(&pack, b"not-a-real-tarball").unwrap();
        let url = format!("file://{}", pack.display());
        let err = fetch(&SourceSpec::HttpsTarball {
            url: url.clone(),
            sha256: "0".repeat(64),
        })
        .unwrap_err();
        // Accept either: the hash check rejects before extract (desired),
        // or curl doesn't support file:// in this env (TarballHttp). Both
        // prove the bytes did not end up staged.
        assert!(
            matches!(err, FetchError::Sha256Mismatch { .. } | FetchError::TarballHttp { .. }),
            "unexpected error variant: {err:?}"
        );
    }

    #[test]
    fn fetch_local_returns_canonical_path() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("pkg");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("plugin.toml"), "# ok\n").unwrap();
        let r = fetch(&SourceSpec::Local { path: sub.clone() }).unwrap();
        assert!(r.staging_dir.ends_with("pkg"));
        assert!(r.source_str.starts_with("path:"));
        assert!(r.resolved_sha.is_empty());
    }

    #[test]
    fn fetch_local_missing_path_errors() {
        let err = fetch(&SourceSpec::Local {
            path: PathBuf::from("/totally/not/a/real/path/xyzzy"),
        })
        .unwrap_err();
        assert!(matches!(err, FetchError::LocalMissing(_)));
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
