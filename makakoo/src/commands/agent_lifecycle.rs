//! Slot-aware lifecycle CLI: start / stop / status / restart and the
//! hidden `_supervisor` entry point.
//!
//! Routing rules:
//!
//! - If `~/MAKAKOO/config/agents/<name>.toml` exists, the name refers
//!   to a multi-bot subagent SLOT, and we route to the per-slot
//!   supervisor path (LaunchAgent on macOS / systemd-user on Linux).
//! - Otherwise, fall back to the legacy plugin entrypoint path
//!   (`crate::commands::agent`'s plugin hooks).
//!
//! `_supervisor` is the internal long-running process that LaunchAgent
//! / systemd invokes. It loads the slot config, builds the
//! `GatewayLaunchSpec`, and runs `agents::supervisor_runtime::run_supervisor`
//! in a tokio runtime.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use makakoo_core::agents::slot::slot_path;
use makakoo_core::agents::status::{GatewayStatus, SlotStatus};
use makakoo_core::agents::supervisor::{
    handle, run_dir, GatewayLaunchSpec, SupervisorState, SupervisorStatusFile,
};

use crate::context::CliContext;
use crate::output;

/// Honored env var: when set to `foreground`, `agent start <slot>`
/// runs the supervisor in the foreground (no launchd / systemd
/// registration). Used for headless containers or debugging.
pub const FOREGROUND_ENV_VAR: &str = "MAKAKOO_AGENT_SUPERVISOR";

pub fn os_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Returns true iff a slot config TOML exists for this name.
pub fn is_slot(home: &Path, name: &str) -> bool {
    slot_path(home, name).exists()
}

/// Wait for status.json to reach one of the target states, or until
/// `timeout` elapses. Returns the final observed status (None if no
/// status file ever appeared).
pub fn wait_for_state(
    home: &Path,
    slot_id: &str,
    targets: &[SupervisorState],
    timeout: Duration,
) -> Option<SupervisorStatusFile> {
    let dir = run_dir(home, slot_id);
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(s)) = SupervisorStatusFile::read(&dir) {
            if targets.contains(&s.state) {
                return Some(s);
            }
        }
        if Instant::now() >= deadline {
            return SupervisorStatusFile::read(&dir).ok().flatten();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ── start ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn start_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    use makakoo_core::agents::launchd::{
        current_uid, BootstrapError, LaunchAgentPlist, LaunchctlExec, RealLaunchctl,
    };
    let home = ctx.home();
    if !is_slot(home, slot_id) {
        output::print_error(format!("slot '{slot_id}' not found"));
        return Ok(1);
    }

    // Foreground mode escape hatch — used for headless containers
    // and debugging. Bypasses launchd entirely.
    if std::env::var(FOREGROUND_ENV_VAR).as_deref() == Ok("foreground") {
        return run_supervisor_command(ctx, slot_id);
    }

    let bin = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("read current_exe: {e}"))?;
    let plist = LaunchAgentPlist::from_slot(slot_id, &bin, &os_home(), home)
        .map_err(|e| anyhow::anyhow!("plist generation: {e}"))?;
    plist.write().map_err(|e| anyhow::anyhow!("plist write: {e}"))?;
    let launchctl = RealLaunchctl;
    let out = launchctl
        .bootstrap(current_uid(), &plist.plist_path)
        .map_err(|e| anyhow::anyhow!("launchctl bootstrap: {e}"))?;
    match BootstrapError::from_output(out, &plist.label) {
        Ok(()) => {}
        Err(e) => {
            let s = e.to_string();
            if s.contains("already loaded") {
                // Treat as success — supervisor is already up.
            } else {
                output::print_error(s);
                return Ok(1);
            }
        }
    }
    // Phase 1 exit criterion: command returns within 2s once
    // supervisor PID is in status.json. Gateway PID can take up to
    // 10s; we don't block waiting for it (status will reflect it
    // when it comes up).
    match wait_for_state(
        home,
        slot_id,
        &[
            SupervisorState::Starting,
            SupervisorState::Running,
            SupervisorState::Crashed,
        ],
        Duration::from_secs(2),
    ) {
        Some(s) => {
            let gw = s
                .gateway
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "spawning".into());
            println!(
                "{slot_id}: supervisor up (pid={}) gateway pid={}",
                s.supervisor_pid, gw
            );
            Ok(0)
        }
        None => {
            output::print_warn(format!(
                "{slot_id}: launchd bootstrap returned but no status.json in 2s — check \
                 {}/data/log/agent-{slot_id}.err.log",
                home.display()
            ));
            Ok(2)
        }
    }
}

