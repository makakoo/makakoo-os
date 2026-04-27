//! Test-only helpers shared across section tests.
//!
//! `$PATH` is a process-global env var; multiple unit tests mutating it
//! concurrently will race and corrupt each other. `PathGuard` holds a
//! single shared mutex so every PATH-mutating test across every module
//! in this crate runs sequentially and restores PATH on drop (even on
//! panic). Module-local mutexes are not enough — they only serialize
//! within one file.

use std::sync::Mutex;

/// One-mutex-per-process: every PATH-mutating test in this crate holds
/// the same lock, so they never run concurrently regardless of cargo's
/// default threaded test runner.
static PATH_MUTEX: Mutex<()> = Mutex::new(());

/// RAII guard: swap $PATH on construction, restore on drop.
pub struct PathGuard {
    prev: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl PathGuard {
    pub fn new(new_path: &str) -> Self {
        // If a previous test panicked while holding the lock the mutex
        // is poisoned; take it anyway so serialization still works.
        let lock = PATH_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("PATH").ok();
        std::env::set_var("PATH", new_path);
        Self { prev, _lock: lock }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.prev {
            std::env::set_var("PATH", p);
        } else {
            std::env::remove_var("PATH");
        }
    }
}

/// Write an executable shim at `dir/name` that:
/// - records its args (one per line) to `dir/name.log`,
/// - prints `stdout_content` to stdout,
/// - exits with `exit_code`.
///
/// Used to stub `npm`, `brew`, `which`, `pi`, etc. without needing any
/// mocking framework.
pub fn shim(
    dir: &std::path::Path,
    name: &str,
    exit_code: i32,
    stdout_content: &str,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let log = dir.join(format!("{name}.log"));
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {log}\nprintf '{stdout_content}'\nexit {exit_code}\n",
        log = log.display(),
        stdout_content = stdout_content,
        exit_code = exit_code,
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Read a shim's recorded args, one per line.
pub fn shim_args(dir: &std::path::Path, name: &str) -> Vec<String> {
    let log = dir.join(format!("{name}.log"));
    if !log.exists() {
        return vec![];
    }
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}
