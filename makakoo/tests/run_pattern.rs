//! End-to-end smoke tests for `makakoo run pattern=NAME`.
//!
//! SPRINT-PATTERN-SUBSTRATE-V1 Phase 2. Drives the built `makakoo`
//! binary against a tempdir `$MAKAKOO_HOME` containing a seeded
//! `plugins/pattern-NAME/` directory and asserts:
//!
//!   - `--dry-run` composes pattern + strategy + mascot correctly
//!   - missing pattern errors clearly
//!   - missing system.md is surfaced
//!   - prefix tolerance (`summarize` vs `pattern-summarize`)
//!   - variable substitution
//!   - mascot-overlay composition
//!
//! No network — every test exits via `--dry-run` so switchAILocal
//! is not required.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn makakoo_bin() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("makakoo")
}

fn run(args: &[&str], home: &Path) -> Output {
    run_with_stdin(args, home, "")
}

fn run_with_stdin(args: &[&str], home: &Path, stdin_data: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(makakoo_bin())
        .args(args)
        .env("MAKAKOO_HOME", home)
        .env("HARVEY_HOME", home)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn makakoo");
    if !stdin_data.is_empty() {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stdin_data.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().expect("failed to wait on makakoo")
}

fn mk_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let real = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(real.join("plugins")).unwrap();
    std::fs::create_dir_all(real.join("data")).unwrap();
    std::fs::create_dir_all(real.join("config")).unwrap();
    std::fs::create_dir_all(real.join("state")).unwrap();
    std::fs::create_dir_all(real.join("logs")).unwrap();
    dir
}

fn home_resolved(d: &tempfile::TempDir) -> PathBuf {
    std::fs::canonicalize(d.path()).unwrap()
}

fn seed_pattern(home: &Path, name: &str, system_md: &str, extra_toml: &str) {
    let dir = home.join("plugins").join(format!("pattern-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let toml = format!(
        r#"[plugin]
name = "pattern-{name}"
version = "0.1.0"
kind = "pattern"
language = "shell"
authors = ["test"]
license = "MIT"

[source]
path = "plugins/pattern-{name}"

[pattern]
description = "test pattern {name}"
model = "test-model"
vendor = "switchailocal"
{extra_toml}

[[pattern.variables]]
name = "input"
kind = "string"
required = true
"#
    );
    std::fs::write(dir.join("plugin.toml"), toml).unwrap();
    std::fs::write(dir.join("system.md"), system_md).unwrap();
}

fn seed_mascot(home: &Path, name: &str, persona: &str) {
    let dir = home.join("plugins").join(format!("mascot-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("persona.md"), persona).unwrap();
}

fn assert_success(out: &Output) -> String {
    assert!(
        out.status.success(),
        "expected success but got {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ═══════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════

#[test]
fn dry_run_composes_pattern_only() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(&home, "summarize", "Summarize: {{input}}", "");

    let out = run(
        &[
            "run",
            "summarize",
            "--input",
            "the quick brown fox",
            "--dry-run",
        ],
        &home,
    );
    let stdout = assert_success(&out);
    assert!(stdout.contains("# pattern: pattern-summarize"));
    assert!(stdout.contains("# model: test-model"));
    assert!(stdout.contains("# vendor: switchailocal"));
    assert!(
        stdout.contains("Summarize: the quick brown fox"),
        "expected substituted system message, got:\n{stdout}"
    );
}

#[test]
fn missing_pattern_errors_with_clear_message() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let out = run(
        &["run", "ghost-pattern", "--input", "x", "--dry-run"],
        &home,
    );
    assert!(!out.status.success(), "should fail on missing pattern");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found in registry"),
        "unexpected stderr:\n{stderr}"
    );
    assert!(stderr.contains("ghost-pattern"));
}

#[test]
fn missing_system_md_skips_pattern_at_load() {
    // Phase 1 loader behavior: pattern dirs missing system.md are
    // graceful-skipped at registry walk. The CLI then reports the
    // pattern as not found.
    let dir = mk_home();
    let home = home_resolved(&dir);
    let pattern_dir = home.join("plugins").join("pattern-broken");
    std::fs::create_dir_all(&pattern_dir).unwrap();
    // plugin.toml only — no system.md.
    std::fs::write(
        pattern_dir.join("plugin.toml"),
        r#"[plugin]
name = "pattern-broken"
version = "0.1.0"
kind = "pattern"
language = "shell"

[source]
path = "plugins/pattern-broken"

[pattern]
description = "missing system.md"
"#,
    )
    .unwrap();

    let out = run(
        &["run", "broken", "--input", "x", "--dry-run"],
        &home,
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not found in registry"));
}

#[test]
fn pattern_prefix_optional() {
    // `summarize` and `pattern-summarize` resolve to the same plugin.
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(&home, "summarize", "Body: {{input}}", "");

    let bare = run(
        &["run", "summarize", "--input", "X", "--dry-run"],
        &home,
    );
    let prefixed = run(
        &["run", "pattern-summarize", "--input", "X", "--dry-run"],
        &home,
    );
    assert_success(&bare);
    assert_success(&prefixed);
    let bare_stdout = String::from_utf8_lossy(&bare.stdout);
    let prefixed_stdout = String::from_utf8_lossy(&prefixed.stdout);
    assert_eq!(bare_stdout, prefixed_stdout);
}

#[test]
fn variable_via_var_flag() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    let pattern_dir = home.join("plugins").join("pattern-greet");
    std::fs::create_dir_all(&pattern_dir).unwrap();
    std::fs::write(
        pattern_dir.join("plugin.toml"),
        r#"[plugin]
name = "pattern-greet"
version = "0.1.0"
kind = "pattern"
language = "shell"

[source]
path = "plugins/pattern-greet"

[pattern]
description = "greet someone"
model = "test"

[[pattern.variables]]
name = "name"
kind = "string"
required = true

[[pattern.variables]]
name = "input"
kind = "string"
required = false
"#,
    )
    .unwrap();
    std::fs::write(
        pattern_dir.join("system.md"),
        "Hello, {{name}}! You said: {{input}}",
    )
    .unwrap();

    let out = run(
        &[
            "run",
            "greet",
            "--var",
            "name=Alice",
            "--input",
            "hi there",
            "--dry-run",
        ],
        &home,
    );
    let stdout = assert_success(&out);
    assert!(stdout.contains("Hello, Alice!"));
    assert!(stdout.contains("You said: hi there"));
}

#[test]
fn strategy_overlay_composes_above_pattern() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(&home, "summarize", "Body: {{input}}", "");

    let out = run(
        &[
            "run",
            "summarize",
            "--input",
            "x",
            "--strategy",
            "caveman",
            "--dry-run",
        ],
        &home,
    );
    let stdout = assert_success(&out);
    // Caveman strategy text appears before the pattern body.
    let caveman_pos = stdout.find("HARD-GATE BYPASS").expect("caveman text expected");
    let body_pos = stdout.find("Body: x").expect("body expected");
    assert!(
        caveman_pos < body_pos,
        "caveman should appear before pattern body in composed system"
    );
    assert!(stdout.contains("# strategy: caveman"));
}

#[test]
fn mascot_overlay_composes_between_strategy_and_pattern() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(&home, "summarize", "Body: {{input}}", "");
    seed_mascot(&home, "olibia", "I am Olibia, the wise owl.");

    let out = run(
        &[
            "run",
            "summarize",
            "--input",
            "x",
            "--strategy",
            "cot",
            "--mascot",
            "olibia",
            "--dry-run",
        ],
        &home,
    );
    let stdout = assert_success(&out);
    let cot_pos = stdout.find("Chain of Thought").expect("strategy text");
    let mascot_pos = stdout.find("I am Olibia").expect("mascot text");
    let body_pos = stdout.find("Body: x").expect("body text");
    assert!(
        cot_pos < mascot_pos && mascot_pos < body_pos,
        "ordering should be strategy → mascot → pattern"
    );
}

