//! `makakoo skill <name> [args...]` — plugin-first, capability-enforced dispatch.
//!
//! Dispatch order:
//!   1. Look up `<name>` in the PluginRegistry (installed plugins).
//!      If found, start a per-plugin CapabilityServer on a Unix socket,
//!      spawn the plugin process with the socket path in env, and enforce
//!      grants at runtime.
//!   2. Fall back to legacy `SkillRunner` (walks `harvey-os/skills/`).
//!
//! Both paths use `build_skill_env` for unified PYTHONPATH handling.

use std::process::Command;
use std::sync::Arc;

use makakoo_core::capability::{
    build_plugin_handler, socket_path, AuditLog, CapabilityServer,
};
use makakoo_core::platform::makakoo_home;
use makakoo_core::plugin::PluginRegistry;

use crate::context::CliContext;
use crate::output;
use crate::skill_runner::{build_skill_env, SkillRunner};

pub async fn run(name: &str, args: &[String], ctx: &CliContext) -> anyhow::Result<i32> {
    let home = makakoo_home();
    let registry = PluginRegistry::load_default(&home).unwrap_or_default();

    // Try plugin dispatch first — match by exact name or by stripping
    // the common "skill-" prefix and category segments.
    if let Some(plugin) = find_plugin(&registry, name) {
        if let Some(run_cmd) = &plugin.manifest.entrypoint.run {
            let library_paths = registry.get_library_paths();
            let mut env = build_skill_env(&home, &library_paths);

            // Build capability handler + grant table for this plugin.
            let store = ctx.store()?;
            let llm = ctx.llm();
            let emb = ctx.embeddings();
            let (handler, grants) =
                build_plugin_handler(&plugin.manifest, &home, store, llm, emb)?;

            // Create per-invocation socket (PID suffix prevents parallel collisions).
            let sock = socket_path(&home, &format!(
                "{}-{}",
                plugin.manifest.plugin.name,
                std::process::id()
            ));
            if let Some(parent) = sock.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            let audit = Arc::new(AuditLog::open_default(&home)?);
            let server = CapabilityServer::new(
                sock.clone(),
                grants,
                audit,
                handler,
            );
            let handle = server.serve().await?;

            // Tell the plugin where to connect.
            env.insert(
                "MAKAKOO_PLUGIN_SOCKET".into(),
                sock.to_string_lossy().into_owned(),
            );

            // Split command into parts, expand $MAKAKOO_HOME.
            let parts: Vec<String> = run_cmd
                .split_whitespace()
                .map(|p| p.replace("$MAKAKOO_HOME", &home.to_string_lossy()))
                .collect();

            if parts.is_empty() {
                output::print_error(format!(
                    "plugin '{}' has empty [entrypoint].run",
                    plugin.manifest.plugin.name
                ));
                return Ok(1);
            }

            let mut cmd = Command::new(&parts[0]);
            cmd.args(&parts[1..]);
            cmd.args(args);
            cmd.current_dir(&home);
            for (k, v) in &env {
                cmd.env(k, v);
            }

            let status = cmd.status().map_err(|e| {
                anyhow::anyhow!(
                    "failed to spawn plugin '{}': {e}",
                    plugin.manifest.plugin.name
                )
            })?;

            // Plugin exited — shut down the socket server and clean up.
            handle.shutdown().await;
            // Socket file removed by shutdown, but ensure cleanup on any path.
            let _ = std::fs::remove_file(&sock);

            return Ok(status.code().unwrap_or(1));
        }
    }

    // Fallback to legacy SkillRunner.
    let library_paths = registry.get_library_paths();
    let runner = SkillRunner::with_library_paths(&library_paths)?;
    match runner.run(name, args) {
        Ok(status) => Ok(status.code().unwrap_or(1)),
        Err(e) => {
            output::print_error(format!("skill '{name}': {e}"));
            Ok(1)
        }
    }
}

/// Find a plugin matching the given skill name.
fn find_plugin<'a>(
    registry: &'a PluginRegistry,
    name: &str,
) -> Option<&'a makakoo_core::plugin::LoadedPlugin> {
    use makakoo_core::plugin::manifest::PluginKind;

    // Exact match.
    if let Some(p) = registry.get(name) {
        return Some(p);
    }

    // Try matching by segment-suffix (e.g. "canary" matches "skill-meta-canary").
    for plugin in registry.plugins() {
        if plugin.manifest.plugin.kind != PluginKind::Skill {
            continue;
        }
        let pname = &plugin.manifest.plugin.name;
        let segments: Vec<&str> = pname.split('-').collect();
        let name_segments: Vec<&str> = name.split('-').collect();

        if segments.len() > name_segments.len() {
            let suffix = &segments[segments.len() - name_segments.len()..];
            if suffix == name_segments {
                return Some(plugin);
            }
        }
    }
    None
}
