//! Integration tests for SkillSpector security gate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_makakoo"))
}

fn fresh_home() -> TempDir {
    TempDir::new().expect("tmp home")
}

fn write_mock_skillspector(
    tmp: &TempDir,
    stem: &str,
    json_report: &str,
    args_log: Option<&Path>,
) -> PathBuf {
    write_mock_skillspector_with_exit(tmp, stem, json_report, args_log, 0)
}

fn write_mock_skillspector_with_exit(
    tmp: &TempDir,
    stem: &str,
    json_report: &str,
    args_log: Option<&Path>,
    exit_code: i32,
) -> PathBuf {
    let py_path = tmp.path().join(format!("{stem}.py"));
    let wrapper_path = if cfg!(windows) {
        tmp.path().join(format!("{stem}.cmd"))
    } else {
        tmp.path().join(stem)
    };

    let report_literal = serde_json::to_string(json_report).unwrap();
    let log_literal = args_log
        .map(|p| serde_json::to_string(&p.to_string_lossy().to_string()).unwrap())
        .unwrap_or_else(|| "None".to_string());
    let py = format!(
        r#"import json
import sys
from pathlib import Path

report = {report_literal}
args_log = {log_literal}
args = sys.argv[1:]
if args_log:
    Path(args_log).write_text(" ".join(args), encoding="utf-8")

fmt = None
out = None
i = 0
while i < len(args):
    if args[i] == "--format" and i + 1 < len(args):
        fmt = args[i + 1]
        i += 2
        continue
    if args[i] == "--output" and i + 1 < len(args):
        out = args[i + 1]
        i += 2
        continue
    i += 1

if not out:
    sys.exit(0)
if fmt == "json":
    Path(out).write_text(report, encoding="utf-8")
elif fmt == "sarif":
    Path(out).write_text('{{"runs":[]}}', encoding="utf-8")
sys.exit({exit_code})
"#
    );
    fs::write(&py_path, py).unwrap();

    if cfg!(windows) {
        let file = py_path.file_name().unwrap().to_string_lossy();
        fs::write(
            &wrapper_path,
            format!("@echo off\r\npython \"%~dp0{file}\" %*\r\n"),
        )
        .unwrap();
    } else {
        let file = py_path.file_name().unwrap().to_string_lossy();
        fs::write(
            &wrapper_path,
            format!("#!/bin/sh\nexec python3 \"$(dirname \"$0\")/{file}\" \"$@\"\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    wrapper_path
}

fn run(home: &TempDir, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(binary_path());
    cmd.env("MAKAKOO_HOME", home.path())
        .env("MAKAKOO_FORCE_TTY", "0");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.args(args).output().expect("spawn makakoo")
}

fn write_manifest(dir: &Path, name: &str) {
    let body = format!(
        r#"
[plugin]
name = "{name}"
version = "1.0.0"
kind = "skill"
language = "python"

[source]
path = "."

[abi]
skill = "^1.0"

[entrypoint]
run = "true"
"#
    );
    fs::write(dir.join("plugin.toml"), body).unwrap();
}

#[test]
fn plugin_install_low_risk_succeeds() {
    let home = fresh_home();
    let tmp = TempDir::new().unwrap();
    let plugin_src = tmp.path().join("safe-plugin");
    fs::create_dir_all(&plugin_src).unwrap();
    write_manifest(&plugin_src, "safe-plugin");

    // Mock skillspector
    let mock_bin = write_mock_skillspector(
        &tmp,
        "mock_skillspector",
        r#"{
  "risk_assessment": {
    "score": 0,
    "severity": "LOW",
    "recommendation": "SAFE"
  },
  "issues": []
}"#,
        None,
    );

    let out = run(
        &home,
        &["plugin", "install", &plugin_src.to_string_lossy()],
        &[("MAKAKOO_TEST_SKILLSPECTOR_BIN", &mock_bin.to_string_lossy())],
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected success, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        stdout,
        stderr
    );
    assert!(
        stdout.contains("installed safe-plugin"),
        "expected installed message in stdout, got: {}",
        stdout
    );
}

#[test]
fn plugin_install_high_risk_blocked_by_default() {
    let home = fresh_home();
    let tmp = TempDir::new().unwrap();
    let plugin_src = tmp.path().join("risky-plugin");
    fs::create_dir_all(&plugin_src).unwrap();
    write_manifest(&plugin_src, "risky-plugin");

    // Mock skillspector returning HIGH risk
    let mock_bin = write_mock_skillspector(
        &tmp,
        "mock_skillspector",
        r#"{
  "risk_assessment": {
    "score": 85,
    "severity": "HIGH",
    "recommendation": "DO_NOT_INSTALL"
  },
  "issues": [
    {
      "id": "PE3",
      "severity": "HIGH",
      "category": "Credential Access",
      "location": "main.py:5"
    }
  ]
}"#,
        None,
    );

    let out = run(
        &home,
        &["plugin", "install", &plugin_src.to_string_lossy()],
        &[("MAKAKOO_TEST_SKILLSPECTOR_BIN", &mock_bin.to_string_lossy())],
    );

    let _stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected failure, got {:?}",
        out.status
    );
    assert!(
        stderr.contains("SkillSpector flagged this plugin: HIGH 85/100"),
        "expected security error in stderr, got: {}",
        stderr
    );
    assert!(
        stderr.contains("Install blocked pending explicit override"),
        "expected override warning in stderr, got: {}",
        stderr
    );
}

