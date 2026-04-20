//! Phase B Gate 1 integration test — `daemon install` → `is_installed` →
//! `daemon uninstall` → `is_installed` round trip through the
//! [`PlatformAdapter`] trait, with `$HOME` + `$MAKAKOO_HOME` redirected
//! at a tempdir so the test never touches the developer's real
//! LaunchAgents or systemd unit directory.
//!
//! Runs on macOS and Linux (native). Skipped elsewhere. The `launchctl
//! load` / `systemctl --user daemon-reload` side-effects inside
//! `daemon_install` are best-effort in the adapter, so they may log a
//! warning against the tempdir path but cannot affect the host.

#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::path::PathBuf;
use std::sync::Mutex;

use makakoo_platform::{CurrentPlatform, PlatformAdapter};

// cargo test runs integration tests in parallel by default. Both
// `HOME` and `MAKAKOO_HOME` are process-global; any test that mutates
// them must serialise.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

#[test]
fn daemon_install_status_uninstall_round_trip() {
    let _g = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();

    // Point the adapter at a tempdir so LaunchAgents / systemd user dir
    // resolves under the tempdir instead of the real $HOME.
    let _home = EnvGuard::set("HOME", dir.path());
    let _mh = EnvGuard::set("MAKAKOO_HOME", dir.path());

    let platform = CurrentPlatform::default();

    // Start clean.
    assert!(
        !platform.daemon_is_installed(),
        "tempdir $HOME should have no pre-existing daemon descriptor"
    );

    // Install.
    let descriptor = platform
        .daemon_install()
        .expect("daemon_install under tempdir $HOME");
    assert!(descriptor.exists(), "descriptor path must exist post-install");
    assert!(
        descriptor.starts_with(dir.path()),
        "descriptor {} must be under tempdir {}",
        descriptor.display(),
        dir.path().display()
    );
    assert!(platform.daemon_is_installed(), "is_installed must report true");

    // Uninstall.
    platform.daemon_uninstall().expect("daemon_uninstall");
    assert!(
        !descriptor.exists(),
        "descriptor must be gone post-uninstall"
    );
    assert!(
        !platform.daemon_is_installed(),
        "is_installed must report false post-uninstall"
    );

    // Second uninstall is idempotent — no error when already gone.
    platform
        .daemon_uninstall()
        .expect("second daemon_uninstall must be a no-op");
}

#[test]
fn default_home_is_nonempty_absolute_ish() {
    let _g = ENV_LOCK.lock().unwrap();
    let platform = CurrentPlatform::default();
    let home: PathBuf = platform.default_home();
    assert!(!home.as_os_str().is_empty());
}

#[test]
fn symlink_dir_works_under_tempdir() {
    let _g = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");

    let platform = CurrentPlatform::default();
    assert!(platform.can_symlink(), "macOS + Linux always allow symlinks");
    platform
        .symlink_dir(&target, &link)
        .expect("symlink_dir on a fresh tempdir");
    assert!(link.exists());
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
}
