//! Binary smoke tests for `makakoo adapter` — Phase D.
//!
//! Spawns the compiled `makakoo` binary with a scratch `MAKAKOO_ADAPTERS_HOME`
//! and walks the common install → list → disable → remove lifecycle to
//! prove the CLI glue holds end-to-end. Complements the clap-parse unit
//! tests in `cli::tests`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn makakoo_bin() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("makakoo")
}

fn new_scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let adapters = home.path().join("adapters");
    let trust = home.path().join("trust");
    std::fs::create_dir_all(&adapters).unwrap();
    std::fs::create_dir_all(&trust).unwrap();
    (home, adapters, trust)
}

fn run_makakoo(adapters: &Path, trust: &Path, args: &[&str]) -> std::process::Output {
    Command::new(makakoo_bin())
        .args(args)
        .env("MAKAKOO_ADAPTERS_HOME", adapters)
        .env("MAKAKOO_TRUST_HOME", trust)
        .output()
        .expect("makakoo binary")
}

#[test]
fn install_from_bundled_then_list_then_remove_cycle() {
    let (_home, adapters, trust) = new_scratch();

    let out = run_makakoo(
        &adapters,
        &trust,
        &[
            "adapter",
            "install",
            "--bundled",
            "--allow-unsigned",
            "--skip-health-check",
            "openclaw",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "install failed: {}\nstderr: {}",
        stdout,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("installed openclaw"), "got {stdout}");
    assert!(adapters.join("registered/openclaw.toml").is_file());
    assert!(trust.join("adapters.json").is_file());

    // list now shows one registered row.
    let list = run_makakoo(&adapters, &trust, &["adapter", "list", "--json"]);
    let body = String::from_utf8_lossy(&list.stdout);
    assert!(body.contains("\"name\": \"openclaw\""), "got {body}");
    assert!(body.contains("\"source\": \"registered\""));

    // disable → status shows enabled=false.
    let _ = run_makakoo(&adapters, &trust, &["adapter", "disable", "openclaw"]);
    let status = run_makakoo(&adapters, &trust, &["adapter", "status", "--json"]);
    let body = String::from_utf8_lossy(&status.stdout);
    assert!(body.contains("\"enabled\": false"), "got {body}");

    // remove wipes it.
    let r = run_makakoo(&adapters, &trust, &["adapter", "remove", "openclaw"]);
    assert!(r.status.success());
    assert!(!adapters.join("registered/openclaw.toml").exists());
}

#[test]
fn spec_subcommand_dumps_schema() {
    let (_home, adapters, trust) = new_scratch();
    let out = run_makakoo(&adapters, &trust, &["adapter", "spec"]);
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(body.contains("ADAPTER_MANIFEST.md"));
    assert!(body.contains("adapter.toml"));
    assert!(body.contains("manifest_schema"));
}

#[test]
fn install_same_adapter_twice_with_drift_rejects_without_flag() {
    let (home, adapters, trust) = new_scratch();
    // Stage a local-path adapter with specific features.
    let src = home.path().join("adapter");
    std::fs::create_dir_all(&src).unwrap();
    let manifest_body = |features: &str| {
        format!(
            r#"[adapter]
name = "drift"
version = "0.1.0"
manifest_schema = 1
description = "drift-test"

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
features = {features}
supports_roles = ["validator"]

[install]
source_type = "local"

[security]
requires_network = false
sandbox_profile = "network-io"
"#,
        )
    };
    std::fs::write(src.join("adapter.toml"), manifest_body("[]")).unwrap();

    let r1 = run_makakoo(
        &adapters,
        &trust,
        &[
            "adapter",
            "install",
            src.to_str().unwrap(),
            "--skip-health-check",
        ],
    );
    assert!(r1.status.success(), "first install must succeed");

    // Drift the manifest.
    std::fs::write(src.join("adapter.toml"), manifest_body("[\"fs_write\"]")).unwrap();

    let r2 = run_makakoo(
        &adapters,
        &trust,
        &[
            "adapter",
            "install",
            src.to_str().unwrap(),
            "--skip-health-check",
        ],
    );
    assert!(!r2.status.success(), "second install must reject drift");
    let stderr = format!(
        "{}{}",
        String::from_utf8_lossy(&r2.stderr),
        String::from_utf8_lossy(&r2.stdout)
    );
    assert!(
        stderr.contains("re-trust") || stderr.contains("drift"),
        "got {stderr}"
    );

    // With the flag it goes through.
    let r3 = run_makakoo(
        &adapters,
        &trust,
        &[
            "adapter",
            "install",
            src.to_str().unwrap(),
            "--skip-health-check",
            "--accept-re-trust",
        ],
    );
    assert!(r3.status.success(), "accept-re-trust must override");
}

#[test]
fn migrate_config_emits_toml_per_provider() {
    let (home, adapters, trust) = new_scratch();
    let cfg = home.path().join("config.json");
    std::fs::write(
        &cfg,
        r#"{
  "providers": [
    {"name": "legacy-subprocess", "type": "subprocess", "command": ["echo", "{prompt}"]},
    {"name": "legacy-http", "type": "http", "url": "http://127.0.0.1:11434/v1"}
  ]
}"#,
    )
    .unwrap();

    let r = run_makakoo(
        &adapters,
        &trust,
        &["adapter", "migrate-config", cfg.to_str().unwrap()],
    );
    let body = String::from_utf8_lossy(&r.stdout);
    assert!(r.status.success(), "migrate failed: {body}");
    assert!(body.contains("legacy-subprocess"));
    assert!(body.contains("legacy-http"));

    assert!(adapters
        .join("registered/legacy-subprocess.toml")
        .is_file());
    assert!(adapters.join("registered/legacy-http.toml").is_file());
}

#[test]
fn export_produces_reinstallable_tarball() {
    let (home, adapters, trust) = new_scratch();
    // Install a bundled adapter first.
    let r1 = run_makakoo(
        &adapters,
        &trust,
        &[
            "adapter",
            "install",
            "--bundled",
            "--allow-unsigned",
            "--skip-health-check",
            "openclaw",
        ],
    );
    assert!(r1.status.success());

    let out = home.path().join("pack.tgz");
    let r2 = run_makakoo(
        &adapters,
        &trust,
        &[
            "adapter",
            "export",
            "openclaw",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(r2.status.success(), "export failed");
    assert!(out.is_file());
    let meta = std::fs::metadata(&out).unwrap();
    assert!(meta.len() > 0);
}
