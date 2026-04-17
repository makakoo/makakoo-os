//! `makakoo skill <name> [args...]` — plugin-first, legacy-fallback dispatch.
//!
//! Dispatch order:
//!   1. Look up `<name>` in the PluginRegistry (installed plugins).
//!      If found, read `[entrypoint].run`, expand `$MAKAKOO_HOME`, spawn.
//!   2. Fall back to legacy `SkillRunner` (walks `harvey-os/skills/`).
//!
//! Both paths use `build_skill_env` for unified PYTHONPATH handling.

use std::process::Command;

use makakoo_core::platform::makakoo_home;
use makakoo_core::plugin::PluginRegistry;

use crate::output;
use crate::skill_runner::{build_skill_env, SkillRunner};

pub fn run(name: &str, args: &[String]) -> anyhow::Result<i32> {
    let home = makakoo_home();
    let registry = PluginRegistry::load_default(&home).unwrap_or_default();

    // Try plugin dispatch first — match by exact name or by stripping
    // the common "skill-" prefix and category segments.
    if let Some(plugin) = find_plugin(&registry, name) {
        if let Some(run_cmd) = &plugin.manifest.entrypoint.run {
            let library_paths = registry.get_library_paths();
            let env = build_skill_env(&home, &library_paths);

            // Split command into parts FIRST so that $MAKAKOO_HOME can
            // contain spaces without breaking the argument list.
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
            // Set working dir to $MAKAKOO_HOME so relative paths in
            // entrypoint.run resolve correctly.
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

/// Find a plugin matching the given skill name. Tries:
///   1. Exact match (e.g. "skill-meta-canary")
///   2. Suffix match stripping "skill-" prefix (e.g. "canary" matches "skill-meta-canary")
///   3. Suffix match on last segment (e.g. "dev-orchestrator" matches "skill-dev-dev-orchestrator")
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
