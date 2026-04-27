//! `makakoo plugin start|stop|status|restart` for service-kind plugins.
//!
//! Phase A₀.3 of MAKAKOO-OS-V0.7-S3-ENDPOINT.
//!
//! Drives the built `makakoo` binary against a tempdir `$MAKAKOO_HOME`.
//! Installs a hand-rolled "dummy service" plugin that just touches a
//! marker file on start, removes it on stop, and reports health by
//! checking marker existence — three shell commands, no daemon.

#![cfg(unix)]

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
    Command::new(makakoo_bin())
        .args(args)
        .env("MAKAKOO_HOME", home)
        .env("HARVEY_HOME", home)
        .env("HOME", home)
        .output()
        .expect("failed to run makakoo")
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "command failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Wait up to ~2s for `pred` to flip true. Service plugins are
/// backgrounded — `plugin start` returns before the spawned shell
/// has actually executed its body. Polling beats `sleep(N)` because
/// fast machines don't pay the full N.
fn poll_until<F: Fn() -> bool>(pred: F, label: &str) {
    use std::thread;
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if pred() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("poll_until timed out: {label}");
}

fn write_plugin_source(src_root: &Path, marker: &Path) {
    std::fs::create_dir_all(src_root).unwrap();
    let toml = format!(
        r#"
[plugin]
name = "dummy-service"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "."

[entrypoint]
start = "/usr/bin/touch {marker}"
stop = "/bin/rm -f {marker}"
health = "/bin/test -f {marker}"

[service]
restart_policy = "never"
"#,
        marker = marker.display(),
    );
    std::fs::write(src_root.join("plugin.toml"), toml).unwrap();
}

#[test]
fn service_lifecycle_start_status_stop() {
    let dir = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(dir.path()).unwrap();

    let src_root = home.parent().unwrap().join("dummy-svc-src");
    let _ = std::fs::remove_dir_all(&src_root);
    let marker = home.join("dummy-svc.alive");
    write_plugin_source(&src_root, &marker);

    // Install via the path source — using --core would force a
    // different resolution path that doesn't apply here.
    let install = run(
        &["plugin", "install", src_root.to_str().unwrap()],
        &home,
    );
    let _ = ok(&install);

    // Pre-start: marker absent → status reports down (rc != 0).
    let pre = run(&["plugin", "status", "dummy-service"], &home);
    let pre_stdout = String::from_utf8_lossy(&pre.stdout).to_string();
    assert!(!pre.status.success(), "pre-start status: {pre_stdout}");
    assert!(pre_stdout.contains("down"), "pre-start stdout: {pre_stdout}");

    // Start: marker file should appear (backgrounded — poll).
    let start = run(&["plugin", "start", "dummy-service"], &home);
    let start_stdout = ok(&start);
    assert!(
        start_stdout.contains("started"),
        "start stdout: {start_stdout}"
    );
    poll_until(|| marker.exists(), "marker creation after start");

    // Status: up.
    let status = run(&["plugin", "status", "dummy-service"], &home);
    let status_stdout = ok(&status);
    assert!(status_stdout.contains("up"), "status stdout: {status_stdout}");

    // Stop.
    let stop = run(&["plugin", "stop", "dummy-service"], &home);
    let _ = ok(&stop);
    poll_until(|| !marker.exists(), "marker removed after stop");

    // Status: down again.
    let post = run(&["plugin", "status", "dummy-service"], &home);
    assert!(
        !post.status.success(),
        "post-stop status should report down, stdout: {}",
        String::from_utf8_lossy(&post.stdout)
    );

    let _ = std::fs::remove_dir_all(&src_root);
}

#[test]
fn service_restart_cycles_marker() {
    let dir = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(dir.path()).unwrap();

    let src_root = home.parent().unwrap().join("dummy-restart-src");
    let _ = std::fs::remove_dir_all(&src_root);
    let marker = home.join("dummy-restart.alive");
    // Distinct plugin name so the two #[test]s in this file can't
    // race on the same registry entry.
    let toml = format!(
        r#"
[plugin]
name = "dummy-restart"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "."

[entrypoint]
start = "/usr/bin/touch {marker}"
stop = "/bin/rm -f {marker}"
health = "/bin/test -f {marker}"
"#,
        marker = marker.display(),
    );
    std::fs::create_dir_all(&src_root).unwrap();
    std::fs::write(src_root.join("plugin.toml"), &toml).unwrap();

    let _ = ok(&run(
        &["plugin", "install", src_root.to_str().unwrap()],
        &home,
    ));
    let _ = ok(&run(&["plugin", "start", "dummy-restart"], &home));
    poll_until(|| marker.exists(), "marker creation after start");

    // Restart should drop marker (stop) and recreate it (start).
    let restart = run(&["plugin", "restart", "dummy-restart"], &home);
    let _ = ok(&restart);
    poll_until(|| marker.exists(), "marker re-created after restart");

    let _ = run(&["plugin", "stop", "dummy-restart"], &home);
    let _ = std::fs::remove_dir_all(&src_root);
}

#[test]
fn rejects_lifecycle_on_skill_kind() {
    // Skill-kind plugins should refuse start/stop/status/restart with a
    // friendly error — this surface is service- and agent-only.
    let dir = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(dir.path()).unwrap();

    let src_root = home.parent().unwrap().join("dummy-skill-src");
    let _ = std::fs::remove_dir_all(&src_root);
    let toml = r#"
[plugin]
name = "dummy-skill"
version = "0.1.0"
kind = "skill"
language = "shell"

[source]
path = "."

[abi]
skill = "^1.0"

[entrypoint]
run = "/bin/true"
"#;
    std::fs::create_dir_all(&src_root).unwrap();
    std::fs::write(src_root.join("plugin.toml"), toml).unwrap();

    let _ = ok(&run(
        &["plugin", "install", src_root.to_str().unwrap()],
        &home,
    ));
    let out = run(&["plugin", "start", "dummy-skill"], &home);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "expected failure, stderr: {stderr}");
    assert!(
        stderr.contains("kind = skill") || stderr.contains("only supported"),
        "expected friendly kind-mismatch error, stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&src_root);
}
