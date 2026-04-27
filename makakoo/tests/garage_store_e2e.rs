//! End-to-end integration: garage-store plugin install → start →
//! bootstrap → aws-cli ls → stop. Phase A₁.7 of
//! `MAKAKOO-OS-V0.7-S3-ENDPOINT`.
//!
//! This test exercises the **real** Garage subprocess via brew + the
//! **real** macOS Keychain. CI machines without Garage / aws-cli /
//! macOS Keychain skip cleanly with an `eprintln!` rather than failing.
//! Sebastian's local box has all three; commit-time CI gates the path.
//!
//! Marker that this test ran in earnest: `cargo test garage_store_e2e
//! -- --nocapture` should print "garage-store E2E: live".

#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::process::{Command, Output};

fn makakoo_bin() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("makakoo")
}

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("spawn failed")
}

fn cmd_exists(name: &str) -> bool {
    Command::new("/usr/bin/which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn install_start_bootstrap_aws_ls_stop_cycle() {
    if !cmd_exists("garage") {
        eprintln!("garage-store E2E skipped: `garage` not on PATH (brew install garage)");
        return;
    }
    if !cmd_exists("aws") {
        eprintln!("garage-store E2E skipped: `aws` not on PATH");
        return;
    }
    if !cmd_exists("security") {
        eprintln!("garage-store E2E skipped: `security` (Keychain) not available");
        return;
    }

    eprintln!("garage-store E2E: live");

    // Tempdir as $MAKAKOO_HOME — install + config land there. The
    // plist lands in ~/Library/LaunchAgents/ regardless (launchd
    // requires absolute paths it controls).
    let dir = tempfile::tempdir().expect("tempdir");
    let home = std::fs::canonicalize(dir.path()).unwrap();
    let plugins_core = workspace_root().join("plugins-core");

    let env = |c: &mut Command| {
        c.env("MAKAKOO_HOME", &home)
            .env("HARVEY_HOME", &home)
            .env("MAKAKOO_PLUGINS_CORE", &plugins_core);
    };

    // 1. install. install.sh seeds garage.toml + plist.
    let mut c = Command::new(makakoo_bin());
    c.args(["plugin", "install", "--core", "garage-store"]);
    env(&mut c);
    let out = run(&mut c);
    assert!(
        out.status.success(),
        "plugin install failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(home.join("config/garage.toml").exists());
    let plist = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join("Library/LaunchAgents/com.makakoo.garage.plist");
    assert!(plist.exists(), "plist not seeded at {}", plist.display());

    // 2. start. launchctl load. Garage takes a moment to bind ports.
    let mut c = Command::new(makakoo_bin());
    c.args(["plugin", "start", "garage-store"]);
    env(&mut c);
    let out = run(&mut c);
    assert!(out.status.success(), "plugin start failed");

    // Wait for admin port to become responsive (200 or 503 — both
    // mean Garage is alive; 503 just means quorum not yet, which is
    // expected pre-bootstrap).
    let mut alive = false;
    for _ in 0..40 {
        let probe = Command::new("/usr/bin/curl")
            .args([
                "-sS",
                "-m",
                "1",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "http://127.0.0.1:3903/health",
            ])
            .output()
            .unwrap();
        let code = String::from_utf8_lossy(&probe.stdout);
        if !code.starts_with("000") && code.trim().len() == 3 {
            alive = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert!(alive, "Garage admin port did not respond within 20s");

    // 3. bootstrap.
    let mut c = Command::new(makakoo_bin());
    c.args(["s3", "bootstrap"]);
    env(&mut c);
    let out = run(&mut c);
    assert!(
        out.status.success(),
        "s3 bootstrap failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // 4. Pull creds from keychain, run `aws s3 ls`.
    let creds_out = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", "makakoo", "-a", "makakoo-s3-service", "-w"])
        .output()
        .expect("security spawn");
    assert!(
        creds_out.status.success(),
        "keychain entry not found after bootstrap"
    );
    let creds_json = String::from_utf8_lossy(&creds_out.stdout).trim().to_string();
    let creds: serde_json::Value = serde_json::from_str(&creds_json).unwrap();
    let access = creds["access_key"].as_str().unwrap();
    let secret = creds["secret_key"].as_str().unwrap();
    assert!(access.starts_with("GK"), "unexpected access key shape");
    assert!(!secret.is_empty());

    let ls = Command::new("aws")
        .args([
            "--endpoint-url",
            "http://127.0.0.1:3900",
            "--region",
            "garage",
            "s3",
            "ls",
        ])
        .env("AWS_ACCESS_KEY_ID", access)
        .env("AWS_SECRET_ACCESS_KEY", secret)
        .output()
        .expect("aws spawn");
    assert!(
        ls.status.success(),
        "aws s3 ls failed: {}",
        String::from_utf8_lossy(&ls.stderr)
    );

    // 5. status reports up.
    let mut c = Command::new(makakoo_bin());
    c.args(["plugin", "status", "garage-store"]);
    env(&mut c);
    let out = run(&mut c);
    assert!(out.status.success(), "plugin status reported down");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("up"), "status not 'up': {stdout}");

    // 6. stop. Subsequent connect should fail.
    let mut c = Command::new(makakoo_bin());
    c.args(["plugin", "stop", "garage-store"]);
    env(&mut c);
    let _ = run(&mut c);

    // launchctl unload is async; give it a moment.
    std::thread::sleep(std::time::Duration::from_secs(2));

    let post_ls = Command::new("aws")
        .args([
            "--endpoint-url",
            "http://127.0.0.1:3900",
            "--region",
            "garage",
            "s3",
            "ls",
        ])
        .env("AWS_ACCESS_KEY_ID", access)
        .env("AWS_SECRET_ACCESS_KEY", secret)
        .output()
        .expect("aws spawn");
    assert!(
        !post_ls.status.success(),
        "aws s3 ls should fail after stop, but it succeeded"
    );
}