#[test]
fn plugin_install_nonzero_report_is_risk_block_not_scan_error() {
    let home = fresh_home();
    let tmp = TempDir::new().unwrap();
    let plugin_src = tmp.path().join("risky-plugin");
    fs::create_dir_all(&plugin_src).unwrap();
    write_manifest(&plugin_src, "risky-plugin");

    // Real SkillSpector can exit non-zero when it has produced a valid
    // DO_NOT_INSTALL report. That must flow into the risk policy gate, not
    // become an infrastructure "scan error" that hides the report.
    let mock_bin = write_mock_skillspector_with_exit(
        &tmp,
        "mock_skillspector_nonzero",
        r#"{
  "risk_assessment": {
    "score": 100,
    "severity": "CRITICAL",
    "recommendation": "DO_NOT_INSTALL"
  },
  "issues": [
    {
      "id": "PE3",
      "severity": "CRITICAL",
      "category": "Credential Access",
      "location": "main.py:5"
    }
  ]
}"#,
        None,
        1,
    );

    let out = run(
        &home,
        &["plugin", "install", &plugin_src.to_string_lossy()],
        &[("MAKAKOO_TEST_SKILLSPECTOR_BIN", &mock_bin.to_string_lossy())],
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected risk block, got success\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("SkillSpector flagged this plugin: CRITICAL 100/100"),
        "expected risk policy block in stderr, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("SkillSpector scan error"),
        "valid nonzero report should not be classified as scan infra error: {}",
        stderr
    );
}

