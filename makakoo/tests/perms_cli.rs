//! End-to-end smoke tests for `makakoo perms`.
//!
//! Phase D.6 of `MAKAKOO-OS-V0.3-USER-GRANTS`. Pairs with the
//! Python-side enforcement suite at
//! `MAKAKOO/tests/test_user_grants_enforcement.py` and the loader
//! tests at `MAKAKOO/tests/test_user_grants.py`. We drive the built
//! `makakoo` binary with a tempdir `$MAKAKOO_HOME` and assert the
//! CLI surface: list / grant / revoke / purge / audit / show, plus
//! the LD#2/LD#14/LD#15 guardrails.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn makakoo_bin() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // strip test binary name
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("makakoo")
}

fn run(args: &[&str], home: &Path) -> Output {
    Command::new(makakoo_bin())
        .args(args)
        .env("MAKAKOO_HOME", home)
        .env("HARVEY_HOME", home)
        // Isolate $HOME for validate_and_expand_scope's tilde-expand —
        // prevents the test machine's real $HOME from leaking in.
        .env("HOME", home)
        .output()
        .expect("failed to run makakoo")
}

fn ok_stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "command failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn mk_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // Realpath-normalise — macOS tempfiles live under /var/folders/...
    // which canonicalises to /private/var/... and the enforcement path
    // uses realpath, so scopes need to be stored in the resolved form.
    let real = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(real.join("config")).unwrap();
    std::fs::create_dir_all(real.join("state")).unwrap();
    std::fs::create_dir_all(real.join("logs")).unwrap();
    // Give the `TempDir` the resolved path by writing it back via the
    // returned handle — the caller uses `home()`/`path()` which returns
    // the unresolved form; we expose `resolved()` below.
    dir
}

fn home_resolved(d: &tempfile::TempDir) -> PathBuf {
    std::fs::canonicalize(d.path()).unwrap()
}

// ═══════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════

#[test]
fn empty_list_prints_no_grants_and_exits_zero() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let out = run(&["perms", "list"], &home);
    let stdout = ok_stdout(&out);
    assert!(
        stdout.contains("(no grants)"),
        "unexpected stdout:\n{stdout}"
    );
}

#[test]
fn empty_list_json_is_v033_envelope() {
    // v0.3.3 Phase C: `perms list --json` emits a structured envelope
    // `{schema_version, baseline, active, expired_today_count, all}`.
    // Previously it emitted a flat array — that shape was never
    // officially documented (docs called --json "deferred to v0.3.3"),
    // so the schema change is additive from a contract-freeze POV.
    let dir = mk_home();
    let home = home_resolved(&dir);
    let out = run(&["perms", "list", "--json"], &home);
    let stdout = ok_stdout(&out);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .expect("valid json");
    let obj = parsed.as_object().expect("top-level object");
    assert_eq!(obj["schema_version"], serde_json::json!(1));
    assert!(
        obj["baseline"].as_array().unwrap().len() >= 4,
        "baseline should include 4 canonical roots"
    );
    assert!(
        obj["active"]
            .as_array()
            .expect("active array")
            .is_empty()
    );
    assert_eq!(obj["expired_today_count"], serde_json::json!(0));
    assert_eq!(obj["all"], serde_json::json!(false));
}

#[test]
fn populated_list_json_returns_grant_records_in_active() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let target = home.join("json-zone");
    std::fs::create_dir_all(&target).unwrap();
    run(&["perms", "grant", target.to_str().unwrap(), "--for", "1h"], &home);

    let out = run(&["perms", "list", "--json"], &home);
    let stdout = ok_stdout(&out);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .expect("valid json");
    let active = parsed["active"]
        .as_array()
        .expect("active array");
    assert_eq!(active.len(), 1);
    // Grant records carry the full v0.3.3 field set — including
    // `owner`, which v0.3.3 Phase A introduced.
    let g = &active[0];
    for key in &["id", "scope", "created_at", "expires_at", "plugin", "owner"] {
        assert!(g.get(key).is_some(), "grant record missing {key}: {g:?}");
    }
    assert_eq!(
        g["plugin"], serde_json::json!("cli"),
        "CLI-created grant carries plugin='cli'"
    );
    assert_eq!(
        g["owner"], serde_json::json!("cli"),
        "owner defaults to plugin at grant time"
    );
}

