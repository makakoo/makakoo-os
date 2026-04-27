//! Plugin lifecycle dispatch for `makakoo plugin start|stop|status|restart`.
//!
//! Service-aware. Agent-kind plugins fall through to the same kind-agnostic
//! `[entrypoint]` runner that `makakoo agent` uses today (foreground exec).
//! Service-kind plugins get extra behavior:
//!
//!   * `start_cmd` / `stop_cmd` from `[service]` override `[entrypoint]`.
//!   * `start` is **backgrounded** — the started process is detached and
//!     stdout/stderr redirected to `~/Library/Logs/<plugin>.{out,err}.log`
//!     (macOS convention). Linux falls back to `~/.local/state/makakoo/log/`.
//!   * `health_endpoint` strings starting with `http://` / `https://` are
//!     probed via GET (200/204 = healthy); everything else is shelled.
//!   * `restart` = stop + small grace + start.
//!
//! The runner does not yet implement `restart_policy` enforcement under
//! supervision — that's a daemon-side concern landing alongside the
//! Garage launchd integration in Phase A₁. For now, `restart_policy` is
//! parsed and stored; nothing acts on it.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use makakoo_core::plugin::manifest::{PluginKind, ServiceTable};
use makakoo_core::plugin::{LoadedPlugin, PluginRegistry};

use crate::context::CliContext;
use crate::output;

pub fn start(ctx: &CliContext, name: &str) -> anyhow::Result<i32> {
    with_plugin(ctx, name, |p| match p.manifest.plugin.kind {
        PluginKind::Service => service_start(p),
        PluginKind::Agent => foreground_run(p, EntrypointHook::Start),
        other => {
            output::print_error(format!(
                "plugin {name} has kind = {} — start/stop/status/restart \
                 only supported for service or agent kinds",
                other.as_str()
            ));
            Ok(2)
        }
    })
}

pub fn stop(ctx: &CliContext, name: &str) -> anyhow::Result<i32> {
    with_plugin(ctx, name, |p| match p.manifest.plugin.kind {
        PluginKind::Service => service_stop(p),
        PluginKind::Agent => foreground_run(p, EntrypointHook::Stop),
        other => {
            output::print_error(format!(
                "plugin {name} has kind = {} — start/stop/status/restart \
                 only supported for service or agent kinds",
                other.as_str()
            ));
            Ok(2)
        }
    })
}

pub fn status(ctx: &CliContext, name: &str) -> anyhow::Result<i32> {
    with_plugin(ctx, name, |p| match p.manifest.plugin.kind {
        PluginKind::Service => service_status(p),
        PluginKind::Agent => agent_status(p),
        other => {
            output::print_error(format!(
                "plugin {name} has kind = {} — start/stop/status/restart \
                 only supported for service or agent kinds",
                other.as_str()
            ));
            Ok(2)
        }
    })
}

pub fn restart(ctx: &CliContext, name: &str) -> anyhow::Result<i32> {
    with_plugin(ctx, name, |p| {
        let stop_rc = match p.manifest.plugin.kind {
            PluginKind::Service => service_stop(p),
            PluginKind::Agent => foreground_run(p, EntrypointHook::Stop),
            other => {
                output::print_error(format!(
                    "plugin {name} has kind = {} — restart not supported",
                    other.as_str()
                ));
                return Ok(2);
            }
        }?;
        // Stop returning non-zero (e.g. nothing was running) is not fatal
        // for a restart — we proceed to start regardless.
        if stop_rc != 0 {
            output::print_warn(format!(
                "stop exited {stop_rc} during restart; continuing to start"
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
        match p.manifest.plugin.kind {
            PluginKind::Service => service_start(p),
            PluginKind::Agent => foreground_run(p, EntrypointHook::Start),
            _ => unreachable!(),
        }
    })
}

#[derive(Clone, Copy)]
enum EntrypointHook {
    Start,
    Stop,
}

fn with_plugin<F>(ctx: &CliContext, name: &str, f: F) -> anyhow::Result<i32>
where
    F: FnOnce(&LoadedPlugin) -> anyhow::Result<i32>,
{
    let registry = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let Some(plugin) = registry.get(name) else {
        output::print_error(format!("plugin not installed: {name}"));
        return Ok(1);
    };
    f(plugin)
}

fn foreground_run(p: &LoadedPlugin, which: EntrypointHook) -> anyhow::Result<i32> {
    let cmd = match which {
        EntrypointHook::Start => p.manifest.entrypoint.start.as_deref(),
        EntrypointHook::Stop => p.manifest.entrypoint.stop.as_deref(),
    };
    let Some(cmd) = cmd else {
        output::print_error(format!(
            "plugin {} has no `[entrypoint].{}` declared",
            p.manifest.plugin.name,
            match which {
                EntrypointHook::Start => "start",
                EntrypointHook::Stop => "stop",
            }
        ));
        return Ok(2);
    };
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(&p.root)
        .status()?;
    Ok(status.code().unwrap_or(1))
}

fn agent_status(p: &LoadedPlugin) -> anyhow::Result<i32> {
    let name = &p.manifest.plugin.name;
    if let Some(cmd) = p.manifest.entrypoint.health.as_deref() {
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&p.root)
            .status()?;
        let rc = status.code().unwrap_or(1);
        if rc == 0 {
            println!("{name}: up (health exit 0)");
        } else {
            println!("{name}: down (health exit {rc})");
        }
        return Ok(rc);
    }
    let scan = Command::new("/usr/bin/pgrep")
        .arg("-f")
        .arg(name)
        .output();
    match scan {
        Ok(out) if out.status.success() => {
            println!("{name}: up (pgrep match)");
            Ok(0)
        }
        Ok(_) => {
            println!("{name}: down (no pgrep match, no declared health hook)");
            Ok(1)
        }
        Err(e) => {
            output::print_warn(format!("status fallback (pgrep) failed: {e}"));
            Ok(2)
        }
    }
}

// ── service-kind specific paths ─────────────────────────────────────────

fn service_start(p: &LoadedPlugin) -> anyhow::Result<i32> {
    let name = &p.manifest.plugin.name;
    let cmd = service_start_cmd(p);
    let Some(cmd) = cmd else {
        output::print_error(format!(
            "service plugin {name} has neither [entrypoint].start nor [service].start_cmd"
        ));
        return Ok(2);
    };

    let log_dir = log_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        output::print_error(format!(
            "failed to create log dir {}: {e}",
            log_dir.display()
        ));
        return Ok(1);
    }
    let out_path = log_dir.join(format!("{name}.out.log"));
    let err_path = log_dir.join(format!("{name}.err.log"));

    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&out_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&err_path)?;

    let child = Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .current_dir(&p.root)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;

    println!(
        "{name}: started (pid {}, log {})",
        child.id(),
        out_path.display()
    );
    // Detach — drop the child handle without waiting.
    drop(child);
    Ok(0)
}