#[test]
fn plugin_install_high_risk_with_override_succeeds() {
    let home = fresh_home();
    let tmp = TempDir::new().unwrap();
    let plugin_src = tmp.path().join("risky-plugin");
    fs::create_dir_all(&plugin_src).unwrap();
    write_manifest(&plugin_src, "risky-plugin");

    // Mock skillspector returning HIGH risk
    let mock_bin = write_mock_skillspector(
        &tmp,
        "mock_skillspector",
        r#"{
  "risk_assessment": {
    "score": 85,
    "severity": "HIGH",
    "recommendation": "DO_NOT_INSTALL"
  },
  "issues": [
    {
      "id": "PE3",
      "severity": "HIGH",
      "category": "Credential Access",
      "location": "main.py:5"
    }
  ]
}"#,
        None,
    );

    // 1) Test allow_risk without risk_ack -> fails
    let out_no_ack = run(
        &home,
        &[
            "plugin",
            "install",
            &plugin_src.to_string_lossy(),
            "--allow-risk",
        ],
        &[("MAKAKOO_TEST_SKILLSPECTOR_BIN", &mock_bin.to_string_lossy())],
    );
    assert!(!out_no_ack.status.success());
    assert!(
        String::from_utf8_lossy(&out_no_ack.stderr)
            .contains("--allow-risk requires a non-empty --risk-ack explanation")
    );

    // 2) Test allow_risk with risk_ack -> succeeds
    let out = run(
        &home,
        &[
            "plugin",
            "install",
            &plugin_src.to_string_lossy(),
            "--allow-risk",
            "--risk-ack",
            "reviewed reports",
        ],
        &[("MAKAKOO_TEST_SKILLSPECTOR_BIN", &mock_bin.to_string_lossy())],
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected success with override, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        stdout,
        stderr
    );
    assert!(
        stdout.contains("installed risky-plugin"),
        "expected installed message in stdout, got: {}",
        stdout
    );

    // Companion file exists and says override: true
    let metadata_path = home
        .path()
        .join("state")
        .join("plugin-risk")
        .join("risky-plugin.json");
    assert!(metadata_path.exists());
    let meta_content = fs::read_to_string(metadata_path).unwrap();
    let meta: serde_json::Value = serde_json::from_str(&meta_content).unwrap();
    assert_eq!(meta["override"].as_bool(), Some(true));
    assert_eq!(meta["override_ack"].as_str(), Some("reviewed reports"));
}

#[test]
fn plugin_install_no_skill_scan_policy() {
    let home = fresh_home();
    let tmp = TempDir::new().unwrap();
    let plugin_src = tmp.path().join("local-plugin");
    fs::create_dir_all(&plugin_src).unwrap();
    write_manifest(&plugin_src, "local-plugin");

    // 1) Local path with --no-skill-scan -> succeeds with warning
    let out = run(
        &home,
        &[
            "plugin",
            "install",
            &plugin_src.to_string_lossy(),
            "--no-skill-scan",
        ],
        &[],
    );
    let _stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "expected local path skip to succeed");
    assert!(stderr.contains("Skipping security scan for local path install as requested."));

    // 2) Remote git source with --no-skill-scan -> blocked at CLI validation
    let out_git = run(
        &home,
        &[
            "plugin",
            "install",
            "git+https://github.com/example/plugin@v1.0",
            "--no-skill-scan",
        ],
        &[],
    );
    assert!(!out_git.status.success());
    assert!(
        String::from_utf8_lossy(&out_git.stderr)
            .contains("--no-skill-scan is only allowed for local path installs")
    );
}