#[test]
fn list_all_flag_surfaces_in_envelope() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let out = run(&["perms", "list", "--json", "--all"], &home);
    let stdout = ok_stdout(&out);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .expect("valid json");
    assert_eq!(parsed["all"], serde_json::json!(true));
    // When --all is passed, expired_today_count is 0 — everything
    // is in `active` regardless of expiry status.
    assert_eq!(parsed["expired_today_count"], serde_json::json!(0));
}

#[test]
fn grant_then_list_shows_it() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let target = home.join("workzone");
    let target_str = target.to_string_lossy().to_string();

    let grant_out = run(
        &[
            "perms",
            "grant",
            &target_str,
            "--for",
            "1h",
            "--label",
            "today",
            "--mkdir",
        ],
        &home,
    );
    let grant_id = ok_stdout(&grant_out).trim().to_string();
    assert!(grant_id.starts_with("g_"), "unexpected id: {grant_id:?}");
    assert!(target.exists(), "mkdir flag should create the dir");

    let list_out = run(&["perms", "list"], &home);
    let list = ok_stdout(&list_out);
    assert!(list.contains(&grant_id), "list missed the grant:\n{list}");
    assert!(list.contains("today"), "label missing:\n{list}");
}

#[test]
fn grant_then_revoke_removes_it() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let target = home.join("w");
    let target_str = target.to_string_lossy().to_string();

    let grant_out = run(
        &[
            "perms", "grant", &target_str, "--for", "24h", "--mkdir",
        ],
        &home,
    );
    let grant_id = ok_stdout(&grant_out).trim().to_string();

    let revoke_out = run(&["perms", "revoke", &grant_id], &home);
    assert!(revoke_out.status.success());

    let list_out = run(&["perms", "list"], &home);
    let list = ok_stdout(&list_out);
    assert!(list.contains("(no grants)"), "grants still present: {list}");

    // Audit log has BOTH entries.
    let audit_out = run(&["perms", "audit", "--json", "--since", "1h"], &home);
    let audit = ok_stdout(&audit_out);
    let entries: serde_json::Value = serde_json::from_str(audit.trim()).unwrap();
    let arr = entries.as_array().unwrap();
    let verbs: Vec<&str> = arr
        .iter()
        .filter_map(|e| e.get("verb").and_then(|v| v.as_str()))
        .collect();
    assert!(
        verbs.contains(&"perms/grant"),
        "no perms/grant entry: {verbs:?}"
    );
    assert!(
        verbs.contains(&"perms/revoke"),
        "no perms/revoke entry: {verbs:?}"
    );
}

#[test]
fn grant_slash_refused() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let out = run(&["perms", "grant", "/", "--for", "1h"], &home);
    assert!(
        !out.status.success(),
        "grant / should fail; stdout:\n{}",
        String::from_utf8_lossy(&out.stdout),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("too broad") || stderr.contains("root"),
        "stderr missing broad-scope reason:\n{stderr}"
    );
}

