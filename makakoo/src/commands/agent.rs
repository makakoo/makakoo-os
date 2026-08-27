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
//!     a pgrep scan on the plugin name — useful for legacy agents that
//!     ship no `health` hook.
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
    use crate::commands::agent_lifecycle;
    match cmd {
        // Slot-aware: if a slot.toml exists for this name, route to the
        // per-slot supervisor lifecycle (LaunchAgent / systemd-user).
        // Otherwise fall back to the legacy plugin entrypoint hooks.
        AgentCmd::Start { name } => {
            if agent_lifecycle::is_slot(ctx.home(), &name) {
                agent_lifecycle::start_slot(ctx, &name)
            } else {
                hook(ctx, &name, Hook::Start)
            }
        }
        AgentCmd::Stop { name } => {
            if agent_lifecycle::is_slot(ctx.home(), &name) {
                agent_lifecycle::stop_slot(ctx, &name)
            } else {
                hook(ctx, &name, Hook::Stop)
            }
        }
        AgentCmd::Status { name } => {
            if agent_lifecycle::is_slot(ctx.home(), &name) {
                agent_lifecycle::status_slot(ctx, &name)
            } else {
                status(ctx, &name)
            }
        }
        AgentCmd::Prompt {
            slot,
            prompt,
            session,
        } => crate::commands::agent_prompt::run(ctx, &slot, &prompt, &session),
        AgentCmd::Restart { name } => {
            if agent_lifecycle::is_slot(ctx.home(), &name) {
                agent_lifecycle::restart_slot(ctx, &name)
            } else {
                let _ = hook(ctx, &name, Hook::Stop)?;
                hook(ctx, &name, Hook::Start)
            }
        }
        AgentCmd::Supervisor { slot } => agent_lifecycle::run_supervisor_command(ctx, &slot),
        AgentCmd::Health { name } => match health_route(ctx.home(), &name)? {
            HealthRoute::DeepseekHarness => crate::commands::agent_prompt::health(ctx, &name),
            HealthRoute::LegacySlot => agent_lifecycle::status_slot(ctx, &name),
            HealthRoute::Plugin => hook(ctx, &name, Hook::Health),
        },

        // Phase 2 multi-bot subagent registry.
        AgentCmd::List { json } => crate::commands::agent_slot::list(ctx, json),
        AgentCmd::Show { slot, json } => crate::commands::agent_slot::show(ctx, &slot, json),
        AgentCmd::Validate { slot } => crate::commands::agent_slot::validate(ctx, &slot),
        AgentCmd::ValidateSpec { path } => crate::commands::agent_slot::validate_spec(ctx, &path),
        AgentCmd::InitSpec { path, minimal } => {
            crate::commands::agent_slot::init_spec(ctx, &path, minimal)
        }
        AgentCmd::ProviderSet { provider, model } => {
            // If model is None, look up the provider's default_model
            // from the discovery function. If the provider isn't
            // discovered (no running gateway / env var), default
            // to `<provider>/<provider>` as a placeholder.
            let specifier = match model {
                Some(m) => format!("{}/{}", provider, m),
                None => {
                    use makakoo_core::agents::llm_provider::discover_providers;
                    let providers = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(discover_providers())
                    });
                    let p = providers.iter().find(|p| p.id == provider);
                    match p {
                        Some(p) => format!("{}/{}", p.id, p.default_model),
                        None => format!("{}/{}", provider, provider),
                    }
                }
            };
            match makakoo_core::agents::llm_provider_default::set_default(&specifier) {
                Ok(()) => {
                    output::print_info(format!("✓ Set project default: {}", specifier));
                    Ok(0)
                }
                Err(e) => {
                    output::print_error(format!("failed to set default: {}", e));
                    Ok(1)
                }
            }
        }
        AgentCmd::ProviderGet => {
            match makakoo_core::agents::llm_provider_default::get_default() {
                Some(d) => println!("{}", d),
                None => {
                    println!(
                        "No project default set. Use `makakoo agent provider-set <provider> <model>`."
                    );
                    return Ok(1);
                }
            }
            Ok(0)
        }
        AgentCmd::Inventory { json } => crate::commands::agent_slot::inventory(ctx, json),
        AgentCmd::Create {
            slot,
            name,
            persona,
            allowed_paths,
            forbidden_paths,
            tools,
            telegram_token,
            telegram_allowed,
            slack_bot_token,
            slack_app_token,
            slack_team,
            slack_allowed,
            skip_credential_check,
            out,
            specs,
        } => crate::commands::agent_slot::create(
            ctx,
            crate::commands::agent_slot::CreateArgs {
                slot: slot.unwrap_or_default(),
                name,
                persona,
                allowed_paths,
                forbidden_paths,
                tools,
                telegram_token,
                telegram_allowed,
                slack_bot_token,
                slack_app_token,
                slack_team,
                slack_allowed,
                skip_credential_check,
                out,
                specs,
            },
        ),
        AgentCmd::MigrateHarveychat => crate::commands::agent_slot::migrate_harveychat(ctx),
        AgentCmd::Destroy {
            slot,
            yes,
            revoke_secrets,
            keep_secrets,
            really_destroy_harveychat,
        } => crate::commands::agent_destroy::run(
            ctx,
            crate::commands::agent_destroy::DestroyArgs {
                slot,
                yes,
                revoke_secrets,
                keep_secrets,
                really_destroy_harveychat,
            },
        ),
        AgentCmd::Audit { last, kind, json } => {
            crate::commands::agent_audit::run(ctx, last, kind, json)?;
            Ok(0)
        }
        AgentCmd::TestFaults { scenario, json } => {
            crate::commands::agent_test_faults::run(scenario, json)?;
            Ok(0)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum HealthRoute {
    DeepseekHarness,
    LegacySlot,
    Plugin,
}

fn health_route(home: &Path, name: &str) -> anyhow::Result<HealthRoute> {
    let path = makakoo_core::agents::checked_slot_path(home, name)?;
    if !path.exists() {
        return Ok(HealthRoute::Plugin);
    }
    let slot = makakoo_core::agents::AgentSlot::load_from_file(&path)
        .map_err(|error| anyhow::anyhow!("agent slot '{}' load failed: {}", name, error))?;
    Ok(match slot.runtime.as_ref().map(|runtime| runtime.engine) {
        Some(makakoo_core::agents::AgentRuntimeEngine::DeepseekHarness) => {
            HealthRoute::DeepseekHarness
        }
        _ => HealthRoute::LegacySlot,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_slot(home: &Path, name: &str, body: &str) {
        let dir = home.join("config/agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.toml")), body).unwrap();
    }

    #[test]
    fn health_dispatch_distinguishes_dsh_legacy_and_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        write_slot(tmp.path(), "legacy", "slot_id = \"legacy\"\n");
        write_slot(
            tmp.path(),
            "dsh",
            "slot_id = \"dsh\"\n[runtime]\nengine = \"deepseek-harness\"\nproject_dir = \"/tmp/dsh\"\n",
        );

        assert_eq!(
            health_route(tmp.path(), "dsh").unwrap(),
            HealthRoute::DeepseekHarness
        );
        assert_eq!(
            health_route(tmp.path(), "legacy").unwrap(),
            HealthRoute::LegacySlot
        );
        assert_eq!(
            health_route(tmp.path(), "plugin-only").unwrap(),
            HealthRoute::Plugin
        );
    }
}
