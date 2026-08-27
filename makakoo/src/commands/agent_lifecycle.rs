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

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;
use makakoo_core::agents::slot::checked_slot_path;
use makakoo_core::agents::status::{GatewayStatus, SlotStatus};
use makakoo_core::agents::supervisor::{
    checked_run_dir, handle, SupervisorState, SupervisorStatusFile,
};

use crate::context::CliContext;
use crate::output;

/// Honored env var: when set to `foreground`, `agent start <slot>`
/// runs the supervisor in the foreground (no launchd / systemd
/// registration). Used for headless containers or debugging.
pub const FOREGROUND_ENV_VAR: &str = "MAKAKOO_AGENT_SUPERVISOR";

fn foreground_requested() -> bool {
    foreground_requested_from(std::env::var(FOREGROUND_ENV_VAR).ok().as_deref())
}

fn foreground_requested_from(value: Option<&str>) -> bool {
    value == Some("foreground")
}

pub fn os_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Returns true iff a slot config TOML exists for this name.
pub fn is_slot(home: &Path, name: &str) -> bool {
    checked_slot_path(home, name)
        .map(|path| path.exists())
        .unwrap_or(false)
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
    wait_for_state_with_process_check(home, slot_id, targets, timeout, supervisor_process_matches)
}

fn wait_for_state_with_process_check(
    home: &Path,
    slot_id: &str,
    targets: &[SupervisorState],
    timeout: Duration,
    mut process_matches: impl FnMut(u32, &str) -> bool,
) -> Option<SupervisorStatusFile> {
    let dir = checked_run_dir(home, slot_id).ok()?;
    let deadline = Instant::now() + timeout;
    let mut last_live = None;
    loop {
        if let Ok(Some(s)) = SupervisorStatusFile::read(&dir) {
            if process_matches(s.supervisor_pid, slot_id) {
                last_live = Some(s.clone());
            }
            if targets.contains(&s.state) && process_matches(s.supervisor_pid, slot_id) {
                return Some(s);
            }
        }
        if Instant::now() >= deadline {
            return last_live;
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
    preflight_slot(home, slot_id)?;
    if supervisor_already_running(home, slot_id)? {
        println!("{slot_id}: already running");
        return Ok(0);
    }

    // Foreground mode escape hatch — used for headless containers
    // and debugging. Bypasses launchd entirely.
    if foreground_requested() {
        return run_supervisor_command(ctx, slot_id);
    }

    let bin = std::env::current_exe().map_err(|e| anyhow::anyhow!("read current_exe: {e}"))?;
    let plist = LaunchAgentPlist::from_slot(slot_id, &bin, &os_home(), home)
        .map_err(|e| anyhow::anyhow!("plist generation: {e}"))?;
    plist
        .write()
        .map_err(|e| anyhow::anyhow!("plist write: {e}"))?;
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
    preflight_slot(home, slot_id)?;
    if supervisor_already_running(home, slot_id)? {
        println!("{slot_id}: already running");
        return Ok(0);
    }

    if foreground_requested() {
        return run_supervisor_command(ctx, slot_id);
    }

    let bin = std::env::current_exe().map_err(|e| anyhow::anyhow!("read current_exe: {e}"))?;
    let unit = SystemdUserUnit::from_slot(slot_id, &bin, &os_home(), home)
        .map_err(|e| anyhow::anyhow!("unit generation: {e}"))?;
    unit.write()
        .map_err(|e| anyhow::anyhow!("unit write: {e}"))?;
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
pub fn start_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    if !is_slot(ctx.home(), slot_id) {
        output::print_error(format!("slot '{slot_id}' not found"));
        return Ok(1);
    }
    preflight_slot(ctx.home(), slot_id)?;
    if supervisor_already_running(ctx.home(), slot_id)? {
        println!("{slot_id}: already running");
        return Ok(0);
    }
    if foreground_requested() {
        return run_supervisor_command(ctx, slot_id);
    }
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
    makakoo_core::agents::validate_slot_id(slot_id)?;
    let bin = std::env::current_exe()?;
    let plist = LaunchAgentPlist::from_slot(slot_id, &bin, &os_home(), home)
        .map_err(|e| anyhow::anyhow!("plist: {e}"))?;
    if !plist.plist_path.exists() {
        if status_has_live_process(home, slot_id)? {
            signal_foreground_supervisor(home, slot_id)?;
        }
        return finish_stop(home, slot_id, 0, "");
    }
    let launchctl = RealLaunchctl;
    let mut result = launchctl.bootout(current_uid(), &plist.plist_path)?;
    if result.exit_code != 0
        && known_inactive(&result.stderr)
        && status_has_live_process(home, slot_id)?
    {
        signal_foreground_supervisor(home, slot_id)?;
        result.exit_code = 0;
    }
    let rc = finish_stop(home, slot_id, result.exit_code, &result.stderr)?;
    if rc == 0 {
        remove_service_artifact(&plist.plist_path)?;
    }
    Ok(rc)
}

#[cfg(target_os = "linux")]
pub fn stop_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    use makakoo_core::agents::systemd::{RealSystemctl, SystemctlExec, SystemdUserUnit};
    let home = ctx.home();
    makakoo_core::agents::validate_slot_id(slot_id)?;
    let bin = std::env::current_exe()?;
    let unit = SystemdUserUnit::from_slot(slot_id, &bin, &os_home(), home)
        .map_err(|e| anyhow::anyhow!("unit: {e}"))?;
    if !unit.unit_path.exists() {
        if status_has_live_process(home, slot_id)? {
            signal_foreground_supervisor(home, slot_id)?;
        }
        return finish_stop(home, slot_id, 0, "");
    }
    let s = RealSystemctl;
    let mut result = s.stop(&unit.unit_name)?;
    if result.exit_code != 0
        && known_inactive(&result.stderr)
        && status_has_live_process(home, slot_id)?
    {
        signal_foreground_supervisor(home, slot_id)?;
        result.exit_code = 0;
    }
    let rc = finish_stop(home, slot_id, result.exit_code, &result.stderr)?;
    if rc == 0 {
        remove_service_artifact(&unit.unit_path)?;
        match s.daemon_reload() {
            Ok(reload) if reload.exit_code != 0 => output::print_warn(format!(
                "slot is stopped and its unit was removed, but systemctl daemon-reload failed: {}",
                reload.stderr
            )),
            Err(error) => output::print_warn(format!(
                "slot is stopped and its unit was removed, but systemctl daemon-reload failed: {error}"
            )),
            Ok(_) => {}
        }
    }
    Ok(rc)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn stop_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let home = ctx.home();
    makakoo_core::agents::validate_slot_id(slot_id)?;
    // No launchd / systemd here: the only way a supervisor can be alive is a
    // foreground run (MAKAKOO_AGENT_SUPERVISOR=foreground), which holds the
    // canonical runtime lock. Never claim a stop we cannot prove — refuse
    // while the lock (or any detectable live process) says otherwise, so
    // `agent destroy` cannot archive files out from under a live supervisor.
    if status_has_live_process(home, slot_id)? {
        output::print_error(format!(
            "{slot_id}: supervisor still owns the runtime lock and this platform has no \
             service manager to stop it — terminate the foreground supervisor process, \
             then retry"
        ));
        return Ok(1);
    }
    // Offline / generated-only slot: nothing can be running, so the shared
    // cleanup path (quiescent lock + status removal) is safe to reuse.
    finish_stop(home, slot_id, 0, "")
}

// ── restart ────────────────────────────────────────────────────────

pub fn restart_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let stop_rc = stop_slot(ctx, slot_id)?;
    if stop_rc != 0 {
        return Ok(stop_rc);
    }
    // Brief settle to let launchd / systemd reap.
    std::thread::sleep(Duration::from_millis(500));
    start_slot(ctx, slot_id)
}

