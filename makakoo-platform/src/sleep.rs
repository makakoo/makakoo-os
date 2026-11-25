//! Cross-platform scoped idle-sleep prevention (`SleepGuard`).
//!
//! Keeps the machine from entering **idle system sleep** while a long-running
//! unattended job (vector embedding, SANCHO rebuild, dream consolidation, or
//! `--keep-awake` pattern execution) is actively executing. The display may
//! still dim/sleep, and the assertion is released automatically on `Drop` or
//! process crash.
//!
//! **Global opt-out:** set `MAKAKOO_NO_WAKE_LOCK=1` (or `true`/`yes`/`on`) to
//! disable wake locks everywhere (useful on battery or in headless CI).

use std::fmt;

/// RAII guard that holds an OS idle-sleep assertion until dropped.
///
/// Construct via [`SleepGuard::acquire`] or
/// [`crate::PlatformAdapter::prevent_idle_sleep`]. Always degrades silently to
/// a no-op if `MAKAKOO_NO_WAKE_LOCK` is set, if the platform helper binary is
/// missing, or if running inside a headless container without a power daemon.
#[must_use = "SleepGuard releases the OS idle-sleep assertion immediately when dropped"]
pub struct SleepGuard {
    inner: SleepGuardInner,
}

enum SleepGuardInner {
    Noop,
    #[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
    Child {
        child: std::process::Child,
        stdin: Option<std::process::ChildStdin>,
        kill_immediately: bool,
    },
    #[cfg(target_os = "windows")]
    Windows {
        stop_tx: Option<std::sync::mpsc::SyncSender<()>>,
        handle: Option<std::thread::JoinHandle<()>>,
    },
}

impl fmt::Debug for SleepGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SleepGuard")
            .field("active", &self.is_active())
            .finish()
    }
}

impl SleepGuard {
    /// Create an inactive no-op guard.
    pub fn noop() -> Self {
        Self {
            inner: SleepGuardInner::Noop,
        }
    }

    /// Acquire a scoped idle-sleep assertion on the current platform.
    ///
    /// Returns [`SleepGuard::noop()`] when `MAKAKOO_NO_WAKE_LOCK` is enabled or
    /// when the underlying OS mechanism is unavailable.
    pub fn acquire(reason: &str) -> Self {
        Self::acquire_with_disabled(reason, wake_lock_disabled())
    }

    pub(crate) fn acquire_with_disabled(reason: &str, disabled: bool) -> Self {
        if disabled {
            return Self::noop();
        }
        use crate::PlatformAdapter;
        crate::CurrentPlatform::default().prevent_idle_sleep(reason)
    }

    /// Returns `true` when this guard holds an OS wake-lock handle/child that
    /// passed startup liveness verification.
    pub fn is_active(&self) -> bool {
        !matches!(self.inner, SleepGuardInner::Noop)
    }

    #[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
    pub(crate) fn from_child(
        child: std::process::Child,
        stdin: Option<std::process::ChildStdin>,
        kill_immediately: bool,
    ) -> Self {
        Self {
            inner: SleepGuardInner::Child {
                child,
                stdin,
                kill_immediately,
            },
        }
    }

