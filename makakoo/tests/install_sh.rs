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

    // Use the release build if it exists — test is idempotent either way.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let repo_root = std::path::Path::new(manifest_dir).parent().unwrap();
    let bin_rel = repo_root.join("target/release/makakoo");
    let bin_debug = repo_root.join("target/debug/makakoo");
    let bin = if bin_rel.exists() {
        bin_rel
    } else if bin_debug.exists() {
        bin_debug
    } else {
        eprintln!("skipping local_tarball_end_to_end: no makakoo binary built");
        return;
    };
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