// ── status ─────────────────────────────────────────────────────────

pub fn status_slot(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    status_slot_with_process_check(ctx, slot_id, supervisor_process_matches)
}

fn status_slot_with_process_check(
    ctx: &CliContext,
    slot_id: &str,
    process_matches: impl Fn(u32, &str) -> bool,
) -> anyhow::Result<i32> {
    let home = ctx.home();
    let dir = checked_run_dir(home, slot_id)?;
    match SupervisorStatusFile::read(&dir).map_err(|e| anyhow::anyhow!("status read: {e}"))? {
        Some(st) => {
            if st.slot_id != slot_id {
                anyhow::bail!(
                    "status file slot '{}' does not match requested slot '{}'",
                    st.slot_id,
                    slot_id
                );
            }
            // Prefer the supervisor-recorded executable when present (see
            // supervisor_snapshot_matches); the injected process check stays
            // the fallback for pre-0.3.0 snapshots.
            let alive = if st.supervisor_exe.is_some() {
                supervisor_snapshot_matches(&st)
            } else {
                process_matches(st.supervisor_pid, slot_id)
            };
            if !alive {
                remove_status_file(&dir)?;
                println!(
                    "{slot_id}: not running (removed stale status for supervisor pid {})",
                    st.supervisor_pid
                );
                return Ok(1);
            }
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
            remove_status_file(&dir)?;
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
    makakoo_core::agents::validate_slot_id(slot_id)?;
    if !is_slot(&home, slot_id) {
        output::print_error(format!("slot '{slot_id}' not found"));
        return Ok(1);
    }

    let h = handle(slot_id);
    let dir = checked_run_dir(&home, slot_id)?;
    let _supervisor_lock = match acquire_supervisor_lock(&dir) {
        Ok(lock) => lock,
        Err(error) if lock_is_contended(&error) => {
            output::print_warn(format!(
                "{slot_id}: supervisor already owns the runtime lock; duplicate start ignored"
            ));
            return Ok(0);
        }
        Err(error) => return Err(anyhow::anyhow!("supervisor lock: {error}")),
    };
    cleanup_orphaned_legacy_gateway(&home, slot_id, &dir)?;

    // Phase 4: load slot config and resolve effective LLM so the
    // supervisor can propagate MAKAKOO_LLM_* env to the gateway.
    let slot_path = checked_slot_path(&home, slot_id)?;
    let slot_cfg = makakoo_core::agents::slot::AgentSlot::load_from_file(&slot_path)
        .map_err(|e| anyhow::anyhow!("slot load: {e}"))?;
    let defaults = makakoo_core::agents::llm_override::LlmDefaults::builtin_fallback();
    let over = slot_cfg.llm.as_ref().and_then(|s| s.effective_override());
    let eff = makakoo_core::agents::llm_override::resolve_effective(over.as_ref(), &defaults);
    let spec = crate::commands::agent_runtime::launch_spec(&home, &slot_cfg, &eff)?;

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            makakoo_core::agents::supervisor_runtime::run_supervisor(spec, h, dir).await
        })
    })
    .map_err(|e| anyhow::anyhow!("supervisor: {e}"))?;
    Ok(0)
}

fn preflight_slot(home: &Path, slot_id: &str) -> anyhow::Result<()> {
    let path = checked_slot_path(home, slot_id)?;
    let slot = makakoo_core::agents::AgentSlot::load_from_file(&path)
        .map_err(|e| anyhow::anyhow!("slot load: {}", e))?;
    crate::commands::agent_runtime::preflight(&slot)
}

