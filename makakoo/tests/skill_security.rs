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
    let mock_bin = tmp.path().join("mock_skillspector");
    let script = r#"#!/bin/sh
format="$4"
output="$6"
if [ "$format" = "json" ]; then
  cat <<EOF > "$output"
{
  "risk_assessment": {
    "score": 0,
    "severity": "LOW",
    "recommendation": "SAFE"
  },
  "issues": []
}
EOF
elif [ "$format" = "sarif" ]; then
  echo '{"runs":[]}' > "$output"
fi
"#;
    fs::write(&mock_bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mock_bin, fs::Permissions::from_mode(0o755)).unwrap();
    }

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
    let mock_bin = tmp.path().join("mock_skillspector");
    let script = r#"#!/bin/sh
format="$4"
output="$6"
if [ "$format" = "json" ]; then
  cat <<EOF > "$output"
{
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
}
EOF
elif [ "$format" = "sarif" ]; then
  echo '{"runs":[]}' > "$output"
fi
"#;
    fs::write(&mock_bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mock_bin, fs::Permissions::from_mode(0o755)).unwrap();
    }

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
fn plugin_install_high_risk_with_override_succeeds() {
    let home = fresh_home();
    let tmp = TempDir::new().unwrap();
    let plugin_src = tmp.path().join("risky-plugin");
    fs::create_dir_all(&plugin_src).unwrap();
    write_manifest(&plugin_src, "risky-plugin");

    // Mock skillspector returning HIGH risk
    let mock_bin = tmp.path().join("mock_skillspector");
    let script = r#"#!/bin/sh
format="$4"
output="$6"
if [ "$format" = "json" ]; then
  cat <<EOF > "$output"
{
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
}
EOF
elif [ "$format" = "sarif" ]; then
  echo '{"runs":[]}' > "$output"
fi
"#;
    fs::write(&mock_bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mock_bin, fs::Permissions::from_mode(0o755)).unwrap();
    }

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
    assert!(String::from_utf8_lossy(&out_no_ack.stderr)
        .contains("--allow-risk requires a non-empty --risk-ack explanation"));

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
    assert!(String::from_utf8_lossy(&out_git.stderr)
        .contains("--no-skill-scan is only allowed for local path installs"));
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
    let mock_bin = tmp.path().join("mock_skillspector");
    let script = r#"#!/bin/sh
format=""
output=""
for arg in "$@"; do
  if [ "$arg" = "json" ] || [ "$arg" = "sarif" ]; then
    format="$arg"
  fi
done
# In CLI invocation: format is passed after --format and output is passed after --output
# Let's parse args to find --format and --output correctly
while [ $# -gt 0 ]; do
  case "$1" in
    --format) format="$2"; shift ;;
    --output) output="$2"; shift ;;
  esac
  shift
done

if [ "$format" = "json" ]; then
  cat <<EOF > "$output"
{
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
}
EOF
elif [ "$format" = "sarif" ]; then
  echo '{"runs":[]}' > "$output"
fi
"#;
    fs::write(&mock_bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mock_bin, fs::Permissions::from_mode(0o755)).unwrap();
    }

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

    let mock_bin = tmp.path().join("mock_skillspector");
    let args_log = tmp.path().join("args.log");

    // We write a shell script that logs all its arguments to args_log
    // and writes a dummy JSON report to the requested output path
    let script = format!(
        r#"#!/bin/sh
# Log all args
echo "$@" > "{}"

format=""
output=""
while [ $# -gt 0 ]; do
  case "$1" in
    --format) format="$2"; shift ;;
    --output) output="$2"; shift ;;
  esac
  shift
done

if [ "$format" = "json" ]; then
  cat <<EOF > "$output"
{{
  "risk_assessment": {{
    "score": 5,
    "severity": "LOW",
    "recommendation": "SAFE"
  }},
  "issues": []
}}
EOF
elif [ "$format" = "sarif" ]; then
  echo '{{"runs":[]}}' > "$output"
fi
"#,
        args_log.to_string_lossy()
    );
    fs::write(&mock_bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mock_bin, fs::Permissions::from_mode(0o755)).unwrap();
    }

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