#[cfg(target_os = "linux")]
pub fn start_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    use makakoo_core::agents::systemd::{RealSystemctl, SystemctlExec, SystemdUserUnit};
    let home = ctx.home();
    if !is_slot(home, slot_id) {
        output::print_error(format!("slot '{slot_id}' not found"));
        return Ok(1);
    }

    if std::env::var(FOREGROUND_ENV_VAR).as_deref() == Ok("foreground") {
        return run_supervisor_command(ctx, slot_id);
    }

    let bin = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("read current_exe: {e}"))?;
    let unit = SystemdUserUnit::from_slot(slot_id, &bin, &os_home(), home)
        .map_err(|e| anyhow::anyhow!("unit generation: {e}"))?;
    unit.write().map_err(|e| anyhow::anyhow!("unit write: {e}"))?;
    let s = RealSystemctl;
    let out = s
        .daemon_reload()
        .map_err(|e| anyhow::anyhow!("daemon-reload: {e}"))?;
    if out.exit_code != 0 {
        output::print_error(format!("systemctl daemon-reload failed: {}", out.stderr));
        return Ok(1);
    }
    let out = s
        .start(&unit.unit_name)
        .map_err(|e| anyhow::anyhow!("start: {e}"))?;
    if out.exit_code != 0 {
        output::print_error(format!("systemctl start failed: {}", out.stderr));
        return Ok(1);
    }
    match wait_for_state(
        home,
        slot_id,
        &[
            SupervisorState::Starting,
            SupervisorState::Running,
            SupervisorState::Crashed,
        ],
        Duration::from_secs(2),
    ) {
        Some(st) => {
            let gw = st
                .gateway
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "spawning".into());
            println!(
                "{slot_id}: supervisor up (pid={}) gateway pid={}",
                st.supervisor_pid, gw
            );
            Ok(0)
        }
        None => {
            output::print_warn(format!(
                "{slot_id}: systemctl start returned but no status.json in 2s"
            ));
            Ok(2)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn start_slot(_ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    output::print_error(format!(
        "platform not supported — `makakoo agent start {slot_id}` requires macOS launchd or \
         Linux systemd-user. Set MAKAKOO_AGENT_SUPERVISOR=foreground to run the supervisor \
         directly."
    ));
    Ok(2)
}

// ── stop ───────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn stop_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    use makakoo_core::agents::launchd::{
        current_uid, LaunchAgentPlist, LaunchctlExec, RealLaunchctl,
    };
    let home = ctx.home();
    let bin = std::env::current_exe()?;
    let plist = LaunchAgentPlist::from_slot(slot_id, &bin, &os_home(), home)
        .map_err(|e| anyhow::anyhow!("plist: {e}"))?;
    let launchctl = RealLaunchctl;
    let _ = launchctl.bootout(current_uid(), &plist.plist_path);
    // Also explicitly remove the status.json so subsequent `status`
    // does not report stale data.
    let dir = run_dir(home, slot_id);
    let _ = std::fs::remove_file(dir.join("status.json"));
    println!("{slot_id}: stopped");
    Ok(0)
}

#[cfg(target_os = "linux")]
pub fn stop_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    use makakoo_core::agents::systemd::{RealSystemctl, SystemctlExec, SystemdUserUnit};
    let home = ctx.home();
    let bin = std::env::current_exe()?;
    let unit = SystemdUserUnit::from_slot(slot_id, &bin, &os_home(), home)
        .map_err(|e| anyhow::anyhow!("unit: {e}"))?;
    let s = RealSystemctl;
    let _ = s.stop(&unit.unit_name);
    let dir = run_dir(home, slot_id);
    let _ = std::fs::remove_file(dir.join("status.json"));
    println!("{slot_id}: stopped");
    Ok(0)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn stop_slot(_ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    output::print_error(format!("platform not supported — cannot stop slot {slot_id}"));
    Ok(2)
}

// ── restart ────────────────────────────────────────────────────────

pub fn restart_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let _ = stop_slot(ctx, slot_id)?;
    // Brief settle to let launchd / systemd reap.
    std::thread::sleep(Duration::from_millis(500));
    start_slot(ctx, slot_id)
}

// ── status ─────────────────────────────────────────────────────────