fn finish_stop(home: &Path, slot_id: &str, exit_code: i32, stderr: &str) -> anyhow::Result<i32> {
    let dir = checked_run_dir(home, slot_id)?;
    if exit_code != 0 && !known_inactive(stderr) {
        output::print_error(format!(
            "{slot_id}: service-manager stop failed (exit {exit_code}): {}",
            stderr.trim()
        ));
        return Ok(1);
    }

    // Acquire and hold the canonical lock while the final process-table sweep
    // runs. This covers both the status-less exec→lock startup window and an
    // orphaned gateway whose supervisor/status file already disappeared.
    let _shutdown_guard = match acquire_quiescent_supervisor_lock(&dir, slot_id) {
        Ok(guard) => guard,
        Err(error) => {
            output::print_error(error.to_string());
            return Ok(1);
        }
    };
    if let Err(error) = terminate_matching_gateways(home, slot_id) {
        output::print_error(error.to_string());
        return Ok(1);
    }
    remove_status_file(&dir)?;
    if exit_code == 0 {
        println!("{slot_id}: stopped");
    } else {
        println!("{slot_id}: already stopped (service manager exit {exit_code})");
    }
    Ok(0)
}

fn status_has_live_process(home: &Path, slot_id: &str) -> anyhow::Result<bool> {
    let dir = checked_run_dir(home, slot_id)?;
    if supervisor_lock_held(&dir)?
        || !matching_supervisor_pids(slot_id).is_empty()
        || !matching_gateway_pids(home, slot_id).is_empty()
    {
        return Ok(true);
    }
    let status = SupervisorStatusFile::read(&dir)
        .map_err(|error| anyhow::anyhow!("status read before stop: {error}"))?;
    Ok(status.is_some_and(|snapshot| {
        snapshot.slot_id == slot_id
            && (supervisor_snapshot_matches(&snapshot)
                || snapshot
                    .gateway
                    .pid
                    .is_some_and(|pid| gateway_process_matches(home, slot_id, pid)))
    }))
}

/// Supervisor identity check for a status.json snapshot. When the supervisor
/// recorded its own executable path (v0.3.0+), match against that instead of
/// the CLI's `current_exe` — after an in-place upgrade the two differ while
/// the old supervisor keeps running.
#[cfg(unix)]
fn supervisor_snapshot_matches(snapshot: &SupervisorStatusFile) -> bool {
    match snapshot.supervisor_exe.as_deref() {
        Some(recorded) => supervisor_process_matches_recorded(
            snapshot.supervisor_pid,
            &snapshot.slot_id,
            recorded,
        ),
        None => supervisor_process_matches(snapshot.supervisor_pid, &snapshot.slot_id),
    }
}

#[cfg(not(unix))]
fn supervisor_snapshot_matches(_snapshot: &SupervisorStatusFile) -> bool {
    false
}

fn supervisor_already_running(home: &Path, slot_id: &str) -> anyhow::Result<bool> {
    let dir = checked_run_dir(home, slot_id)?;
    Ok(supervisor_lock_held(&dir)? || status_has_live_process(home, slot_id)?)
}

fn acquire_supervisor_lock(dir: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("supervisor.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.try_lock_exclusive()?;
    Ok(file)
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || (cfg!(windows) && error.raw_os_error() == Some(33))
}

fn acquire_quiescent_supervisor_lock(dir: &Path, slot_id: &str) -> anyhow::Result<File> {
    let deadline = Instant::now() + Duration::from_secs(7);
    loop {
        signal_matching_supervisors(slot_id, false);
        match acquire_supervisor_lock(dir) {
            Ok(lock) => {
                // A process observed before it acquired the lock now loses the
                // singleton race. Wait for that process to exit as well.
                let startup_deadline = Instant::now() + Duration::from_secs(2);
                while !matching_supervisor_pids(slot_id).is_empty()
                    && Instant::now() < startup_deadline
                {
                    signal_matching_supervisors(slot_id, false);
                    std::thread::sleep(Duration::from_millis(50));
                }
                if !matching_supervisor_pids(slot_id).is_empty() {
                    signal_matching_supervisors(slot_id, true);
                    std::thread::sleep(Duration::from_millis(100));
                }
                let remaining = matching_supervisor_pids(slot_id);
                if !remaining.is_empty() {
                    anyhow::bail!(
                        "{slot_id}: supervisor process(es) {remaining:?} survived shutdown; refusing cleanup"
                    );
                }
                return Ok(lock);
            }
            Err(error) if lock_is_contended(&error) => {
                if Instant::now() >= deadline {
                    signal_matching_supervisors(slot_id, true);
                    std::thread::sleep(Duration::from_millis(100));
                    let lock = acquire_supervisor_lock(dir).map_err(|lock_error| {
                        anyhow::anyhow!(
                            "{slot_id}: supervisor owns runtime lock after shutdown grace: {lock_error}"
                        )
                    })?;
                    let remaining = matching_supervisor_pids(slot_id);
                    if !remaining.is_empty() {
                        anyhow::bail!(
                            "{slot_id}: supervisor process(es) {remaining:?} survived forced shutdown"
                        );
                    }
                    return Ok(lock);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(anyhow::anyhow!("supervisor lock: {error}")),
        }
    }
}

pub(crate) fn acquire_destroy_guard(home: &Path, slot_id: &str) -> anyhow::Result<File> {
    let dir = checked_run_dir(home, slot_id)?;
    let guard = acquire_quiescent_supervisor_lock(&dir, slot_id)?;
    terminate_matching_gateways(home, slot_id)?;
    Ok(guard)
}

pub(crate) fn destroy_guard_is_held(dir: &Path) -> std::io::Result<bool> {
    supervisor_lock_held(dir)
}

fn supervisor_lock_held(dir: &Path) -> std::io::Result<bool> {
    let path = dir.join("supervisor.lock");
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(&file)?;
            Ok(false)
        }
        Err(error) if lock_is_contended(&error) => Ok(true),
        Err(error) => Err(error),
    }
}

fn remove_service_artifact(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!(
            "remove service artifact {}: {error}",
            path.display()
        )),
    }
}