fn service_stop(p: &LoadedPlugin) -> anyhow::Result<i32> {
    let cmd = service_stop_cmd(p);
    let Some(cmd) = cmd else {
        output::print_error(format!(
            "service plugin {} has neither [entrypoint].stop nor [service].stop_cmd",
            p.manifest.plugin.name
        ));
        return Ok(2);
    };
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .current_dir(&p.root)
        .status()?;
    Ok(status.code().unwrap_or(1))
}

fn service_status(p: &LoadedPlugin) -> anyhow::Result<i32> {
    let name = &p.manifest.plugin.name;
    let probe = service_health(p);
    let Some(probe) = probe else {
        output::print_error(format!(
            "service plugin {name} has no [entrypoint].health or [service].health_endpoint"
        ));
        return Ok(2);
    };
    let rc = match probe {
        HealthProbe::Http(url) => probe_http(&url),
        HealthProbe::Shell(cmd) => Command::new("/bin/sh")
            .arg("-c")
            .arg(&cmd)
            .current_dir(&p.root)
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or(1),
    };
    if rc == 0 {
        println!("{name}: up (health exit 0)");
    } else {
        println!("{name}: down (health exit {rc})");
    }
    Ok(rc)
}

enum HealthProbe {
    Http(String),
    Shell(String),
}

fn service_start_cmd(p: &LoadedPlugin) -> Option<String> {
    let svc = p.manifest.service.as_ref();
    svc.and_then(|s| s.start_cmd.clone())
        .or_else(|| p.manifest.entrypoint.start.clone())
}

fn service_stop_cmd(p: &LoadedPlugin) -> Option<String> {
    let svc = p.manifest.service.as_ref();
    svc.and_then(|s| s.stop_cmd.clone())
        .or_else(|| p.manifest.entrypoint.stop.clone())
}

fn service_health(p: &LoadedPlugin) -> Option<HealthProbe> {
    let svc = p.manifest.service.as_ref();
    if let Some(ep) = svc.and_then(|s: &ServiceTable| s.health_endpoint.clone()) {
        if ep.starts_with("http://") || ep.starts_with("https://") {
            return Some(HealthProbe::Http(ep));
        }
        return Some(HealthProbe::Shell(ep));
    }
    p.manifest
        .entrypoint
        .health
        .clone()
        .map(HealthProbe::Shell)
}

fn probe_http(url: &str) -> i32 {
    // Minimal HEAD/GET probe via curl — we already shell out elsewhere
    // and avoid pulling reqwest into the CLI binary just for this.
    // -fsS: fail on HTTP error, silent progress, show errors.
    // -m 5: 5s timeout. Output discarded.
    let status = Command::new("/usr/bin/curl")
        .args(["-fsS", "-m", "5", "-o", "/dev/null", url])
        .status();
    match status {
        Ok(s) if s.success() => 0,
        Ok(s) => s.code().unwrap_or(1),
        Err(_) => 1,
    }
}

fn log_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME").unwrap_or_default();
        Path::new(&home).join("Library").join("Logs").join("makakoo")
    } else {
        let home = std::env::var_os("HOME").unwrap_or_default();
        Path::new(&home)
            .join(".local")
            .join("state")
            .join("makakoo")
            .join("log")
    }
}