pub fn status_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let home = ctx.home();
    let dir = run_dir(home, slot_id);
    match SupervisorStatusFile::read(&dir).map_err(|e| anyhow::anyhow!("status read: {e}"))? {
        Some(st) => {
            // Render via the locked Phase 4 v1 layout in
            // `SlotStatus::render_human()` so multi-bot subagents
            // share the exact same surface.
            let slot_status = SlotStatus {
                slot_id: st.slot_id.clone(),
                gateway: GatewayStatus {
                    alive: st.gateway.alive,
                    pid: st.gateway.pid,
                    last_frame_at: st.gateway.last_frame_at,
                },
                transports: st.transports.clone(),
            };
            print!("{}", slot_status.render_human());
            // Augmenting line for supervisor lifecycle visibility.
            // Goes BELOW the Phase 4 v1 block so the locked layout
            // remains pixel-stable for parsers.
            println!(
                "  state={:?} supervisor_pid={} restart_count={}",
                st.state, st.supervisor_pid, st.restart_count
            );
            Ok(if matches!(st.state, SupervisorState::Running) {
                0
            } else {
                1
            })
        }
        None => {
            println!("{slot_id}: not running (no status.json)");
            Ok(1)
        }
    }
}

// ── _supervisor (internal) ────────────────────────────────────────

/// Internal entry point invoked by LaunchAgent / systemd-user. Loads
/// the slot config, builds the gateway launch spec, runs the
/// supervisor. NOT exposed via a clap visible flag.
pub fn run_supervisor_command(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let home = ctx.home().to_path_buf();
    if !is_slot(&home, slot_id) {
        output::print_error(format!("slot '{slot_id}' not found"));
        return Ok(1);
    }

    let h = handle(slot_id);
    let dir = run_dir(&home, slot_id);

    // Phase 1: gateway is the bundled harveychat default. Phase 3
    // adds per-slot override via slot.toml [gateway] section.
    let spec = GatewayLaunchSpec::harveychat_default(&home, slot_id);

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| anyhow::anyhow!("tokio runtime: {e}"))?;
    rt.block_on(async {
        makakoo_core::agents::supervisor_runtime::run_supervisor(spec, h, dir).await
    })
    .map_err(|e| anyhow::anyhow!("supervisor: {e}"))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn ctx_for(home: &Path) -> CliContext {
        CliContext::for_home(home.to_path_buf())
    }

    #[test]
    fn is_slot_true_when_toml_exists() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config/agents");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.join("secretary.toml"), "slot_id = \"secretary\"\n").unwrap();
        assert!(is_slot(tmp.path(), "secretary"));
    }

    #[test]
    fn is_slot_false_when_toml_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_slot(tmp.path(), "missing"));
    }

    #[test]
    fn status_slot_reports_no_status_when_unsupervised() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        let rc = status_slot(&ctx, "ghost").unwrap();
        assert_eq!(rc, 1);
    }

    #[test]
    fn status_slot_reads_from_status_json() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        let dir = run_dir(tmp.path(), "secretary");
        fs::create_dir_all(&dir).unwrap();
        let snap = SupervisorStatusFile {
            slot_id: "secretary".into(),
            state: SupervisorState::Running,
            supervisor_pid: 100,
            gateway: makakoo_core::agents::status::GatewayStatus {
                alive: true,
                pid: Some(200),
                last_frame_at: None,
            },
            transports: Vec::new(),
            restart_count: 0,
            circuit_break_until: None,
            written_at: chrono::Utc::now(),
        };
        snap.write_atomic(&dir).unwrap();
        let rc = status_slot(&ctx, "secretary").unwrap();
        assert_eq!(rc, 0);
    }

    #[test]
    fn wait_for_state_returns_when_target_hit() {
        let tmp = TempDir::new().unwrap();
        let dir = run_dir(tmp.path(), "secretary");
        fs::create_dir_all(&dir).unwrap();
        let snap = SupervisorStatusFile {
            slot_id: "secretary".into(),
            state: SupervisorState::Running,
            supervisor_pid: 1,
            gateway: makakoo_core::agents::status::GatewayStatus {
                alive: true,
                pid: Some(2),
                last_frame_at: None,
            },
            transports: Vec::new(),
            restart_count: 0,
            circuit_break_until: None,
            written_at: chrono::Utc::now(),
        };
        snap.write_atomic(&dir).unwrap();
        let st = wait_for_state(
            tmp.path(),
            "secretary",
            &[SupervisorState::Running],
            Duration::from_secs(2),
        );
        assert!(st.is_some());
        assert_eq!(st.unwrap().state, SupervisorState::Running);
    }

    #[test]
    fn wait_for_state_returns_last_seen_on_timeout() {
        let tmp = TempDir::new().unwrap();
        let st = wait_for_state(
            tmp.path(),
            "ghost",
            &[SupervisorState::Running],
            Duration::from_millis(50),
        );
        assert!(st.is_none());
    }
}
