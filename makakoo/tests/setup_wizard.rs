//! End-to-end smoke tests for the `makakoo setup` binary.
//!
//! These tests spawn the real binary (built by cargo) against an isolated
//! `$MAKAKOO_HOME` so they verify the clap layer, the dispatcher, and the
//! state file writer all stitch together. They intentionally stay fast
//! by running in non-interactive mode and asserting on stdout shape.
//!
//! Phase 1 exercises only the persona section. Phases 2-6 will extend
//! this file with scenarios for each new section.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn binary_path() -> PathBuf {
    // cargo puts the compiled bin at target/debug/makakoo (or release/ if
    // built with --release). The env var CARGO_BIN_EXE_<name> points to
    // the right one for the current test profile.
    PathBuf::from(env!("CARGO_BIN_EXE_makakoo"))
}

fn fresh_home() -> TempDir {
    TempDir::new().expect("tmp home")
}

fn run(home: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new(binary_path())
        .env("MAKAKOO_HOME", home.path())
        .env("MAKAKOO_FORCE_TTY", "0") // all tests are non-interactive
        .args(args)
        .output()
        .expect("spawn makakoo")
}

#[test]
fn setup_help_lists_section_positional_and_flags() {
    let home = fresh_home();
    let out = run(&home, &["setup", "--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "--help must exit 0; got {:?}", out.status);
    assert!(stdout.contains("SECTION"), "help should mention positional SECTION");
    assert!(stdout.contains("--only"), "help should mention --only");
    assert!(stdout.contains("--skip"), "help should mention --skip");
    assert!(stdout.contains("--non-interactive"), "help should mention --non-interactive");
    assert!(stdout.contains("--reset"), "help should mention --reset");
    assert!(stdout.contains("--force"), "help should mention --force");
}

#[test]
fn setup_non_interactive_on_fresh_home_reports_persona_not_started() {
    let home = fresh_home();
    let out = run(&home, &["setup", "--non-interactive"]);
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("persona") && stdout.contains("not-started"),
        "expected persona not-started in stdout, got: {stdout}"
    );
}

#[test]
fn setup_without_tty_and_without_flag_hints_user() {
    let home = fresh_home();
    let out = run(&home, &["setup"]);
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("not running on a live terminal"),
        "expected TTY-gate hint, got: {stdout}"
    );
}

#[test]
fn setup_unknown_positional_exits_nonzero() {
    let home = fresh_home();
    let out = run(&home, &["setup", "ghost"]);
    assert!(!out.status.success(), "unknown section must exit nonzero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown section"), "stderr should explain; got: {stderr}");
}

#[test]
fn setup_unknown_only_exits_nonzero() {
    let home = fresh_home();
    let out = run(&home, &["setup", "--only", "ghost"]);
    assert!(!out.status.success(), "unknown --only must exit nonzero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown section in --only"));
}

#[test]
fn setup_reset_wipes_state_file() {
    let home = fresh_home();
    // Seed state
    let state_dir = home.path().join("state").join("makakoo-setup");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("completed.json");
    fs::write(
        &state_path,
        r#"{"version":1,"sections":{"persona":{"status":"Completed","at":"2026-04-23T12:00:00Z"}}}"#,
    )
    .unwrap();
    assert!(state_path.exists());

    let out = run(&home, &["setup", "--reset", "--non-interactive"]);
    assert!(out.status.success());
    assert!(!state_path.exists(), "state file should be wiped");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Reset: wiped"));
}

#[test]
fn setup_only_filters_to_single_section() {
    let home = fresh_home();
    let out = run(&home, &["setup", "--only", "persona", "--non-interactive"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("persona"));
}

#[test]
fn setup_skip_single_section_leaves_remaining() {
    let home = fresh_home();
    let out = run(&home, &["setup", "--skip", "persona", "--non-interactive"]);
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("brain"), "brain should still run; got: {stdout}");
    assert!(!stdout.contains("persona"), "persona should be filtered out; got: {stdout}");
}

#[test]
fn setup_skip_every_registered_section_errors() {
    let home = fresh_home();
    let out = run(
        &home,
        &[
            "setup",
            "--skip",
            "persona,brain,cli-agent,terminal,model-provider,infect",
            "--non-interactive",
        ],
    );
    // "skip everything" = no sections matched → error path (exit 1).
    // This is deliberate: it surfaces a user mistake rather than silently doing nothing.
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no sections matched"));
}

#[test]
fn setup_terminal_section_is_reachable() {
    // On macOS, terminal section is applicable; elsewhere the summary shows (n/a).
    // Either way, `setup terminal --non-interactive` must exit 0 and not crash.
    let home = fresh_home();
    let out = run(&home, &["setup", "terminal", "--non-interactive"]);
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("terminal"));
}

#[test]
fn setup_cli_agent_status_detects_pi_missing() {
    let home = fresh_home();
    // Use an isolated PATH that shouldn't contain `pi`. System PATH may
    // still have it on Sebastian's dev machine, so we deliberately nuke
    // PATH for this probe.
    let out = Command::new(binary_path())
        .env("MAKAKOO_HOME", home.path())
        .env("MAKAKOO_FORCE_TTY", "0")
        .env("PATH", "/var/empty")
        .args(["setup", "cli-agent", "--non-interactive"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("cli-agent"));
    assert!(
        stdout.contains("not-started"),
        "expected not-started when pi absent; got: {stdout}"
    );
}

#[test]
fn setup_brain_non_interactive_on_fresh_home_reports_not_started() {
    let home = fresh_home();
    let out = run(&home, &["setup", "brain", "--non-interactive"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("brain"));
    assert!(stdout.contains("not-started"));
}

#[test]
fn setup_brain_detects_existing_multi_source_config() {
    let home = fresh_home();
    let cfg_dir = home.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("brain_sources.json"),
        r#"{"default":"default","sources":[
            {"name":"default","type":"logseq","path":"x"},
            {"name":"obsidian","type":"obsidian","path":"y"}
        ]}"#,
    )
    .unwrap();
    let out = run(&home, &["setup", "brain", "--non-interactive"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("already-satisfied"),
        "expected already-satisfied when multi-source registered; got: {stdout}"
    );
}
