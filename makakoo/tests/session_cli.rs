//! End-to-end smoke tests for `makakoo session`.
//!
//! v0.2 Phase G.5 — proves that the feature flag truly gates the
//! subcommand. Also walks the common lifecycle (show → label → fork →
//! export) against a real temp home, so we catch regressions in the
//! glue between `makakoo_core::session` and the CLI wrapper.

use std::process::Command;

fn makakoo_bin() -> std::path::PathBuf {
    // target/debug/makakoo lives next to the test binary.
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("makakoo")
}

fn seed_session(home: &std::path::Path) {
    // Build a minimal 3-entry session: header + user msg + assistant msg.
    let sessions = home.join("data").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let file = sessions.join("alpha.jsonl");
    let body = r#"{"type":"session","id":"alpha","created_at":"2026-04-21T00:00:00Z"}
{"type":"message","id":"m1","parentId":"alpha","role":"user","content":"hi","ts":"2026-04-21T00:00:01Z"}
{"type":"message","id":"m2","parentId":"m1","role":"assistant","content":"hello","ts":"2026-04-21T00:00:02Z"}
"#;
    std::fs::write(&file, body).unwrap();
}

fn write_kernel_toml(home: &std::path::Path, session_tree: bool) {
    let cfg = home.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("kernel.toml"),
        format!("[kernel]\nsession_tree = {session_tree}\n"),
    )
    .unwrap();
}

#[test]
fn session_list_is_gated_by_feature_flag() {
    let dir = tempfile::tempdir().unwrap();
    // No kernel.toml — feature defaults to OFF.
    seed_session(dir.path());

    let out = Command::new(makakoo_bin())
        .args(["session", "list"])
        .env("MAKAKOO_HOME", dir.path())
        .output()
        .expect("failed to run makakoo");

    // Exit code 2 → feature disabled (distinct from 1 = error).
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 when feature OFF, got {:?}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("session tree is disabled"), "{stderr}");
}

#[test]
fn session_list_works_when_flag_enabled() {
    let dir = tempfile::tempdir().unwrap();
    write_kernel_toml(dir.path(), true);
    seed_session(dir.path());

    let out = Command::new(makakoo_bin())
        .args(["session", "list"])
        .env("MAKAKOO_HOME", dir.path())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.lines().any(|l| l.trim() == "alpha"), "missing alpha: {stdout}");
}

#[test]
fn session_export_markdown_contains_session_header() {
    let dir = tempfile::tempdir().unwrap();
    write_kernel_toml(dir.path(), true);
    seed_session(dir.path());

    let out = Command::new(makakoo_bin())
        .args(["session", "export", "alpha", "--format", "markdown"])
        .env("MAKAKOO_HOME", dir.path())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("# Session `alpha`"), "{stdout}");
    assert!(stdout.contains("### user"));
    assert!(stdout.contains("### assistant"));
}

#[test]
fn session_export_html_to_file_writes_escaped_output() {
    let dir = tempfile::tempdir().unwrap();
    write_kernel_toml(dir.path(), true);
    seed_session(dir.path());
    let out_path = dir.path().join("export.html");

    let out = Command::new(makakoo_bin())
        .args([
            "session",
            "export",
            "alpha",
            "--format",
            "html",
            "--out",
            out_path.to_str().unwrap(),
        ])
        .env("MAKAKOO_HOME", dir.path())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0));
    assert!(out_path.exists());
    let body = std::fs::read_to_string(&out_path).unwrap();
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("Session <code>alpha</code>"));
}

#[test]
fn session_label_then_rewind_collapses_history() {
    let dir = tempfile::tempdir().unwrap();
    write_kernel_toml(dir.path(), true);
    seed_session(dir.path());

    // Label after m1 by crafting a direct label entry via the CLI.
    let out = Command::new(makakoo_bin())
        .args(["session", "label", "alpha", "stop-here"])
        .env("MAKAKOO_HOME", dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let label_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(label_id.starts_with("label-"), "got {label_id}");

    // Append one more message AFTER the label, so rewind has something
    // to drop. We inject directly into the JSONL since the CLI doesn't
    // expose an append-message command yet.
    let sessions = dir.path().join("data").join("sessions").join("alpha.jsonl");
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&sessions).unwrap();
        let line = format!(
            r#"{{"type":"message","id":"m3","parentId":"{label_id}","role":"user","content":"drop me","ts":"2026-04-21T00:00:10Z"}}
"#,
        );
        f.write_all(line.as_bytes()).unwrap();
    }

    let rewind = Command::new(makakoo_bin())
        .args(["session", "rewind", "alpha", "stop-here"])
        .env("MAKAKOO_HOME", dir.path())
        .output()
        .unwrap();
    assert_eq!(
        rewind.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&rewind.stderr),
    );

    // Reload the file and confirm the trailing `m3` entry is gone.
    let body = std::fs::read_to_string(&sessions).unwrap();
    assert!(!body.contains("m3"), "rewind did not drop trailing entry: {body}");
    // And a .bak file now exists alongside.
    let parent = sessions.parent().unwrap();
    let has_backup = std::fs::read_dir(parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            s.starts_with("alpha.jsonl.") && s.ends_with(".bak")
        });
    assert!(has_backup, "expected .bak file alongside alpha.jsonl");
}