#[test]
fn skill_audit_all_scans_fleet() {
    let home = fresh_home();
    let tmp = TempDir::new().unwrap();
    let path = tmp.path();

    // Create an installed plugin directory inside the mock home
    let p1 = home.path().join("plugins").join("p1");
    fs::create_dir_all(&p1).unwrap();
    write_manifest(&p1, "p1");

    // Create a local skill directory inside the cwd/temp directory
    let local_skill = path.join("local-skill");
    fs::create_dir_all(&local_skill).unwrap();
    fs::write(local_skill.join("SKILL.md"), "").unwrap();

    // Create a vendor directory which should be skipped
    let vendor = path.join("node_modules");
    fs::create_dir_all(&vendor).unwrap();
    fs::write(vendor.join("SKILL.md"), "").unwrap();

    // Mock skillspector returning LOW/SAFE risk
    let mock_bin = write_mock_skillspector(
        &tmp,
        "mock_skillspector",
        r#"{
  "risk_assessment": {
    "score": 10,
    "severity": "LOW",
    "recommendation": "SAFE"
  },
  "issues": [
    {
      "id": "PE1",
      "severity": "LOW",
      "category": "Credential Access",
      "location": "SKILL.md:5"
    }
  ]
}"#,
        None,
    );

    let mut cmd = Command::new(binary_path());
    cmd.env("MAKAKOO_HOME", home.path())
        .env("MAKAKOO_FORCE_TTY", "0")
        .env("MAKAKOO_TEST_SKILLSPECTOR_BIN", &mock_bin)
        .env("MAKAKOO_TEST_NO_USER_SKILL_ROOTS", "1")
        .current_dir(path)
        .args(&["skill", "audit", "--all", "--limit", "5"]);

    let out = cmd.output().expect("spawn makakoo");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "expected success, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        stdout,
        stderr
    );
    assert!(
        stdout.contains("Total targets scanned: 2"),
        "expected 2 targets, got: {}",
        stdout
    );
    assert!(
        stdout.contains("Findings: CRITICAL 0, HIGH 0, MEDIUM 0, LOW 2"),
        "expected 2 low findings, got: {}",
        stdout
    );

    // Check dated fleet reports
    let report_root = home
        .path()
        .join("data")
        .join("reports")
        .join("skillspector");
    let mut found_json = false;
    let mut found_md = false;
    for entry in fs::read_dir(&report_root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found_json |= path.join("fleet-summary.json").exists();
            found_md |= path.join("fleet-summary.md").exists();
        }
    }
    assert!(
        found_json,
        "missing fleet-summary.json under {}",
        report_root.display()
    );
    assert!(
        found_md,
        "missing fleet-summary.md under {}",
        report_root.display()
    );
}

#[test]
fn skill_audit_llm_flag_behavior() {
    let home = fresh_home();
    let tmp = TempDir::new().unwrap();
    let audit_target = tmp.path().join("audit-target");
    fs::create_dir_all(&audit_target).unwrap();
    fs::write(audit_target.join("SKILL.md"), "").unwrap();

    let args_log = tmp.path().join("args.log");

    // Mock logs all arguments and writes a dummy JSON report.
    let mock_bin = write_mock_skillspector(
        &tmp,
        "mock_skillspector",
        r#"{
  "risk_assessment": {
    "score": 5,
    "severity": "LOW",
    "recommendation": "SAFE"
  },
  "issues": []
}"#,
        Some(&args_log),
    );

    // 1. Run skill audit without --llm -> should log `--no-llm`
    let out1 = run(
        &home,
        &[
            "skill",
            "audit",
            &audit_target.to_string_lossy(),
            "--no-cache",
        ],
        &[("MAKAKOO_TEST_SKILLSPECTOR_BIN", &mock_bin.to_string_lossy())],
    );
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    let stderr1 = String::from_utf8_lossy(&out1.stderr);

    if !args_log.exists() {
        panic!(
            "args_log does not exist!\nstdout: {}\nstderr: {}",
            stdout1, stderr1
        );
    }
    let log1 = fs::read_to_string(&args_log).unwrap();
    assert!(
        log1.contains("--no-llm"),
        "expected --no-llm to be passed by default, args were: {}",
        log1
    );

    // Clear args.log
    fs::remove_file(&args_log).unwrap();

    // 2. Run skill audit with --llm -> should NOT log `--no-llm`
    let out2 = run(
        &home,
        &[
            "skill",
            "audit",
            &audit_target.to_string_lossy(),
            "--llm",
            "--no-cache",
        ],
        &[("MAKAKOO_TEST_SKILLSPECTOR_BIN", &mock_bin.to_string_lossy())],
    );
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    let stderr2 = String::from_utf8_lossy(&out2.stderr);

    if !args_log.exists() {
        panic!(
            "args_log does not exist for second run!\nstdout: {}\nstderr: {}",
            stdout2, stderr2
        );
    }
    let log2 = fs::read_to_string(&args_log).unwrap();
    assert!(
        !log2.contains("--no-llm"),
        "expected --no-llm to be omitted when --llm is passed, args were: {}",
        log2
    );
}