#[test]
fn pattern_strategy_default_used_when_no_flag() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(
        &home,
        "auditor",
        "Audit: {{input}}",
        r#"strategy_default = "harvey-rigor""#,
    );

    let out = run(
        &["run", "auditor", "--input", "x", "--dry-run"],
        &home,
    );
    let stdout = assert_success(&out);
    assert!(stdout.contains("# strategy: harvey-rigor"));
    assert!(stdout.contains("Harvey Rigor"));
}

#[test]
fn cli_strategy_overrides_pattern_default() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(
        &home,
        "auditor",
        "Audit: {{input}}",
        r#"strategy_default = "harvey-rigor""#,
    );

    let out = run(
        &[
            "run",
            "auditor",
            "--input",
            "x",
            "--strategy",
            "cot",
            "--dry-run",
        ],
        &home,
    );
    let stdout = assert_success(&out);
    assert!(stdout.contains("# strategy: cot"));
    assert!(stdout.contains("Chain of Thought"));
    assert!(!stdout.contains("Harvey Rigor"));
}

#[test]
fn strategy_none_disables_strategy() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(
        &home,
        "auditor",
        "Audit: {{input}}",
        r#"strategy_default = "harvey-rigor""#,
    );

    let out = run(
        &[
            "run",
            "auditor",
            "--input",
            "x",
            "--strategy",
            "none",
            "--dry-run",
        ],
        &home,
    );
    let stdout = assert_success(&out);
    assert!(!stdout.contains("Harvey Rigor"));
    assert!(!stdout.contains("Chain of Thought"));
}

#[test]
fn input_from_stdin_with_dash() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(&home, "summarize", "Got: {{input}}", "");

    let out = run_with_stdin(
        &["run", "summarize", "--input", "-", "--dry-run"],
        &home,
        "from stdin",
    );
    let stdout = assert_success(&out);
    assert!(stdout.contains("Got: from stdin"));
}

#[test]
fn input_from_file_with_at_prefix() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(&home, "summarize", "Got: {{input}}", "");
    let input_file = home.join("input.txt");
    std::fs::write(&input_file, "from file").unwrap();
    let arg = format!("@{}", input_file.display());

    let out = run(
        &["run", "summarize", "--input", &arg, "--dry-run"],
        &home,
    );
    let stdout = assert_success(&out);
    assert!(stdout.contains("Got: from file"));
}

#[test]
fn missing_required_variable_errors() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(&home, "summarize", "Body: {{input}}", "");

    // No --input passed, no value for the required `input` variable.
    let out = run(&["run", "summarize", "--dry-run"], &home);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("required variable") && stderr.contains("input"),
        "unexpected stderr:\n{stderr}"
    );
}

#[test]
fn model_flag_overrides_pattern_toml() {
    let dir = mk_home();
    let home = home_resolved(&dir);
    seed_pattern(&home, "summarize", "Body: {{input}}", "");

    let out = run(
        &[
            "run",
            "summarize",
            "--input",
            "x",
            "--model",
            "gemini-2.5-flash-lite",
            "--dry-run",
        ],
        &home,
    );
    let stdout = assert_success(&out);
    assert!(stdout.contains("# model: gemini-2.5-flash-lite"));
    assert!(!stdout.contains("# model: test-model"));
}
