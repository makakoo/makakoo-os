//! `makakoo plugin list|info|install|uninstall` — user-facing lifecycle.

use std::path::{Path, PathBuf};

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};
use crossterm::style::Stylize;
use serde_json::json;

use makakoo_core::plugin::{
    install_from_path, uninstall as core_uninstall, InstallRequest, Manifest, PluginRegistry,
    PluginSource, PluginsLock,
};

use crate::cli::PluginCmd;
use crate::context::CliContext;
use crate::output;

pub async fn run(ctx: &CliContext, cmd: PluginCmd) -> anyhow::Result<i32> {
    match cmd {
        PluginCmd::List { json } => list(ctx, json),
        PluginCmd::Info { name } => info(ctx, &name),
        PluginCmd::Install {
            source,
            core,
            blake3,
        } => install(ctx, &source, core, blake3),
        PluginCmd::Uninstall { name, purge } => uninstall(ctx, &name, purge),
    }
}

fn list(ctx: &CliContext, as_json: bool) -> anyhow::Result<i32> {
    let registry = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let lock = PluginsLock::load(ctx.home())?;

    if as_json {
        let rows: Vec<_> = registry
            .plugins()
            .iter()
            .map(|p| {
                let lock_entry = lock.get(&p.manifest.plugin.name);
                json!({
                    "name": p.manifest.plugin.name,
                    "version": p.manifest.plugin.version.to_string(),
                    "kind": format!("{:?}", p.manifest.plugin.kind),
                    "language": format!("{:?}", p.manifest.plugin.language),
                    "root": p.root.display().to_string(),
                    "blake3": lock_entry.and_then(|e| e.blake3.clone()),
                    "source": lock_entry.map(|e| e.source.clone()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }

    if registry.is_empty() {
        println!("{}", "(no plugins installed)".dark_grey());
        if let Some(ref d) = lock.meta.distro {
            output::print_info(format!("active distro: {d}"));
        }
        return Ok(0);
    }

    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(vec![
        Cell::new("name").fg(TableColor::Cyan),
        Cell::new("version").fg(TableColor::Cyan),
        Cell::new("kind").fg(TableColor::Cyan),
        Cell::new("language").fg(TableColor::Cyan),
        Cell::new("source").fg(TableColor::Cyan),
    ]);
    for p in registry.plugins() {
        let entry = lock.get(&p.manifest.plugin.name);
        t.add_row(vec![
            Cell::new(&p.manifest.plugin.name).fg(TableColor::White),
            Cell::new(p.manifest.plugin.version.to_string()),
            Cell::new(format!("{:?}", p.manifest.plugin.kind)),
            Cell::new(format!("{:?}", p.manifest.plugin.language)),
            Cell::new(
                entry
                    .map(|e| e.source.clone())
                    .unwrap_or_else(|| "(no lock entry)".into()),
            ),
        ]);
    }
    println!("{t}");
    if let Some(ref d) = lock.meta.distro {
        output::print_info(format!("active distro: {d}"));
    }
    Ok(0)
}

fn info(ctx: &CliContext, name: &str) -> anyhow::Result<i32> {
    let registry = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let Some(plugin) = registry.get(name) else {
        output::print_error(format!("plugin not installed: {name}"));
        return Ok(1);
    };

    let m = &plugin.manifest;
    output::print_info(format!("{} v{}", m.plugin.name, m.plugin.version));
    if let Some(ref s) = m.plugin.summary {
        println!("  summary: {s}");
    }
    println!("  kind:     {:?}", m.plugin.kind);
    println!("  language: {:?}", m.plugin.language);
    println!("  root:     {}", plugin.root.display());
    if let Some(ref lic) = m.plugin.license {
        println!("  license:  {lic}");
    }

    if !m.capabilities.grants.is_empty() {
        println!("\n  capabilities:");
        for g in &m.capabilities.grants {
            println!("    - {g}");
        }
    }
    if !m.sancho.tasks.is_empty() {
        println!("\n  sancho tasks:");
        for task in &m.sancho.tasks {
            println!(
                "    - {} (interval: {})",
                task.name,
                task.interval
            );
        }
    }
    if !m.mcp.tools.is_empty() {
        println!("\n  mcp tools:");
        for t in &m.mcp.tools {
            println!("    - {}", t.name);
        }
    }

    let lock = PluginsLock::load(ctx.home())?;
    if let Some(entry) = lock.get(name) {
        println!("\n  lock entry:");
        println!("    installed_at: {}", entry.installed_at.to_rfc3339());
        println!(
            "    blake3:       {}",
            entry.blake3.as_deref().unwrap_or("(missing)")
        );
        println!("    source:       {}", entry.source);
    } else {
        output::print_warn("no plugins.lock entry — registry + lock are out of sync");
    }

    if !plugin.warnings.is_empty() {
        println!();
        for w in &plugin.warnings {
            output::print_warn(w);
        }
    }
    Ok(0)
}

fn install(
    ctx: &CliContext,
    source: &str,
    use_core: bool,
    blake3: Option<String>,
) -> anyhow::Result<i32> {
    let source_path = if use_core {
        resolve_plugins_core(source)?
    } else {
        PathBuf::from(source)
    };
    if !source_path.exists() {
        output::print_error(format!("source does not exist: {}", source_path.display()));
        return Ok(1);
    }

    let req = InstallRequest {
        source: PluginSource::Path(source_path.clone()),
        expected_blake3: blake3,
    };

    match install_from_path(&req, ctx.home()) {
        Ok(outcome) => {
            output::print_info(format!(
                "installed {} → {} (blake3: {})",
                outcome.name,
                outcome.final_dir.display(),
                outcome.computed_blake3
            ));
            Ok(0)
        }
        Err(e) => {
            output::print_error(e.to_string());
            Ok(1)
        }
    }
}

fn uninstall(ctx: &CliContext, name: &str, purge: bool) -> anyhow::Result<i32> {
    match core_uninstall(name, ctx.home(), purge) {
        Ok(outcome) => {
            output::print_info(format!(
                "uninstalled {} (removed {}){}",
                outcome.name,
                outcome.removed_from.display(),
                if outcome.state_wiped {
                    ", state wiped"
                } else {
                    ""
                }
            ));
            Ok(0)
        }
        Err(e) => {
            output::print_error(e.to_string());
            Ok(1)
        }
    }
}

/// Return the `plugins-core/` root directory.
///
/// Precedence: `$MAKAKOO_PLUGINS_CORE` env var → walk upward from CWD
/// looking for a `plugins-core/` dir → error.
pub(crate) fn plugins_core_root() -> anyhow::Result<PathBuf> {
    if let Ok(root) = std::env::var("MAKAKOO_PLUGINS_CORE") {
        return Ok(PathBuf::from(root));
    }
    let cwd = std::env::current_dir()?;
    if let Some(p) = walk_up_for(&cwd, "plugins-core") {
        return Ok(p);
    }
    anyhow::bail!(
        "can't find plugins-core/ — set $MAKAKOO_PLUGINS_CORE or run from a checkout that contains plugins-core/"
    )
}

/// Resolve a `--core <name>` argument to the plugin directory.
pub(crate) fn resolve_plugins_core(name: &str) -> anyhow::Result<PathBuf> {
    Ok(plugins_core_root()?.join(name))
}

pub(crate) fn walk_up_for(start: &Path, needle: &str) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        let candidate = cur.join(needle);
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !cur.pop() {
            return None;
        }
    }
}

// Keep a light compile-time touch on unused imports when features change.
#[allow(dead_code)]
fn _assert_unused_manifest_import(_m: Manifest) {}