#[cfg(unix)]
fn signal_foreground_supervisor(home: &Path, slot_id: &str) -> anyhow::Result<()> {
    let dir = checked_run_dir(home, slot_id)?;
    let startup_pids = matching_supervisor_pids(slot_id);
    if !startup_pids.is_empty() {
        signal_processes(&startup_pids, libc::SIGTERM, false);
        return Ok(());
    }
    if !supervisor_lock_held(&dir)? {
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = SupervisorStatusFile::read(&dir)
            .map_err(|error| anyhow::anyhow!("status read before signal: {error}"))?
        {
            if status.slot_id != slot_id {
                anyhow::bail!(
                    "refusing to signal supervisor: status belongs to '{}'",
                    status.slot_id
                );
            }
            if !supervisor_snapshot_matches(&status) {
                anyhow::bail!(
                    "refusing to signal supervisor pid {}: process identity does not match slot '{}'",
                    status.supervisor_pid,
                    slot_id
                );
            }
            let rc = unsafe { libc::kill(status.supervisor_pid as i32, libc::SIGTERM) };
            if rc != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(anyhow::anyhow!(
                        "signal foreground supervisor {}: {error}",
                        status.supervisor_pid
                    ));
                }
            }
            return Ok(());
        }
        if !supervisor_lock_held(&dir)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "supervisor owns runtime lock but status is unavailable for slot '{slot_id}'"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn supervisor_process_matches(pid: u32, slot_id: &str) -> bool {
    let Some(command) = makakoo_core::agents::process::command_for_current_user(pid) else {
        return false;
    };
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    supervisor_command_matches(pid, &command, slot_id, &current_exe)
}

/// Variant used when status.json recorded the supervisor's own executable
/// path. After an in-place upgrade the running supervisor still executes the
/// OLD binary, so comparing against the CLI's `current_exe` would misreport a
/// live supervisor as dead; the recorded path is the same generation as the
/// running process and stays the correct reference.
#[cfg(unix)]
fn supervisor_process_matches_recorded(pid: u32, slot_id: &str, recorded_exe: &Path) -> bool {
    let Some(command) = makakoo_core::agents::process::command_for_current_user(pid) else {
        return false;
    };
    supervisor_command_matches(pid, &command, slot_id, recorded_exe)
}

#[cfg(unix)]
fn supervisor_command_matches(pid: u32, command: &str, slot_id: &str, expected_exe: &Path) -> bool {
    if !supervisor_argv_matches(command, slot_id, expected_exe) {
        return false;
    }
    let Some(process_exe) = makakoo_core::agents::process::executable_for_current_user(pid) else {
        return false;
    };
    exe_paths_match(&process_exe, expected_exe)
}

/// Pure argv-structure check, separated from the live-process exe lookup for
/// testability. `expected_exe` doubles as the reference for resolving a
/// whitespace-containing argv0.
#[cfg(unix)]
fn supervisor_argv_matches(command: &str, slot_id: &str, expected_exe: &Path) -> bool {
    let args_suffix = format!(" agent _supervisor --slot {slot_id}");
    let tokens: Vec<_> = command.split_whitespace().collect();
    // Fast path: exactly `<argv0> agent _supervisor --slot <id>`. Covers
    // PATH-style invocations where argv0 is a bare name.
    if tokens.len() == 5 && tokens[1..] == ["agent", "_supervisor", "--slot", slot_id] {
        return true;
    }
    // argv0 itself contains whitespace (e.g. `/opt/My Apps/makakoo`):
    // split the known argument suffix off and prove the remaining
    // invocation path resolves to the expected executable. Impostors
    // embedding the suffix in a larger command line (`python -c ...`)
    // fail here because their prefix is not a path to the binary.
    command
        .strip_suffix(&args_suffix)
        .is_some_and(|invocation| {
            !invocation.is_empty() && paths_name_same(Path::new(invocation), expected_exe)
        })
}

/// Compare a running process's executable against an expected path. Linux
/// reports binaries replaced by an in-place upgrade as `<path> (deleted)`;
/// strip that marker so an upgraded-out-from-under supervisor still matches
/// its recorded path.
#[cfg(unix)]
fn exe_paths_match(process_exe: &Path, expected_exe: &Path) -> bool {
    let process_exe = process_exe
        .to_str()
        .and_then(|s| s.strip_suffix(" (deleted)"))
        .map(Path::new)
        .unwrap_or(process_exe);
    paths_name_same(process_exe, expected_exe)
}

#[cfg(unix)]
fn matching_supervisor_pids(slot_id: &str) -> Vec<u32> {
    // Process-table scans have no status snapshot, so the CLI's own exe is
    // the only reference. A pre-upgrade supervisor stopped via the service
    // manager is already gone by the time sweeps run, so this stays correct.
    let Ok(current_exe) = std::env::current_exe() else {
        return Vec::new();
    };
    makakoo_core::agents::process::process_table_for_current_user()
        .into_iter()
        .filter_map(|process| {
            supervisor_command_matches(process.pid, &process.command, slot_id, &current_exe)
                .then_some(process.pid)
        })
        .collect()
}

