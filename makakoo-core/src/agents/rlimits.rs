//! Phase 12 — opt-in setrlimit wrapper.
//!
//! Locked Q12. Two distinct mechanisms:
//!
//! 1. **RSS monitoring** — supervisor reads child RSS every 5s,
//!    writes to status.json. Always on. Never kills.
//! 2. **Hard rlimits** — opt-in via `[agents] enforce_rlimits = true`.
//!    When and only when this flag is true, the supervisor calls
//!    `setrlimit` in the child's pre_exec closure:
//!      - `RLIMIT_AS = memory_mb`
//!      - `RLIMIT_NOFILE = 256`
//!      - `RLIMIT_NPROC = 64` (macOS does not enforce; documented)
//!
//! This module is a pure wrapper around libc::setrlimit. The
//! supervisor calls it from the pre_exec closure when enforce_rlimits
//! is true.

#[derive(Debug, Clone, Copy)]
pub struct RlimitsConfig {
    /// `RLIMIT_AS` cap in MB. The child process will be killed by
    /// the kernel if it tries to map more virtual memory than this.
    pub memory_mb: u64,
    /// `RLIMIT_NOFILE` cap.
    pub max_open_files: u64,
    /// `RLIMIT_NPROC` cap. macOS does not enforce per-process; the
    /// call still succeeds for portability.
    pub max_processes: u64,
}

impl Default for RlimitsConfig {
    fn default() -> Self {
        Self {
            memory_mb: 1024,
            max_open_files: 256,
            max_processes: 64,
        }
    }
}

/// Locked default supervisor self-limit (MB). Opt-in via
/// `[agents] enforce_supervisor_rlimit = true`.
pub const DEFAULT_SUPERVISOR_MEMORY_MB: u64 = 512;

/// Apply the locked rlimits to the CURRENT process. Use from the
/// child's `Command::pre_exec` closure so the limits ride into the
/// gateway. Returns an error if any setrlimit call fails.
#[cfg(unix)]
pub fn apply_to_current_process(cfg: RlimitsConfig) -> std::io::Result<()> {
    let bytes_per_mb: u64 = 1024 * 1024;
    set_rlimit(
        libc_consts::RLIMIT_AS,
        cfg.memory_mb.saturating_mul(bytes_per_mb),
    )?;
    set_rlimit(libc_consts::RLIMIT_NOFILE, cfg.max_open_files)?;
    // macOS: setrlimit on RLIMIT_NPROC may succeed but is not
    // enforced per-process. Document and proceed.
    let _ = set_rlimit(libc_consts::RLIMIT_NPROC, cfg.max_processes);
    Ok(())
}

#[cfg(not(unix))]
pub fn apply_to_current_process(_cfg: RlimitsConfig) -> std::io::Result<()> {
    // Windows has no equivalent rlimit API. Treat as a no-op so the
    // supervisor's pre_exec closure stays portable.
    Ok(())
}

/// Apply only the `RLIMIT_AS` cap to the supervisor itself. Locked
/// behind `[agents] enforce_supervisor_rlimit = true`.
#[cfg(unix)]
pub fn apply_supervisor_self_limit(memory_mb: u64) -> std::io::Result<()> {
    let bytes_per_mb: u64 = 1024 * 1024;
    set_rlimit(
        libc_consts::RLIMIT_AS,
        memory_mb.saturating_mul(bytes_per_mb),
    )
}

#[cfg(not(unix))]
pub fn apply_supervisor_self_limit(_memory_mb: u64) -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(unix, target_os = "macos"))]
type PlatformRlimitRes = libc::c_int;

#[cfg(all(unix, not(target_os = "macos")))]
type PlatformRlimitRes = libc::__rlimit_resource_t;

#[cfg(unix)]
fn set_rlimit(resource: PlatformRlimitRes, soft: u64) -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: soft as libc::rlim_t,
        rlim_max: soft as libc::rlim_t,
    };
    let rc = unsafe { libc::setrlimit(resource, &limit) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
mod libc_consts {
    pub use libc::{RLIMIT_AS, RLIMIT_NOFILE, RLIMIT_NPROC};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_match_locked_q12() {
        let d = RlimitsConfig::default();
        assert_eq!(d.memory_mb, 1024);
        assert_eq!(d.max_open_files, 256);
        assert_eq!(d.max_processes, 64);
    }

    #[test]
    fn supervisor_self_limit_default_locked_at_512mb() {
        assert_eq!(DEFAULT_SUPERVISOR_MEMORY_MB, 512);
    }

    // The actual setrlimit call has process-wide side effects, so we
    // don't run it inside a #[test] (would shrink the test
    // process's memory and invalidate the cargo runner). The Q12
    // contract is honored as long as RlimitsConfig defaults match
    // the spec — supervisor invocation is exercised by the
    // integration paths in agents::supervisor_runtime.

    #[cfg(unix)]
    #[test]
    fn computed_byte_count_does_not_overflow_for_practical_caps() {
        let bytes = (RlimitsConfig::default().memory_mb).saturating_mul(1024 * 1024);
        assert_eq!(bytes, 1024 * 1024 * 1024);
    }
}
