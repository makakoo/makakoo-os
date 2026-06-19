//! Integration tests for `install/install.sh`.
//!
//! Verifies the installer script parses + runs its `--help` and
//! `--dry-run` paths, and end-to-end-installs from a local tarball
//! (skipping the GitHub download path) so the extract + move + chmod
//! flow is exercised on every `cargo test`.
//!
//! **Compiled only on Unix** — install.sh is a bash script; the
//! Windows counterpart (install.ps1) has its own PowerShell-based
//! test harness that would go into a sibling `install_ps1.rs`
//! compiled only on Windows. Skipping here via a file-level cfg
//! keeps Windows CI green without polluting every test body with
//! cfg forks.

#![cfg(unix)]

use std::path::PathBuf;

use tokio::process::Command;

fn script_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("manifest dir has parent")
        .join("install/install.sh")
}

async fn bash_available() -> bool {
    Command::new("bash")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn help_prints_usage_and_exits_zero() {
    if !bash_available().await {
        eprintln!("skipping install_sh tests: bash not on PATH");
        return;
    }
    let out = Command::new("bash")
        .arg(script_path())
        .arg("--help")
        .output()
        .await
        .expect("bash spawn failed");
    assert!(out.status.success(), "bash --help exited {}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Makakoo OS installer"));
    assert!(stdout.contains("--version"));
    assert!(stdout.contains("--install-dir"));
    assert!(stdout.contains("--dry-run"));
}

#[tokio::test]
async fn dry_run_prints_plan_and_exits_zero() {
    if !bash_available().await {
        return;
    }
    let out = Command::new("bash")
        .arg(script_path())
        .arg("--dry-run")
        .output()
        .await
        .expect("bash spawn failed");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Makakoo OS installer"));
    assert!(stdout.contains("--dry-run: no changes made"));
    // Plan must reveal target + URL so users know what they're about
    // to download.
    assert!(stdout.contains("apple-darwin") || stdout.contains("linux"));
    assert!(stdout.contains("https://github.com/"));
}

#[tokio::test]
async fn unknown_flag_fails_with_usage() {
    if !bash_available().await {
        return;
    }
    let out = Command::new("bash")
        .arg(script_path())
        .arg("--not-a-real-flag")
        .output()
        .await
        .expect("bash spawn failed");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown option"));
}

#[tokio::test]
async fn local_tarball_end_to_end() {
    if !bash_available().await {
        return;
    }
    // Arrange: build the current makakoo binary, pack it the way
    // cargo-dist would, install via script, verify the file lands
    // where we asked for it.
    let tmp = tempfile::TempDir::new().unwrap();
    let pack = tmp.path().join("pack");
    let install_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&pack).unwrap();

    // The makakoo binary built for this test run. CARGO_BIN_EXE_<name> is set
    // by Cargo for integration tests and honors CARGO_TARGET_DIR + the active
    // profile, so it resolves correctly under an external target directory.
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_makakoo"));
    std::fs::copy(&bin, pack.join("makakoo")).unwrap();

    let tarball = tmp.path().join("makakoo.tar.gz");
    let tar_status = Command::new("tar")
        .arg("-czf")
        .arg(&tarball)
        .arg("-C")
        .arg(&pack)
        .arg("makakoo")
        .status()
        .await
        .expect("tar failed");
    assert!(tar_status.success());

    let out = Command::new("bash")
        .arg(script_path())
        .env("MAKAKOO_LOCAL_TARBALL", &tarball)
        .env("MAKAKOO_INSTALL_DIR", &install_dir)
        .output()
        .await
        .expect("bash spawn failed");
    if !out.status.success() {
        panic!(
            "installer failed:\n---stdout---\n{}\n---stderr---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    let installed = install_dir.join("makakoo");
    assert!(installed.exists(), "binary not placed at {installed:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&installed).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "binary should be executable");
    }

    // Smoke the installed binary actually runs — simplest is `--help`
    // on a read-only subcommand.
    let bin_out = Command::new(&installed)
        .arg("version")
        .output()
        .await
        .expect("installed binary failed to run");
    assert!(bin_out.status.success());
}

// ─── checksum verification (F12) ─────────────────────────────────────────
//
// These exercise the remote-download path via a `file://` mirror set with
// MAKAKOO_BASE_URL, so we can stage a tarball + sidecar and assert the
// installer verifies the sha256 and fails closed on a mismatch. curl
// handles file:// (wget does not), so both tests skip when curl is absent.

/// The target triple install.sh derives from `uname` on this host —
/// mirrors the case arms in the script.
fn current_target() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => other,
    };
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        other => other,
    };
    format!("{arch}-{os}")
}