    /// Best-effort helper to spawn a wake-lock child process (e.g. `caffeinate`
    /// or `systemd-inhibit`). Any spawn failure or immediate helper exit (such
    /// as `systemd-inhibit` in a headless container without D-Bus or where
    /// polkit denies inhibition) reaps the child immediately and degrades
    /// silently to a no-op with zero zombies.
    #[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
    pub(crate) fn spawn_child(
        program: &str,
        args: &[&str],
        pipe_stdin: bool,
        kill_immediately: bool,
        probe_early_exit: bool,
    ) -> Self {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if pipe_stdin {
            cmd.stdin(std::process::Stdio::piped());
        } else {
            cmd.stdin(std::process::Stdio::null());
        }
        match cmd.spawn() {
            Ok(mut child) => {
                // Check whether the helper exited immediately (e.g. missing
                // system D-Bus or polkit denial under systemd-inhibit). When
                // `probe_early_exit` is set, poll briefly up to 25 ms so a
                // failing helper is reaped immediately instead of lingering as
                // a zombie while `is_active()` falsely reports true.
                let polls = if probe_early_exit { 5 } else { 1 };
                for i in 0..polls {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            tracing::debug!(
                                program,
                                status = %status,
                                "wake-lock helper exited immediately; continuing without idle-sleep assertion"
                            );
                            return Self::noop();
                        }
                        Err(e) => {
                            tracing::debug!(
                                program,
                                error = %e,
                                "failed to poll wake-lock helper; reaping and continuing as no-op"
                            );
                            let _ = child.kill();
                            let _ = child.wait();
                            return Self::noop();
                        }
                        Ok(None) => {
                            if i + 1 < polls {
                                std::thread::sleep(std::time::Duration::from_millis(5));
                            }
                        }
                    }
                }
                let stdin = if pipe_stdin { child.stdin.take() } else { None };
                Self::from_child(child, stdin, kill_immediately)
            }
            Err(e) => {
                tracing::debug!(
                    program,
                    error = %e,
                    "wake-lock helper unavailable; continuing without idle-sleep assertion"
                );
                Self::noop()
            }
        }
    }

    /// Windows waiter thread keeping `ES_CONTINUOUS | ES_SYSTEM_REQUIRED` bound
    /// to one dedicated OS thread so async task migration across worker threads
    /// can never leak the thread-local Win32 execution state.
    #[cfg(target_os = "windows")]
    pub(crate) fn spawn_windows_thread() -> Self {
        use windows_sys::Win32::System::Power::{
            SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
        };

        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<bool>(1);
        let (stop_tx, stop_rx) = std::sync::mpsc::sync_channel::<()>(0);
        let spawned = std::thread::Builder::new()
            .name("makakoo-sleep-guard".to_string())
            .spawn(move || {
                // SAFETY: `SetThreadExecutionState` is a thread-local Win32 API
                // taking plain bitflags and returning the previous state bitmask
                // (or 0 on failure).
                let acquired =
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) } != 0;
                let _ = ready_tx.send(acquired);
                if !acquired {
                    tracing::debug!(
                        "SetThreadExecutionState failed to acquire wake lock; continuing as no-op"
                    );
                    return;
                }
                let _ = stop_rx.recv();
                let cleared = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                if cleared == 0 {
                    tracing::warn!(
                        "SetThreadExecutionState failed to clear wake lock on SleepGuard drop"
                    );
                }
            });

        match spawned {
            Ok(handle) => match ready_rx.recv() {
                Ok(true) => Self {
                    inner: SleepGuardInner::Windows {
                        stop_tx: Some(stop_tx),
                        handle: Some(handle),
                    },
                },
                _ => {
                    drop(stop_tx);
                    let _ = handle.join();
                    Self::noop()
                }
            },
            Err(e) => {
                tracing::debug!(error = %e, "failed to spawn Windows sleep-guard thread");
                Self::noop()
            }
        }
    }

    #[cfg(all(unix, test))]
    pub(crate) fn child_pid(&self) -> Option<u32> {
        match &self.inner {
            SleepGuardInner::Child { child, .. } => Some(child.id()),
            _ => None,
        }
    }
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        match &mut self.inner {
            SleepGuardInner::Noop => {}
            #[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
            SleepGuardInner::Child {
                child,
                stdin,
                kill_immediately,
            } => {
                drop(stdin.take());
                if !*kill_immediately {
                    // Allow stdin-EOF listeners (`systemd-inhibit ... cat`) to
                    // exit cleanly before falling back to SIGKILL.
                    for _ in 0..10 {
                        if matches!(child.try_wait(), Ok(Some(_))) {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(target_os = "windows")]
            SleepGuardInner::Windows { stop_tx, handle } => {
                drop(stop_tx.take());
                if let Some(h) = handle.take() {
                    let _ = h.join();
                }
            }
        }
    }
}

/// Returns `true` when the user opted out of wake locks via `MAKAKOO_NO_WAKE_LOCK`.
pub(crate) fn wake_lock_disabled() -> bool {
    is_disabled_value(std::env::var("MAKAKOO_NO_WAKE_LOCK").ok().as_deref())
}

pub(crate) fn is_disabled_value(raw: Option<&str>) -> bool {
    raw.map(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_guard_is_inactive() {
        let g = SleepGuard::noop();
        assert!(!g.is_active());
    }

    #[test]
    fn env_opt_out_parser_and_disabled_acquire_return_noop() {
        for truthy in ["1", "true", "TRUE", " yes ", "on", "On"] {
            assert!(is_disabled_value(Some(truthy)), "{truthy:?} must disable");
        }
        for falsy in [None, Some(""), Some("0"), Some("false"), Some("no")] {
            assert!(!is_disabled_value(falsy), "{falsy:?} must not disable");
        }
        let g = SleepGuard::acquire_with_disabled("unit-test", true);
        assert!(
            !g.is_active(),
            "disabled acquire must yield no-op guard without mutating global env"
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_binary_degrades_silently_to_noop() {
        let g = SleepGuard::spawn_child(
            "makakoo-nonexistent-wake-binary-xyz",
            &[],
            false,
            true,
            false,
        );
        assert!(!g.is_active());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn fast_exiting_helper_reaps_immediately_and_returns_noop() {
        // Mirrors systemd-inhibit exiting immediately in a headless container
        // without D-Bus or where polkit denies inhibition.
        let g = SleepGuard::spawn_child("sh", &["-c", "exit 1"], true, false, true);
        assert!(
            !g.is_active(),
            "helper that exits immediately must degrade to inactive no-op"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn drop_reaps_child_immediately_with_zero_zombies() {
        let g = SleepGuard::spawn_child("sleep", &["60"], false, true, false);
        assert!(g.is_active());
        let pid = g.child_pid().expect("child pid present");
        drop(g);

        // `ps -o stat= -p <pid>` succeeds with non-empty output if the process
        // is still alive OR lingering as a zombie ('Z'). After Drop it must be gone.
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        let stat = String::from_utf8_lossy(&out.stdout);
        assert!(
            stat.trim().is_empty(),
            "child pid {pid} lingered after Drop with stat={stat:?}"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn stdin_eof_guard_exits_cleanly_on_drop() {
        let g = SleepGuard::spawn_child("cat", &[], true, false, false);
        assert!(g.is_active());
        let pid = g.child_pid().expect("child pid present");
        let start = std::time::Instant::now();
        drop(g);
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "cat should exit cleanly on stdin EOF well before SIGKILL fallback ({elapsed:?})"
        );

        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        let stat = String::from_utf8_lossy(&out.stdout);
        assert!(
            stat.trim().is_empty(),
            "stdin-EOF child pid {pid} lingered after Drop with stat={stat:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_acquire_registers_caffeinate_and_clears_on_drop() {
        if wake_lock_disabled() {
            // Respect explicit runner opt-out without mutating global env vars.
            return;
        }
        let g = SleepGuard::acquire_with_disabled("makakoo unit test", false);
        assert!(g.is_active(), "macOS caffeinate should spawn cleanly");
        let pid = g.child_pid().expect("caffeinate pid present");
        let needle = format!("pid {pid}(");

        // Poll `pmset -g assertions` up to 500 ms for our exact child PID.
        let mut seen = false;
        let mut last_output = String::new();
        for _ in 0..20 {
            let assertions = std::process::Command::new("pmset")
                .args(["-g", "assertions"])
                .output()
                .expect("pmset runs");
            last_output = String::from_utf8_lossy(&assertions.stdout).into_owned();
            if last_output.contains(&needle) {
                seen = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(
            seen,
            "expected macOS pmset assertion for {needle:?} while SleepGuard is live; last output:\n{last_output}"
        );

        drop(g);

        // Verify caffeinate process is completely reaped after drop.
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
    }
}
