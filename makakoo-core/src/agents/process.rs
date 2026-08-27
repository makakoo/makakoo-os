//! Small process-liveness primitives shared by agent lifecycle surfaces.

#[cfg(unix)]
use std::process::Command;

/// Same-user process metadata used for bounded lifecycle tree cleanup.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub state: String,
    pub command: String,
}

/// Returns whether `pid` currently names a live process visible to this user.
/// This is a liveness check, not a process-identity proof.
#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Return the command line only when `pid` belongs to the current Unix user.
/// Lifecycle callers combine this with exact argument checks before signalling
/// a PID recovered from status.json, closing the PID-reuse hazard.
#[cfg(unix)]
pub fn command_for_current_user(pid: u32) -> Option<String> {
    if !pid_is_alive(pid) {
        return None;
    }
    let output = Command::new("/bin/ps")
        .args([
            "-ww",
            "-p",
            &pid.to_string(),
            "-o",
            "uid=",
            "-o",
            "command=",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8(output.stdout).ok()?;
    let line = line.trim();
    let split = line.find(char::is_whitespace)?;
    let uid: u32 = line[..split].parse().ok()?;
    if uid != unsafe { libc::geteuid() } {
        return None;
    }
    let command = line[split..].trim();
    (!command.is_empty()).then(|| command.to_string())
}

/// Snapshot all visible processes owned by the current Unix user. Lifecycle
/// code uses this when status.json does not exist yet or was corrupted.
#[cfg(unix)]
pub fn processes_for_current_user() -> Vec<(u32, String)> {
    process_table_for_current_user()
        .into_iter()
        .map(|process| (process.pid, process.command))
        .collect()
}

/// Snapshot PID, parent PID, and command for visible processes owned by the
/// current user. Parent relationships let lifecycle cleanup terminate a legacy
/// gateway's descendants even when it did not create a process group.
#[cfg(unix)]
pub fn process_table_for_current_user() -> Vec<ProcessInfo> {
    let output = match Command::new("/bin/ps")
        .args([
            "-ww", "-axo", "uid=", "-o", "pid=", "-o", "ppid=", "-o", "state=", "-o", "command=",
        ])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    let Ok(body) = String::from_utf8(output.stdout) else {
        return Vec::new();
    };
    let current_uid = unsafe { libc::geteuid() };
    body.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let uid: u32 = fields.next()?.parse().ok()?;
            let pid: u32 = fields.next()?.parse().ok()?;
            let parent_pid: u32 = fields.next()?.parse().ok()?;
            let state = fields.next()?.to_string();
            let command = fields.collect::<Vec<_>>().join(" ");
            (uid == current_uid && !command.is_empty()).then_some(ProcessInfo {
                pid,
                parent_pid,
                state,
                command,
            })
        })
        .collect()
}

/// Return the current working directory only for a process owned by this user.
/// Failing to prove the directory returns `None`; callers must fail closed.
#[cfg(target_os = "linux")]
pub fn cwd_for_current_user(pid: u32) -> Option<std::path::PathBuf> {
    command_for_current_user(pid)?;
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(target_os = "linux")]
pub fn executable_for_current_user(pid: u32) -> Option<std::path::PathBuf> {
    command_for_current_user(pid)?;
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn cwd_for_current_user(pid: u32) -> Option<std::path::PathBuf> {
    command_for_current_user(pid)?;
    let output = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix('n').map(std::path::PathBuf::from))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn executable_for_current_user(pid: u32) -> Option<std::path::PathBuf> {
    command_for_current_user(pid)?;
    let output = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "txt", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix('n').map(std::path::PathBuf::from))
}

#[cfg(not(unix))]
pub fn command_for_current_user(_pid: u32) -> Option<String> {
    None
}

#[cfg(not(unix))]
pub fn processes_for_current_user() -> Vec<(u32, String)> {
    Vec::new()
}

#[cfg(not(unix))]
pub fn cwd_for_current_user(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

#[cfg(not(unix))]
pub fn executable_for_current_user(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

/// Returns whether `pid` currently names a live process visible to this user.
/// This is a liveness check, not a process-identity proof.
#[cfg(windows)]
pub fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return false;
    }
    // Query-only access: OpenProcess fails for dead PIDs and for processes
    // this user may not inspect — both count as "not alive" here.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut exit_code = 0u32;
    let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
    unsafe {
        CloseHandle(handle);
    }
    ok != 0 && exit_code == STILL_ACTIVE as u32
}

/// No portable liveness primitive exists outside unix/windows. Report dead
/// (with this doc comment as the honesty clause) so lifecycle callers fail
/// open on a PID they could never verify, instead of treating every nonzero
/// PID as alive and blocking cleanup forever.
#[cfg(not(any(unix, windows)))]
pub fn pid_is_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive_and_invalid_pids_are_not() {
        // The non-unix/non-windows fallback stub reports dead for everything.
        #[cfg(any(unix, windows))]
        assert!(pid_is_alive(std::process::id()));
        assert!(!pid_is_alive(0));
        assert!(!pid_is_alive(u32::MAX));
    }

    #[cfg(unix)]
    #[test]
    fn current_process_command_requires_same_user() {
        let command = command_for_current_user(std::process::id()).unwrap();
        assert!(!command.is_empty());
        assert!(command_for_current_user(u32::MAX).is_none());
        assert!(processes_for_current_user()
            .iter()
            .any(|(pid, command)| *pid == std::process::id() && !command.is_empty()));
        assert!(process_table_for_current_user().iter().any(|process| {
            process.pid == std::process::id()
                && process.parent_pid > 0
                && !process.state.is_empty()
                && !process.command.is_empty()
        }));
        assert!(cwd_for_current_user(std::process::id()).is_some());
        assert!(executable_for_current_user(std::process::id()).is_some());
    }
}