#[cfg(unix)]
fn signal_matching_supervisors(slot_id: &str, force: bool) {
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    signal_processes(&matching_supervisor_pids(slot_id), signal, false);
}

#[cfg(not(unix))]
fn supervisor_process_matches(_pid: u32, _slot_id: &str) -> bool {
    false
}

#[cfg(unix)]
fn gateway_process_matches(home: &Path, slot_id: &str, pid: u32) -> bool {
    let Some(command) = makakoo_core::agents::process::command_for_current_user(pid) else {
        return false;
    };
    gateway_command_matches(home, slot_id, pid, &command)
}

#[cfg(unix)]
fn gateway_command_matches(home: &Path, slot_id: &str, pid: u32, command: &str) -> bool {
    if legacy_gateway_command_matches(home, pid, command, slot_id) {
        return true;
    }
    let tokens: Vec<_> = command.split_whitespace().collect();
    if tokens.len() != 3 || tokens[1] != "--env-file-if-exists=.env" || tokens[2] != "runner.mjs" {
        return false;
    }
    let Ok(slot_path) = checked_slot_path(home, slot_id) else {
        return false;
    };
    let Ok(slot) = makakoo_core::agents::AgentSlot::load_from_file(&slot_path) else {
        return false;
    };
    let Some(runtime) = slot.runtime else {
        return false;
    };
    if !process_cwd_matches(pid, &runtime.project_dir) {
        return false;
    }
    let Some(executable) = makakoo_core::agents::process::executable_for_current_user(pid) else {
        return false;
    };
    if !executable
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("node"))
    {
        return false;
    }
    // The exact argv + per-slot cwd + node-executable triple above is already
    // strong identity. runtime.json confirms it once the runner has written
    // its pid/slot marker — but a supervisor SIGKILLed in the spawn→write
    // window leaves a gateway with no marker, and cleanup must still find it.
    // So: when the file exists and carries both fields, require an exact
    // match (stale-pid protection); when it is absent or not yet complete,
    // accept on the triple alone (startup-window orphan coverage).
    match std::fs::read(runtime.project_dir.join("runtime.json")) {
        Ok(body) => match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(info) => match (info["pid"].as_u64(), info["slot"].as_str()) {
                (Some(marker_pid), Some(marker_slot)) => {
                    marker_pid == u64::from(pid) && marker_slot == slot_id
                }
                _ => true,
            },
            Err(_) => true,
        },
        Err(_) => true,
    }
}

#[cfg(unix)]
fn legacy_gateway_command_matches(home: &Path, pid: u32, command: &str, slot_id: &str) -> bool {
    let tokens: Vec<_> = command.split_whitespace().collect();
    if tokens.len() != 4
        || tokens[1] != "gateway.py"
        || tokens[2] != "--slot"
        || tokens[3] != slot_id
    {
        return false;
    }
    let expected_cwd = home.join("plugins-core/agent-harveychat/python");
    if !process_cwd_matches(pid, &expected_cwd) {
        return false;
    }
    makakoo_core::agents::process::executable_for_current_user(pid).is_some_and(|executable| {
        executable.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("python")
        })
    })
}

#[cfg(unix)]
fn process_cwd_matches(pid: u32, expected: &Path) -> bool {
    let Some(actual) = makakoo_core::agents::process::cwd_for_current_user(pid) else {
        return false;
    };
    paths_name_same(&actual, expected)
}

#[cfg(unix)]
fn paths_name_same(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    if cfg!(any(target_os = "macos", windows)) {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    } else {
        left == right
    }
}

#[cfg(unix)]
fn matching_gateway_pids(home: &Path, slot_id: &str) -> Vec<u32> {
    makakoo_core::agents::process::process_table_for_current_user()
        .into_iter()
        .filter_map(|process| {
            gateway_command_matches(home, slot_id, process.pid, &process.command)
                .then_some(process.pid)
        })
        .collect()
}

#[cfg(not(unix))]
fn gateway_process_matches(_home: &Path, _slot_id: &str, _pid: u32) -> bool {
    false
}

#[cfg(not(unix))]
fn matching_supervisor_pids(_slot_id: &str) -> Vec<u32> {
    Vec::new()
}

#[cfg(not(unix))]
fn signal_matching_supervisors(_slot_id: &str, _force: bool) {}

#[cfg(not(unix))]
fn matching_gateway_pids(_home: &Path, _slot_id: &str) -> Vec<u32> {
    Vec::new()
}

#[cfg(unix)]
fn signal_processes(pids: &[u32], signal: i32, include_process_group: bool) {
    for pid in pids {
        if *pid == 0 || *pid > i32::MAX as u32 {
            continue;
        }
        unsafe {
            if include_process_group {
                libc::kill(-(*pid as i32), signal);
            }
            libc::kill(*pid as i32, signal);
        }
    }
}

