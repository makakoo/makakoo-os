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
        if wake_lock_disabled() {
            return Self::noop();
        }
        use crate::PlatformAdapter;
        crate::CurrentPlatform::default().prevent_idle_sleep(reason)
    }

    /// Returns `true` when this guard owns a live OS wake-lock handle/child.
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
    /// or `systemd-inhibit`). Any spawn failure degrades silently to a no-op.
    #[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
    pub(crate) fn spawn_child(
        program: &str,
        args: &[&str],
        pipe_stdin: bool,
        kill_immediately: bool,
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

        let (stop_tx, stop_rx) = std::sync::mpsc::sync_channel::<()>(0);
        let spawned = std::thread::Builder::new()
            .name("makakoo-sleep-guard".to_string())
            .spawn(move || {
                // SAFETY: `SetThreadExecutionState` is a thread-local Win32 API
                // taking plain bitflags and returning the previous state bitmask.
                unsafe {
                    SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
                }
                let _ = stop_rx.recv();
                unsafe {
                    SetThreadExecutionState(ES_CONTINUOUS);
                }
            });

        match spawned {
            Ok(handle) => Self {
                inner: SleepGuardInner::Windows {
                    stop_tx: Some(stop_tx),
                    handle: Some(handle),
                },
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
    std::env::var("MAKAKOO_NO_WAKE_LOCK")
        .ok()
        .map(|v| {
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn noop_guard_is_inactive() {
        let g = SleepGuard::noop();
        assert!(!g.is_active());
    }

    #[test]
    fn env_opt_out_disables_wake_lock() {
        let _lock = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("MAKAKOO_NO_WAKE_LOCK").ok();
        std::env::set_var("MAKAKOO_NO_WAKE_LOCK", "1");

        let g = SleepGuard::acquire("unit-test");
        assert!(
            !g.is_active(),
            "MAKAKOO_NO_WAKE_LOCK=1 must yield no-op guard"
        );

        match prev {
            Some(v) => std::env::set_var("MAKAKOO_NO_WAKE_LOCK", v),
            None => std::env::remove_var("MAKAKOO_NO_WAKE_LOCK"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn missing_binary_degrades_silently_to_noop() {
        let g = SleepGuard::spawn_child("makakoo-nonexistent-wake-binary-xyz", &[], false, true);
        assert!(!g.is_active());
    }

    #[cfg(unix)]
    #[test]
    fn drop_reaps_child_immediately_with_zero_zombies() {
        let g = SleepGuard::spawn_child("sleep", &["60"], false, true);
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

    #[cfg(unix)]
    #[test]
    fn stdin_eof_guard_exits_cleanly_on_drop() {
        let g = SleepGuard::spawn_child("cat", &[], true, false);
        assert!(g.is_active());
        let pid = g.child_pid().expect("child pid present");
        drop(g);

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
        let _lock = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("MAKAKOO_NO_WAKE_LOCK").ok();
        std::env::remove_var("MAKAKOO_NO_WAKE_LOCK");

        let g = SleepGuard::acquire("makakoo unit test");
        assert!(g.is_active(), "macOS caffeinate should spawn cleanly");
        let pid = g.child_pid().expect("caffeinate pid present");

        // Give caffeinate a brief moment to register its IOPM assertion with powerd.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let assertions = std::process::Command::new("pmset")
            .args(["-g", "assertions"])
            .output()
            .expect("pmset runs");
        let text = String::from_utf8_lossy(&assertions.stdout);
        assert!(
            text.contains(&format!("pid {pid}(")) || text.contains("PreventUserIdleSystemSleep"),
            "expected macOS assertion while SleepGuard is live; pmset output:\n{text}"
        );

        drop(g);

        // Verify caffeinate process is completely reaped after drop.
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());

        match prev {
            Some(v) => std::env::set_var("MAKAKOO_NO_WAKE_LOCK", v),
            None => std::env::remove_var("MAKAKOO_NO_WAKE_LOCK"),
        }
    }
}
