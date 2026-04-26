//! `makakoo agent start|stop|status|health <plugin-name>` — thin
//! lifecycle driver over a plugin's declared `[entrypoint]` table.
//!
//! The Makakoo daemon is the primary lifecycle supervisor for agent
//! plugins (see `makakoo daemon install`). This subcommand is the
//! manual escape hatch for:
//!
//!   * SKILL.md examples that show operators how to start an agent,
//!   * plugin-update post-hooks that cycle an agent after reinstall
//!     (`sancho-task-plugin-update-check/post_update`),
//!   * local debugging when the daemon itself is the thing you're
//!     diagnosing.
//!
//! Semantics:
//!
//!   * `start  <name>` runs `entrypoint.start`.
//!   * `stop   <name>` runs `entrypoint.stop`.
//!   * `health <name>` runs `entrypoint.health` (exit 0 = alive).
//!   * `status <name>` runs `health` if declared, else falls back to
//!      a pgrep scan on the plugin name — useful for legacy agents that
//!      ship no `health` hook.
//!
//! Every entrypoint command is executed via `/bin/sh -c <cmd>` with
//! `cwd = plugin.root`. This matches how the daemon invokes them today
//! and how the plugins themselves document their entrypoints.

use std::path::Path;

use makakoo_core::plugin::PluginRegistry;

use crate::cli::AgentCmd;
use crate::context::CliContext;
use crate::output;

pub fn run(ctx: &CliContext, cmd: AgentCmd) -> anyhow::Result<i32> {
    match cmd {
        AgentCmd::Start { name } => hook(ctx, &name, Hook::Start),
        AgentCmd::Stop { name } => hook(ctx, &name, Hook::Stop),
        AgentCmd::Health { name } => hook(ctx, &name, Hook::Health),
        AgentCmd::Status { name } => status(ctx, &name),

        // Phase 2 multi-bot subagent registry.
        AgentCmd::List { json } => crate::commands::agent_slot::list(ctx, json),
        AgentCmd::Show { slot, json } => crate::commands::agent_slot::show(ctx, &slot, json),
        AgentCmd::Validate { slot } => crate::commands::agent_slot::validate(ctx, &slot),
        AgentCmd::Inventory { json } => crate::commands::agent_slot::inventory(ctx, json),
        AgentCmd::Create {
            slot,
            name,
            persona,
            from_toml,
            telegram_token,
            telegram_allowed,
            slack_bot_token,
            slack_app_token,
            slack_team,
            slack_allowed,
            skip_credential_check,
        } => crate::commands::agent_slot::create(
            ctx,
            crate::commands::agent_slot::CreateArgs {
                slot,
                name,
                persona,
                from_toml,
                telegram_token,
                telegram_allowed,
                slack_bot_token,
                slack_app_token,
                slack_team,
                slack_allowed,
                skip_credential_check,
            },
        ),
    }
}

#[derive(Clone, Copy)]
enum Hook {
    Start,
    Stop,
    Health,
}

impl Hook {
    fn label(self) -> &'static str {
        match self {
            Hook::Start => "start",
            Hook::Stop => "stop",
            Hook::Health => "health",
        }
    }
}

fn hook(ctx: &CliContext, name: &str, which: Hook) -> anyhow::Result<i32> {
    let registry = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let Some(plugin) = registry.get(name) else {
        output::print_error(format!("plugin not installed: {name}"));
        return Ok(1);
    };

    let ep = &plugin.manifest.entrypoint;
    let cmd = match which {
        Hook::Start => ep.start.as_deref(),
        Hook::Stop => ep.stop.as_deref(),
        Hook::Health => ep.health.as_deref(),
    };
    let Some(cmd) = cmd else {
        output::print_error(format!(
            "plugin {name} has no `[entrypoint].{}` declared in plugin.toml",
            which.label()
        ));
        return Ok(2);
    };

    exec_in(&plugin.root, cmd)
}

fn status(ctx: &CliContext, name: &str) -> anyhow::Result<i32> {
    let registry = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let Some(plugin) = registry.get(name) else {
        output::print_error(format!("plugin not installed: {name}"));
        return Ok(1);
    };

    // Prefer a plugin-declared health check — that's the authoritative
    // signal. Fall back to a pgrep scan on the plugin name for legacy
    // agents that ship no `health` hook.
    if let Some(cmd) = plugin.manifest.entrypoint.health.as_deref() {
        let rc = exec_in(&plugin.root, cmd)?;
        if rc == 0 {
            println!("{name}: up (health exit 0)");
        } else {
            println!("{name}: down (health exit {rc})");
        }
        return Ok(rc);
    }

    // No health hook declared — pgrep scan.
    let scan = std::process::Command::new("/usr/bin/pgrep")
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
            output::print_warn(format!(
                "status fallback (pgrep) failed: {e}; cannot determine state"
            ));
            Ok(2)
        }
    }
}

/// Run `cmd` via `/bin/sh -c`, chdir'd to `cwd`. Forwards the child's
/// stdout/stderr to the parent's terminal. Returns the child's exit
/// code (0 on success).
fn exec_in(cwd: &Path, cmd: &str) -> anyhow::Result<i32> {
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .status()?;
    Ok(status.code().unwrap_or(1))
}