#[test]
fn grant_home_refused() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    for bad in ["~", "~/", "$HOME", "~/**", "$HOME/**", "*", "**"] {
        let out = run(&["perms", "grant", bad, "--for", "1h"], &home);
        assert!(
            !out.status.success(),
            "grant {bad:?} should fail — stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn grant_permanent_outside_home_requires_yes_really() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    // Use /tmp/... — outside $MAKAKOO_HOME.
    let outside = "/tmp/makakoo-perms-test-outside";
    std::fs::create_dir_all(outside).ok();

    let without = run(
        &[
            "perms", "grant", outside, "--for", "permanent",
        ],
        &home,
    );
    assert!(
        !without.status.success(),
        "permanent outside HOME should refuse without --yes-really"
    );
    let stderr = String::from_utf8_lossy(&without.stderr);
    assert!(stderr.contains("yes-really"), "{stderr}");

    let with = run(
        &[
            "perms", "grant", outside, "--for", "permanent", "--yes-really",
        ],
        &home,
    );
    assert!(
        with.status.success(),
        "permanent with --yes-really should succeed; stderr: {}",
        String::from_utf8_lossy(&with.stderr)
    );
    std::fs::remove_dir_all(outside).ok();
}

#[test]
fn grant_nonexistent_dir_warns_without_mkdir() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let target = home.join("ghost").to_string_lossy().to_string();
    let out = run(&["perms", "grant", &target, "--for", "1h"], &home);
    assert!(
        !out.status.success(),
        "nonexistent target should be refused without --mkdir"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not exist") || stderr.contains("--mkdir"),
        "{stderr}"
    );
}

#[test]
fn grant_with_mkdir_creates_dir() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let target = home.join("fresh-zone");
    assert!(!target.exists(), "pre-cond: target shouldn't exist");
    let out = run(
        &[
            "perms",
            "grant",
            &target.to_string_lossy(),
            "--for",
            "1h",
            "--mkdir",
        ],
        &home,
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(target.exists(), "mkdir should have created {target:?}");
}

#[test]
fn duration_rejects_natural_language() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let target = home.join("a");
    std::fs::create_dir_all(&target).unwrap();
    let out = run(
        &[
            "perms",
            "grant",
            &target.to_string_lossy(),
            "--for",
            "forever",
        ],
        &home,
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsupported duration") || stderr.contains("30m"),
        "{stderr}"
    );
}

#[test]
fn audit_filters_by_plugin() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let target = home.join("p");
    let target_str = target.to_string_lossy().to_string();
    // Create one grant with --plugin=cli and one with --plugin=harveychat.
    run(
        &[
            "perms", "grant", &target_str, "--for", "1h", "--mkdir",
            "--plugin", "cli",
        ],
        &home,
    );
    run(
        &[
            "perms", "grant", &target_str, "--for", "1h", "--mkdir",
            "--plugin", "harveychat",
        ],
        &home,
    );

    let out = run(
        &[
            "perms", "audit", "--json", "--since", "1h", "--plugin", "harveychat",
        ],
        &home,
    );
    let stdout = ok_stdout(&out);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().unwrap();
    assert!(!arr.is_empty(), "expected at least one matching entry");
    for entry in arr {
        assert_eq!(
            entry.get("plugin").and_then(|v| v.as_str()),
            Some("harveychat"),
            "plugin filter leaked: {entry}"
        );
    }
}

#[test]
fn show_prints_grant_detail() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let target = home.join("d");
    let target_str = target.to_string_lossy().to_string();
    let grant_id = ok_stdout(&run(
        &[
            "perms", "grant", &target_str, "--for", "1h", "--label",
            "detail-test", "--mkdir",
        ],
        &home,
    ))
    .trim()
    .to_string();

    let out = run(&["perms", "show", &grant_id], &home);
    let stdout = ok_stdout(&out);
    assert!(stdout.contains(&grant_id));
    assert!(stdout.contains("fs/write:"));
    assert!(stdout.contains("detail-test"));
}

#[test]
fn purge_is_idempotent_when_nothing_expired() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let out = run(&["perms", "purge"], &home);
    let stdout = ok_stdout(&out);
    assert!(
        stdout.contains("no expired grants"),
        "unexpected stdout:\n{stdout}"
    );
}

#[test]
fn revoke_unknown_id_fails_with_clear_message() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let out = run(&["perms", "revoke", "g_19700101_deadbeef"], &home);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no grant with id") || stderr.contains("perms list"),
        "{stderr}"
    );
}