#[cfg(unix)]
fn terminate_matching_gateways(home: &Path, slot_id: &str) -> anyhow::Result<()> {
    let roots = matching_gateway_pids(home, slot_id);
    let targets = process_tree_pids(&roots);
    // Record each target's command line at selection time. A pid recycled by
    // an unrelated same-user process inside the TERM→KILL window must not
    // receive our SIGKILL, so every later step re-validates identity before
    // acting on or reporting a pid.
    let known: std::collections::HashMap<u32, String> =
        makakoo_core::agents::process::process_table_for_current_user()
            .into_iter()
            .filter(|process| targets.contains(&process.pid))
            .map(|process| (process.pid, process.command))
            .collect();
    let confirmed = |pids: Vec<u32>| -> Vec<u32> {
        pids.into_iter()
            .filter(|pid| {
                known.get(pid).is_some_and(|recorded| {
                    makakoo_core::agents::process::command_for_current_user(*pid).as_deref()
                        == Some(recorded.as_str())
                })
            })
            .collect()
    };
    signal_processes(&targets, libc::SIGTERM, true);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !confirmed(active_process_pids(&targets)).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let remaining = confirmed(active_process_pids(&targets));
    signal_processes(&remaining, libc::SIGKILL, true);
    let deadline = Instant::now() + Duration::from_secs(1);
    while !confirmed(active_process_pids(&targets)).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let remaining = confirmed(active_process_pids(&targets));
    if !remaining.is_empty() {
        anyhow::bail!(
            "slot '{slot_id}' gateway process(es) {remaining:?} survived shutdown; refusing cleanup"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn process_tree_pids(roots: &[u32]) -> Vec<u32> {
    let table = makakoo_core::agents::process::process_table_for_current_user();
    let mut selected = roots.to_vec();
    loop {
        let mut changed = false;
        for process in &table {
            if selected.contains(&process.parent_pid) && !selected.contains(&process.pid) {
                selected.push(process.pid);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Children first reduces the window in which a dying parent can orphan a
    // descendant before it receives the same signal.
    selected.reverse();
    selected
}

#[cfg(unix)]
fn active_process_pids(targets: &[u32]) -> Vec<u32> {
    makakoo_core::agents::process::process_table_for_current_user()
        .into_iter()
        .filter_map(|process| {
            (targets.contains(&process.pid) && !process.state.starts_with('Z'))
                .then_some(process.pid)
        })
        .collect()
}

#[cfg(not(unix))]
fn terminate_matching_gateways(_home: &Path, _slot_id: &str) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn cleanup_orphaned_legacy_gateway(home: &Path, slot_id: &str, dir: &Path) -> anyhow::Result<()> {
    // Do not trust status.json here. A hard-killed predecessor may leave no
    // snapshot (or a corrupt one), while its gateway process group survives.
    terminate_matching_gateways(home, slot_id)?;
    remove_status_file(dir)?;
    Ok(())
}

#[cfg(not(unix))]
fn cleanup_orphaned_legacy_gateway(_home: &Path, _slot_id: &str, dir: &Path) -> anyhow::Result<()> {
    remove_status_file(dir)
}

fn known_inactive(stderr: &str) -> bool {
    let message = stderr.to_ascii_lowercase();
    message.contains("not found")
        || message.contains("no such process")
        || message.contains("could not find")
        || message.contains("not loaded")
}

fn remove_status_file(dir: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(dir.join("status.json")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!("remove stale status: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::{Child, Command};
    use tempfile::TempDir;

    fn ctx_for(home: &Path) -> CliContext {
        CliContext::for_home(home.to_path_buf())
    }

    #[cfg(unix)]
    fn supervisorish(slot_id: &str) -> Child {
        Command::new("python3")
            .args([
                "-c",
                "import time; time.sleep(30)",
                "agent",
                "_supervisor",
                "--slot",
                slot_id,
            ])
            .spawn()
            .unwrap()
    }

    #[cfg(unix)]
    fn gatewayish(home: &Path, slot_id: &str) -> Child {
        let gateway_dir = home.join("plugins-core/agent-harveychat/python");
        fs::create_dir_all(&gateway_dir).unwrap();
        fs::write(
            gateway_dir.join("gateway.py"),
            "import time\ntime.sleep(30)\n",
        )
        .unwrap();
        Command::new("python3")
            .args(["gateway.py", "--slot", slot_id])
            .current_dir(gateway_dir)
            .spawn()
            .unwrap()
    }

    #[cfg(unix)]
    fn term_ignoring_gateway(home: &Path, slot_id: &str, ready: &Path) -> Child {
        let gateway_dir = home.join("plugins-core/agent-harveychat/python");
        fs::create_dir_all(&gateway_dir).unwrap();
        fs::write(
            gateway_dir.join("gateway.py"),
            "import os, pathlib, signal, time\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\npathlib.Path(os.environ['READY_PATH']).write_text('ready')\ntime.sleep(30)\n",
        )
        .unwrap();
        Command::new("python3")
            .args(["gateway.py", "--slot", slot_id])
            .env("READY_PATH", ready)
            .current_dir(gateway_dir)
            .spawn()
            .unwrap()
    }

    #[cfg(unix)]
    fn wait_for_process_match(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !predicate() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(predicate(), "test process never appeared in process table");
    }

    #[test]
    fn foreground_mode_is_explicit_only() {
        assert!(!foreground_requested_from(None));
        assert!(!foreground_requested_from(Some("")));
        assert!(!foreground_requested_from(Some("background")));
        assert!(foreground_requested_from(Some("foreground")));
    }

    #[test]
    fn supervisor_lock_excludes_duplicate_owner() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("run");
        let first = acquire_supervisor_lock(&dir).unwrap();
        assert!(supervisor_lock_held(&dir).unwrap());
        let duplicate = acquire_supervisor_lock(&dir).unwrap_err();
        assert!(lock_is_contended(&duplicate));
        drop(first);
        let deadline = Instant::now() + Duration::from_secs(1);
        while supervisor_lock_held(&dir).unwrap() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!supervisor_lock_held(&dir).unwrap());
    }

    #[test]
    fn lock_contention_detection_never_masks_unrelated_errors() {
        assert!(lock_is_contended(&std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "contended"
        )));
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::Other,
        ] {
            assert!(
                !lock_is_contended(&std::io::Error::new(kind, "unrelated")),
                "{kind:?} must not be treated as lock contention"
            );
        }
        #[cfg(windows)]
        {
            // ERROR_LOCK_VIOLATION (33) is how fs2 surfaces contention on
            // Windows; other raw codes (e.g. ERROR_ACCESS_DENIED = 5) are
            // unrelated I/O errors and must not be masked.
            assert!(lock_is_contended(&std::io::Error::from_raw_os_error(33)));
            assert!(!lock_is_contended(&std::io::Error::from_raw_os_error(5)));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn stop_slot_unsupported_platform_succeeds_for_offline_slot() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        let dir = checked_run_dir(tmp.path(), "secretary").unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("status.json"), b"{not-json").unwrap();
        assert_eq!(stop_slot(&ctx, "secretary").unwrap(), 0);
        assert!(!dir.join("status.json").exists());
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn stop_slot_unsupported_platform_refuses_while_lock_held() {
        // A foreground supervisor can run on any platform; while it owns the
        // runtime lock, stop must fail rather than claim a stop it cannot prove.
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        let dir = checked_run_dir(tmp.path(), "secretary").unwrap();
        let _guard = acquire_supervisor_lock(&dir).unwrap();
        assert_eq!(stop_slot(&ctx, "secretary").unwrap(), 1);
        assert!(supervisor_lock_held(&dir).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn destroy_guard_does_not_kill_command_line_substring_impostor() {
        let tmp = TempDir::new().unwrap();
        let mut process = supervisorish("startup-race");
        assert!(!supervisor_process_matches(process.id(), "startup-race"));

        let guard = acquire_destroy_guard(tmp.path(), "startup-race").unwrap();
        assert!(makakoo_core::agents::process::pid_is_alive(process.id()));
        drop(guard);
        process.kill().unwrap();
        let _ = process.wait();
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_match_requires_exact_argv_and_current_executable() {
        let pid = std::process::id();
        let exe = std::env::current_exe().unwrap();
        let exact = format!("{} agent _supervisor --slot exact", exe.display());
        assert!(supervisor_command_matches(pid, &exact, "exact", &exe));
        assert!(!supervisor_command_matches(
            pid,
            &format!("python -c payload {exact}"),
            "exact",
            &exe
        ));
        // Slot id is a full-suffix match: `--slot exact-2` must not match
        // a query for slot `exact`.
        assert!(!supervisor_command_matches(
            pid,
            &format!("{} agent _supervisor --slot exact-2", exe.display()),
            "exact",
            &exe
        ));
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_match_tolerates_whitespace_in_invocation_path() {
        // `ps` re-joins argv, so a binary below a directory with spaces
        // tokenizes into more than 5 fields; the suffix + resolved-prefix
        // path must still match.
        let exe = std::env::current_exe().unwrap();
        let tmp = TempDir::new().unwrap();
        let spaced_dir = tmp.path().join("dir with spaces");
        fs::create_dir_all(&spaced_dir).unwrap();
        let spaced_exe = spaced_dir.join(exe.file_name().unwrap());
        fs::hard_link(&exe, &spaced_exe).unwrap();
        let command = format!("{} agent _supervisor --slot spaced", spaced_exe.display());
        assert!(supervisor_argv_matches(&command, "spaced", &spaced_exe));
        // Same trick with a prefix that is not a plain path → impostor.
        assert!(!supervisor_argv_matches(
            &format!("sh -c '{command}'"),
            "spaced",
            &spaced_exe
        ));
    }

    #[cfg(unix)]
    #[test]
    fn exe_match_ignores_linux_deleted_marker() {
        let exe = std::env::current_exe().unwrap();
        let deleted_string = format!("{} (deleted)", exe.display());
        let deleted = Path::new(&deleted_string);
        assert!(exe_paths_match(deleted, &exe));
        assert!(!exe_paths_match(Path::new("/bin/false"), &exe));
    }

    #[cfg(unix)]
    #[test]
    fn orphaned_gateway_is_terminated_without_status_snapshot() {
        let tmp = TempDir::new().unwrap();
        let dir = checked_run_dir(tmp.path(), "orphan").unwrap();
        fs::create_dir_all(&dir).unwrap();
        let mut process = gatewayish(tmp.path(), "orphan");
        wait_for_process_match(|| gateway_process_matches(tmp.path(), "orphan", process.id()));

        cleanup_orphaned_legacy_gateway(tmp.path(), "orphan", &dir).unwrap();
        assert!(!gateway_process_matches(tmp.path(), "orphan", process.id()));
        let _ = process.wait();
    }

    #[cfg(unix)]
    #[test]
    fn orphaned_gateway_that_ignores_term_is_killed_after_grace() {
        let tmp = TempDir::new().unwrap();
        let dir = checked_run_dir(tmp.path(), "stubborn").unwrap();
        fs::create_dir_all(&dir).unwrap();
        let ready = tmp.path().join("ready");
        let mut process = term_ignoring_gateway(tmp.path(), "stubborn", &ready);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(ready.exists(), "TERM-ignoring child never became ready");
        wait_for_process_match(|| gateway_process_matches(tmp.path(), "stubborn", process.id()));

        cleanup_orphaned_legacy_gateway(tmp.path(), "stubborn", &dir).unwrap();
        let status = process.wait().unwrap();
        assert!(
            !status.success(),
            "TERM-ignoring child exited without escalation"
        );
        assert!(!gateway_process_matches(
            tmp.path(),
            "stubborn",
            process.id()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn orphan_cleanup_terminates_descendants_without_process_group_leader() {
        let tmp = TempDir::new().unwrap();
        let dir = checked_run_dir(tmp.path(), "tree").unwrap();
        fs::create_dir_all(&dir).unwrap();
        let gateway_dir = tmp.path().join("plugins-core/agent-harveychat/python");
        fs::create_dir_all(&gateway_dir).unwrap();
        let child_pid_file = tmp.path().join("child-pid");
        fs::write(
            gateway_dir.join("gateway.py"),
            "import os, pathlib, subprocess, sys, time\nchild = subprocess.Popen([sys.executable, '-c', 'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)'])\npathlib.Path(os.environ['CHILD_PID_FILE']).write_text(str(child.pid))\ntime.sleep(30)\n",
        )
        .unwrap();
        let mut gateway = Command::new("python3")
            .args(["gateway.py", "--slot", "tree"])
            .env("CHILD_PID_FILE", &child_pid_file)
            .current_dir(&gateway_dir)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !child_pid_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        let child_pid: u32 = fs::read_to_string(&child_pid_file)
            .unwrap()
            .parse()
            .unwrap();
        wait_for_process_match(|| gateway_process_matches(tmp.path(), "tree", gateway.id()));
        assert!(process_tree_pids(&[gateway.id()]).contains(&child_pid));

        cleanup_orphaned_legacy_gateway(tmp.path(), "tree", &dir).unwrap();
        assert!(active_process_pids(&[gateway.id(), child_pid]).is_empty());
        let _ = gateway.wait();
    }

    #[cfg(unix)]
    #[test]
    fn orphan_cleanup_removes_corrupt_status_snapshot() {
        let tmp = TempDir::new().unwrap();
        let dir = checked_run_dir(tmp.path(), "corrupt").unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("status.json"), b"{not-json").unwrap();

        cleanup_orphaned_legacy_gateway(tmp.path(), "corrupt", &dir).unwrap();
        assert!(!dir.join("status.json").exists());
    }

    #[test]
    fn service_artifact_cleanup_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("agent.service");
        fs::write(&path, "unit").unwrap();
        remove_service_artifact(&path).unwrap();
        assert!(!path.exists());
        remove_service_artifact(&path).unwrap();
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
        let dir = checked_run_dir(tmp.path(), "secretary").unwrap();
        fs::create_dir_all(&dir).unwrap();
        let snap = SupervisorStatusFile {
            slot_id: "secretary".into(),
            state: SupervisorState::Running,
            supervisor_pid: 42,
            gateway: makakoo_core::agents::status::GatewayStatus {
                alive: true,
                pid: Some(200),
                last_frame_at: None,
            },
            transports: Vec::new(),
            restart_count: 0,
            circuit_break_until: None,
            supervisor_exe: None,
            written_at: chrono::Utc::now(),
        };
        snap.write_atomic(&dir).unwrap();
        let rc = status_slot_with_process_check(&ctx, "secretary", |pid, slot| {
            pid == 42 && slot == "secretary"
        })
        .unwrap();
        assert_eq!(rc, 0);
    }

    #[test]
    fn status_slot_rejects_and_removes_stale_snapshot() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        let dir = checked_run_dir(tmp.path(), "secretary").unwrap();
        fs::create_dir_all(&dir).unwrap();
        let snap = SupervisorStatusFile {
            slot_id: "secretary".into(),
            state: SupervisorState::Running,
            supervisor_pid: u32::MAX,
            gateway: makakoo_core::agents::status::GatewayStatus {
                alive: false,
                pid: None,
                last_frame_at: None,
            },
            transports: Vec::new(),
            restart_count: 0,
            circuit_break_until: None,
            supervisor_exe: None,
            written_at: chrono::Utc::now(),
        };
        snap.write_atomic(&dir).unwrap();
        assert_eq!(status_slot(&ctx, "secretary").unwrap(), 1);
        assert!(!dir.join("status.json").exists());
    }

    #[test]
    fn failed_stop_keeps_live_status_and_reports_failure() {
        let tmp = TempDir::new().unwrap();
        let dir = checked_run_dir(tmp.path(), "secretary").unwrap();
        fs::create_dir_all(&dir).unwrap();
        let snap = SupervisorStatusFile {
            slot_id: "secretary".into(),
            state: SupervisorState::Running,
            supervisor_pid: std::process::id(),
            gateway: makakoo_core::agents::status::GatewayStatus {
                alive: false,
                pid: None,
                last_frame_at: None,
            },
            transports: Vec::new(),
            restart_count: 0,
            circuit_break_until: None,
            supervisor_exe: None,
            written_at: chrono::Utc::now(),
        };
        snap.write_atomic(&dir).unwrap();
        assert_eq!(
            finish_stop(tmp.path(), "secretary", 1, "denied").unwrap(),
            1
        );
        assert!(dir.join("status.json").exists());
    }

    #[test]
    fn unexplained_service_manager_failure_is_not_reported_as_stopped() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            finish_stop(tmp.path(), "secretary", 5, "input/output error").unwrap(),
            1
        );
    }

    #[test]
    fn wait_for_state_returns_when_target_hit() {
        let tmp = TempDir::new().unwrap();
        let dir = checked_run_dir(tmp.path(), "secretary").unwrap();
        fs::create_dir_all(&dir).unwrap();
        let snap = SupervisorStatusFile {
            slot_id: "secretary".into(),
            state: SupervisorState::Running,
            supervisor_pid: 42,
            gateway: makakoo_core::agents::status::GatewayStatus {
                alive: true,
                pid: Some(2),
                last_frame_at: None,
            },
            transports: Vec::new(),
            restart_count: 0,
            circuit_break_until: None,
            supervisor_exe: None,
            written_at: chrono::Utc::now(),
        };
        snap.write_atomic(&dir).unwrap();
        let st = wait_for_state_with_process_check(
            tmp.path(),
            "secretary",
            &[SupervisorState::Running],
            Duration::from_secs(2),
            |pid, slot| pid == 42 && slot == "secretary",
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