async fn curl_available() -> bool {
    Command::new("curl")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// sha256 of a file as lowercase hex — shells out so the test needs no
/// hashing crate. Prefers sha256sum (Linux), falls back to shasum (macOS).
async fn sha256_hex(path: &std::path::Path) -> String {
    let attempts: [(&str, &[&str]); 2] = [("sha256sum", &[]), ("shasum", &["-a", "256"])];
    for (bin, args) in attempts {
        let out = Command::new(bin).args(args).arg(path).output().await;
        if let Ok(o) = out {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout);
                if let Some(tok) = s.split_whitespace().next() {
                    return tok.to_lowercase();
                }
            }
        }
    }
    panic!("no sha256 tool (sha256sum/shasum) available to build the test sidecar");
}

/// Stage a tarball containing an executable `makakoo`, named for the current
/// target, under `base`. Returns its path.
async fn stage_tarball(base: &std::path::Path, work: &std::path::Path) -> std::path::PathBuf {
    let pack = work.join("pack");
    std::fs::create_dir_all(&pack).unwrap();
    let stub = pack.join("makakoo");
    std::fs::write(&stub, "#!/bin/sh\necho makakoo-stub\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(base).unwrap();
    let tarball = base.join(format!("makakoo-{}.tar.gz", current_target()));
    let st = Command::new("tar")
        .arg("-czf")
        .arg(&tarball)
        .arg("-C")
        .arg(&pack)
        .arg("makakoo")
        .status()
        .await
        .expect("tar");
    assert!(st.success(), "tar failed");
    tarball
}

#[tokio::test]
async fn remote_install_verifies_good_checksum() {
    if !bash_available().await || !curl_available().await {
        eprintln!("skipping remote_install_verifies_good_checksum: need bash + curl");
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("base");
    let install_dir = tmp.path().join("bin");

    let tarball = stage_tarball(&base, tmp.path()).await;
    // Correct sidecar in the release `<hash>  <file>` shape.
    let hash = sha256_hex(&tarball).await;
    let name = tarball.file_name().unwrap().to_string_lossy().into_owned();
    std::fs::write(
        format!("{}.sha256", tarball.display()),
        format!("{hash}  {name}\n"),
    )
    .unwrap();

    let base_url = format!("file://{}", base.display());
    let out = Command::new("bash")
        .arg(script_path())
        .env("MAKAKOO_BASE_URL", &base_url)
        .env("MAKAKOO_INSTALL_DIR", &install_dir)
        .output()
        .await
        .expect("bash spawn");
    assert!(
        out.status.success(),
        "installer should accept a matching checksum.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        install_dir.join("makakoo").exists(),
        "binary should be installed after a verified download"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("sha256 verified"),
        "expected a verification confirmation in stdout"
    );
}

#[tokio::test]
async fn remote_install_rejects_bad_checksum() {
    if !bash_available().await || !curl_available().await {
        eprintln!("skipping remote_install_rejects_bad_checksum: need bash + curl");
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("base");
    let install_dir = tmp.path().join("bin");

    let tarball = stage_tarball(&base, tmp.path()).await;
    // Deliberately wrong sidecar: 64 zeros.
    let name = tarball.file_name().unwrap().to_string_lossy().into_owned();
    std::fs::write(
        format!("{}.sha256", tarball.display()),
        format!("{}  {name}\n", "0".repeat(64)),
    )
    .unwrap();

    let base_url = format!("file://{}", base.display());
    let out = Command::new("bash")
        .arg(script_path())
        .env("MAKAKOO_BASE_URL", &base_url)
        .env("MAKAKOO_INSTALL_DIR", &install_dir)
        .output()
        .await
        .expect("bash spawn");
    assert!(
        !out.status.success(),
        "installer MUST reject a checksum mismatch (fail closed)"
    );
    assert!(
        !install_dir.join("makakoo").exists(),
        "nothing should be installed when the checksum is wrong"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("checksum mismatch"),
        "expected a 'checksum mismatch' diagnostic on stderr"
    );
}
