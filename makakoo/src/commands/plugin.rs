//! `makakoo plugin list|info|install|uninstall` — user-facing lifecycle.

use std::path::{Path, PathBuf};

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};
use crossterm::style::Stylize;
use serde_json::json;

use makakoo_core::capability::resolve_grants;
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
        PluginCmd::Enable { name } => set_enabled(ctx, &name, true),
        PluginCmd::Disable { name } => set_enabled(ctx, &name, false),
        PluginCmd::Update { name } => update(ctx, &name),
        PluginCmd::Sync { dry_run } => sync(ctx, dry_run),
    }
}

fn list(ctx: &CliContext, as_json: bool) -> anyhow::Result<i32> {
    let registry = match PluginRegistry::load_default(ctx.home()) {
        Ok(r) => r,
        Err(e) => {
            output::print_error(format!("plugin registry failed to load: {e:#}"));
            return Ok(1);
        }
    };
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
                    "enabled": p.enabled,
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
        Cell::new("enabled").fg(TableColor::Cyan),
        Cell::new("source").fg(TableColor::Cyan),
    ]);
    for p in registry.plugins() {
        let entry = lock.get(&p.manifest.plugin.name);
        let enabled_cell = if p.enabled {
            Cell::new("yes").fg(TableColor::Green)
        } else {
            Cell::new("no").fg(TableColor::Yellow)
        };
        t.add_row(vec![
            Cell::new(&p.manifest.plugin.name).fg(TableColor::White),
            Cell::new(p.manifest.plugin.version.to_string()),
            Cell::new(format!("{:?}", p.manifest.plugin.kind)),
            Cell::new(format!("{:?}", p.manifest.plugin.language)),
            enabled_cell,
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
    println!("  enabled:  {}", if plugin.enabled { "yes" } else { "no (soft-disabled)" });
    println!("  root:     {}", plugin.root.display());
    if let Some(ref lic) = m.plugin.license {
        println!("  license:  {lic}");
    }

    // Resolve the full grant table (explicit grants + auto-defaults).
    // Surface both the raw manifest declarations and the effective
    // resolved set so plugin authors can see what Makakoo actually
    // grants their plugin at load time.
    if !m.capabilities.grants.is_empty() {
        println!("\n  declared grants:");
        for g in &m.capabilities.grants {
            println!("    - {g}");
        }
    }
    match resolve_grants(m, ctx.home()) {
        Ok(table) => {
            let rows = table.rows();
            if !rows.is_empty() {
                println!("\n  effective grants (incl. auto-defaults):");
                for (verb, scopes) in rows {
                    if scopes.iter().all(|s| s.is_empty()) {
                        println!("    - {verb}");
                    } else {
                        let rendered: Vec<&str> =
                            scopes.iter().map(|s| s.as_str()).collect();
                        println!("    - {verb}:{}", rendered.join(","));
                    }
                }
            }
        }
        Err(e) => output::print_warn(format!("grant resolution failed: {e}")),
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

fn update(ctx: &CliContext, name: &str) -> anyhow::Result<i32> {
    // 1) Read the existing lock entry — it carries the recorded source
    //    path and the current enabled flag. Both must survive the
    //    reinstall round-trip.
    let lock = PluginsLock::load(ctx.home())?;
    let Some(entry) = lock.get(name).cloned() else {
        output::print_error(format!("plugin not installed: {name}"));
        return Ok(1);
    };

    // 2) Only `path:` sources are supported in v0.1. Git URL + tarball
    //    sources are a Phase F concern — they need a resolver layer the
    //    kernel doesn't ship yet.
    let Some(source_path) = entry.source.strip_prefix("path:") else {
        output::print_error(format!(
            "plugin {name} has a non-path source ({}) — `plugin update` only supports path: in v0.1; git URL and tarball sources land in Phase F",
            entry.source
        ));
        return Ok(1);
    };
    let source_path = PathBuf::from(source_path);
    if !source_path.exists() {
        output::print_error(format!(
            "recorded source path {} no longer exists — reinstall manually via `makakoo plugin install <path>`",
            source_path.display()
        ));
        return Ok(1);
    }

    let prior_enabled = entry.enabled;

    // 3) Uninstall without purge — keep the state dir. A user who wants
    //    a full reset runs `plugin uninstall --purge` + `plugin install`.
    if let Err(e) = core_uninstall(name, ctx.home(), false) {
        output::print_error(format!("uninstall step failed: {e}"));
        return Ok(1);
    }

    // 4) Reinstall from the recorded source.
    let req = InstallRequest {
        source: PluginSource::Path(source_path.clone()),
        expected_blake3: None,
    };
    let outcome = match install_from_path(&req, ctx.home()) {
        Ok(o) => o,
        Err(e) => {
            output::print_error(format!(
                "reinstall failed — plugin is currently uninstalled: {e}"
            ));
            return Ok(1);
        }
    };

    // 5) Reapply the saved enabled flag if it was disabled. Fresh
    //    installs land as enabled=true by design (see install.rs), so
    //    the state roundtrips only when prior_enabled was false.
    if !prior_enabled {
        let mut lock = PluginsLock::load(ctx.home())?;
        if let Some(mut e) = lock.get(name).cloned() {
            e.enabled = false;
            lock.upsert(e);
            lock.save(ctx.home())?;
        }
    }

    output::print_info(format!(
        "updated {} → blake3 {} (enabled: {})",
        outcome.name,
        outcome.computed_blake3,
        if prior_enabled { "yes" } else { "no (preserved from prior state)" }
    ));
    Ok(0)
}

/// Batch reinstall every plugin in `plugins-core/` into $MAKAKOO_HOME/plugins/.
///
/// Walks the plugins-core/ source tree, reads each plugin's manifest to
/// pull its prior enabled flag off the current lock (default: enabled),
/// calls `install_from_path` per-plugin, and restores the enabled flag
/// if it was disabled. Per-plugin failures (missing manifest, malformed
/// toml, native-task collision) log + skip rather than aborting the
/// batch — one bad plugin can't block the rest.
fn sync(ctx: &CliContext, dry_run: bool) -> anyhow::Result<i32> {
    let plugins_core = plugins_core_root()?;
    if !plugins_core.is_dir() {
        output::print_error(format!(
            "plugins-core not found at {}",
            plugins_core.display()
        ));
        return Ok(1);
    }

    let prior_lock = PluginsLock::load(ctx.home()).unwrap_or_default();

    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&plugins_core)? {
        let entry = entry?;
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if !p.join("plugin.toml").is_file() {
            continue;
        }
        candidates.push(p);
    }
    candidates.sort();

    let mut installed = 0usize;
    let mut skipped = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut reenabled = 0usize;

    for src in &candidates {
        let manifest_path = src.join("plugin.toml");
        let (manifest, _warn) = match Manifest::load(&manifest_path) {
            Ok(m) => m,
            Err(e) => {
                failed.push((src.display().to_string(), format!("manifest: {e}")));
                continue;
            }
        };
        let name = manifest.plugin.name.clone();

        if dry_run {
            println!("  would reinstall {name}");
            installed += 1;
            continue;
        }

        let prior_enabled = prior_lock
            .get(&name)
            .map(|e| e.enabled)
            .unwrap_or(true);

        let req = InstallRequest {
            source: PluginSource::Path(src.clone()),
            expected_blake3: None,
        };
        match install_from_path(&req, ctx.home()) {
            Ok(outcome) => {
                installed += 1;
                if !prior_enabled {
                    // Restore disabled flag so sync is state-preserving.
                    if let Ok(mut lock) = PluginsLock::load(ctx.home()) {
                        if let Some(mut e) = lock.get(&outcome.name).cloned() {
                            e.enabled = false;
                            lock.upsert(e);
                            let _ = lock.save(ctx.home());
                            reenabled += 1;
                        }
                    }
                }
            }
            Err(e) => {
                // One plugin fail must not abort the batch.
                let reason = format!("{e}");
                if reason.contains("already installed") || reason.contains("identical") {
                    skipped += 1;
                } else {
                    failed.push((name, reason));
                }
            }
        }
    }

    if dry_run {
        output::print_info(format!(
            "{} plugin(s) would be reinstalled (dry run, no changes made)",
            installed
        ));
    } else {
        output::print_info(format!(
            "sync done: {} installed, {} skipped (already up-to-date), {} failed, {} disabled-flag preserved",
            installed,
            skipped,
            failed.len(),
            reenabled
        ));
    }
    if !failed.is_empty() {
        println!("\n  failures:");
        for (name, reason) in &failed {
            println!("    - {name}: {reason}");
        }
    }
    Ok(if failed.is_empty() { 0 } else { 1 })
}

fn set_enabled(ctx: &CliContext, name: &str, target: bool) -> anyhow::Result<i32> {
    let mut lock = PluginsLock::load(ctx.home())?;

    // Require the plugin to exist in the registry OR in the lock so a
    // typo ("makakoo plugin enable watchdoh") can't silently create a
    // dangling entry. Lock-only matches are allowed (user disabled a
    // plugin whose directory they then manually removed).
    let registry = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let in_registry = registry.get(name).is_some();
    let in_lock = lock.get(name).is_some();
    if !in_registry && !in_lock {
        output::print_error(format!("plugin not installed: {name}"));
        return Ok(1);
    }

    let Some(entry) = lock.get(name).cloned() else {
        output::print_error(format!(
            "plugin {name} has no plugins.lock entry — reinstall to create one before toggling"
        ));
        return Ok(1);
    };

    if entry.enabled == target {
        output::print_info(format!(
            "{name} already {}",
            if target { "enabled" } else { "disabled" }
        ));
        return Ok(0);
    }

    let mut updated = entry;
    updated.enabled = target;
    lock.upsert(updated);
    lock.save(ctx.home())?;

    output::print_info(format!(
        "{name} {}",
        if target { "enabled" } else { "disabled" }
    ));
    if !target {
        output::print_info("restart the daemon (or next sancho tick) to deregister tasks");
    }
    Ok(0)
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
