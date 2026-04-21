//! `makakoo plugin list|info|install|uninstall` — user-facing lifecycle.

use std::path::{Path, PathBuf};

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};
use crossterm::style::Stylize;
use serde_json::json;

use std::io::{self, Write};

use makakoo_core::capability::resolve_grants;
use makakoo_core::plugin::staging::StagingError;
use makakoo_core::plugin::{
    apply_update as core_apply_update, drop_probe, install as core_install, install_from_path,
    list_updatable, probe_upstream, uninstall as core_uninstall, InstallError, InstallRequest,
    LockEntry, Manifest, PluginRegistry, PluginSource, PluginsLock, ProbeDrift,
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
            sha256,
            allow_unstable_ref,
        } => install(ctx, &source, core, blake3, sha256, allow_unstable_ref),
        PluginCmd::Uninstall { name, purge } => uninstall(ctx, &name, purge),
        PluginCmd::Enable { name } => set_enabled(ctx, &name, true),
        PluginCmd::Disable { name } => set_enabled(ctx, &name, false),
        PluginCmd::Update { name, all, yes } => {
            if all {
                update_all(ctx, yes)
            } else {
                update(ctx, name.as_deref().unwrap_or_default(), yes)
            }
        }
        PluginCmd::Outdated { json } => outdated(ctx, json),
        PluginCmd::Sync { dry_run, force } => sync(ctx, dry_run, force),
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
    sha256: Option<String>,
    allow_unstable_ref: bool,
) -> anyhow::Result<i32> {
    let plugin_source = match parse_install_source(source, use_core, sha256, allow_unstable_ref) {
        Ok(s) => s,
        Err(msg) => {
            output::print_error(msg);
            return Ok(1);
        }
    };

    if let PluginSource::Path(ref p) = plugin_source {
        if !p.exists() {
            output::print_error(format!("source does not exist: {}", p.display()));
            return Ok(1);
        }
    }
    if let PluginSource::Git {
        ref ref_,
        allow_unstable,
        ..
    } = plugin_source
    {
        if allow_unstable {
            output::print_warn(format!(
                "installing from unstable git ref `{ref_}` — pass a semver tag or 40-char SHA to pin"
            ));
        }
    }

    let req = InstallRequest {
        source: plugin_source,
        expected_blake3: blake3,
    };

    match core_install(&req, ctx.home()) {
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

/// Parse the `<source>` argument of `plugin install` into a `PluginSource`.
///
/// Order of recognition (first match wins):
///   1. `--core` → plugins-core lookup via `resolve_plugins_core`.
///   2. Starts with `git+` → git URL (optionally `@<ref>`).
///   3. Scheme `http://` or `https://` → tarball (requires `--sha256`).
///   4. Anything else → treated as a local path.
fn parse_install_source(
    raw: &str,
    use_core: bool,
    sha256: Option<String>,
    allow_unstable_ref: bool,
) -> Result<PluginSource, String> {
    if use_core {
        let p = resolve_plugins_core(raw).map_err(|e| format!("resolve plugins-core: {e}"))?;
        return Ok(PluginSource::Path(p));
    }
    if let Some(rest) = raw.strip_prefix("git+") {
        // git+<url>[@<ref>]. Split on the LAST '@' to avoid splitting
        // on SSH-style `git@host:...` authority (which shouldn't appear
        // after `git+` but keep the parser defensive).
        let (url, ref_) = match rest.rfind('@') {
            Some(i) if i > "https://".len() => (
                rest[..i].to_string(),
                rest[i + 1..].to_string(),
            ),
            _ => (rest.to_string(), "HEAD".to_string()),
        };
        return Ok(PluginSource::Git {
            url,
            ref_,
            allow_unstable: allow_unstable_ref,
        });
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        let sha = sha256.ok_or_else(|| {
            "tarball install requires --sha256=<hex>: refusing to install unverified archive"
                .to_string()
        })?;
        return Ok(PluginSource::Tarball {
            url: raw.to_string(),
            sha256: sha,
        });
    }
    Ok(PluginSource::Path(PathBuf::from(raw)))
}

fn update(ctx: &CliContext, name: &str, yes: bool) -> anyhow::Result<i32> {
    let lock = PluginsLock::load(ctx.home())?;
    let Some(entry) = lock.get(name).cloned() else {
        output::print_error(format!("plugin not installed: {name}"));
        return Ok(1);
    };

    if entry.source.starts_with("path:") {
        return update_from_path(ctx, &entry);
    }
    if entry.source.starts_with("git:") {
        return update_from_git(ctx, &entry, yes);
    }
    if entry.source.starts_with("tar:") {
        output::print_error(format!(
            "plugin {name} is tarball-sourced — `update` cannot auto-pin a new sha256. Reinstall with `plugin install {} --sha256=<new-hex>` after verifying the upstream hash.",
            entry.source.strip_prefix("tar:").unwrap_or("<url>")
        ));
        return Ok(1);
    }
    output::print_error(format!(
        "plugin {name} has an unrecognized lock source `{}` — reinstall manually",
        entry.source
    ));
    Ok(1)
}

/// Legacy path-sourced update: uninstall + reinstall from the recorded
/// local directory. Preserves enabled flag across round-trip.
fn update_from_path(ctx: &CliContext, entry: &LockEntry) -> anyhow::Result<i32> {
    let name = &entry.name;
    let Some(source_path) = entry.source.strip_prefix("path:") else {
        unreachable!("caller verified prefix");
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

    if let Err(e) = core_uninstall(name, ctx.home(), false) {
        output::print_error(format!("uninstall step failed: {e}"));
        return Ok(1);
    }

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
        if prior_enabled {
            "yes"
        } else {
            "no (preserved from prior state)"
        }
    ));
    Ok(0)
}

/// Git-sourced update: probe upstream, diff manifest_hash, prompt on
/// drift (or skip with `--yes`), apply.
fn update_from_git(ctx: &CliContext, entry: &LockEntry, yes: bool) -> anyhow::Result<i32> {
    let name = &entry.name;
    let probe = match probe_upstream(entry) {
        Ok(p) => p,
        Err(e) => {
            output::print_error(format!("upstream probe failed for {name}: {e}"));
            return Ok(1);
        }
    };

    match probe.drift {
        ProbeDrift::UpToDate => {
            output::print_info(format!(
                "{name} up to date (sha {short})",
                short = short_sha(&probe.new_resolved_sha)
            ));
            drop_probe(probe);
            return Ok(0);
        }
        ProbeDrift::ContentOnly => {
            output::print_info(format!(
                "{name}: upstream drifted {old} → {new} (manifest unchanged; reinstalling)",
                old = short_sha_opt(probe.old_resolved_sha.as_deref()),
                new = short_sha(&probe.new_resolved_sha),
            ));
        }
        ProbeDrift::ManifestChange => {
            println!(
                "plugin {name} manifest changed upstream (sha {old} → {new})",
                old = short_sha_opt(probe.old_resolved_sha.as_deref()),
                new = short_sha(&probe.new_resolved_sha),
            );
            println!("  manifest_hash: {:?}", probe.old_manifest_hash);
            println!("             → {}", probe.new_manifest_hash);
            if !yes && !prompt_yes_no("Re-trust and apply update?") {
                output::print_info("update declined — installed version unchanged");
                drop_probe(probe);
                return Ok(0);
            }
        }
    }

    match core_apply_update(probe, ctx.home()) {
        Ok(outcome) => {
            output::print_info(format!(
                "updated {} → blake3 {}",
                outcome.name, outcome.computed_blake3
            ));
            Ok(0)
        }
        Err(e) => {
            output::print_error(format!(
                "apply_update failed — plugin may be uninstalled: {e}"
            ));
            Ok(1)
        }
    }
}

/// `plugin update --all [--yes]` — walk every git-sourced entry, update
/// each, print a summary.
fn update_all(ctx: &CliContext, yes: bool) -> anyhow::Result<i32> {
    let candidates = list_updatable(ctx.home())?;
    if candidates.is_empty() {
        output::print_info("no git-sourced plugins installed — nothing to update");
        return Ok(0);
    }
    let mut up_to_date = 0usize;
    let mut updated = 0usize;
    let mut failed = 0usize;
    for entry in &candidates {
        if entry.source.starts_with("tar:") {
            // Tarballs can't auto-update without a fresh --sha256.
            output::print_warn(format!(
                "{} is tarball-sourced; skip (reinstall with new --sha256)",
                entry.name
            ));
            continue;
        }
        match update_from_git(ctx, entry, yes)? {
            0 => {
                // Success path covers both up-to-date + updated; figure
                // out which by re-reading the lock.
                if let Some(new_entry) = PluginsLock::load(ctx.home())?.get(&entry.name) {
                    if new_entry.resolved_sha == entry.resolved_sha {
                        up_to_date += 1;
                    } else {
                        updated += 1;
                    }
                }
            }
            _ => failed += 1,
        }
    }
    output::print_info(format!(
        "{up_to_date} up-to-date, {updated} updated, {failed} failed"
    ));
    Ok(if failed > 0 { 1 } else { 0 })
}

/// `plugin outdated` — pure dry-run. Prints a table of drift info.
fn outdated(ctx: &CliContext, as_json: bool) -> anyhow::Result<i32> {
    let candidates = list_updatable(ctx.home())?;
    if candidates.is_empty() {
        output::print_info("no git-sourced plugins installed");
        return Ok(0);
    }
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for entry in &candidates {
        match probe_upstream(entry) {
            Ok(probe) => {
                let drifted = probe.drift != ProbeDrift::UpToDate;
                rows.push(serde_json::json!({
                    "name": entry.name,
                    "source": entry.source,
                    "current": short_sha_opt(entry.resolved_sha.as_deref()),
                    "upstream": short_sha(&probe.new_resolved_sha),
                    "drift": drifted,
                    "drift_type": match probe.drift {
                        ProbeDrift::UpToDate => "none",
                        ProbeDrift::ContentOnly => "content",
                        ProbeDrift::ManifestChange => "manifest",
                    },
                }));
                drop_probe(probe);
            }
            Err(e) => {
                rows.push(serde_json::json!({
                    "name": entry.name,
                    "source": entry.source,
                    "error": e.to_string(),
                }));
            }
        }
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(vec![
        Cell::new("name").fg(TableColor::Cyan),
        Cell::new("current").fg(TableColor::Cyan),
        Cell::new("upstream").fg(TableColor::Cyan),
        Cell::new("drift").fg(TableColor::Cyan),
    ]);
    for r in &rows {
        let drifted = r.get("drift").and_then(|v| v.as_bool()).unwrap_or(false);
        let drift_label = if r.get("error").is_some() {
            Cell::new("error").fg(TableColor::Red)
        } else if drifted {
            let label = r
                .get("drift_type")
                .and_then(|v| v.as_str())
                .unwrap_or("yes");
            Cell::new(label).fg(TableColor::Yellow)
        } else {
            Cell::new("no").fg(TableColor::Green)
        };
        t.add_row(vec![
            Cell::new(r.get("name").and_then(|v| v.as_str()).unwrap_or("?")),
            Cell::new(r.get("current").and_then(|v| v.as_str()).unwrap_or("-")),
            Cell::new(r.get("upstream").and_then(|v| v.as_str()).unwrap_or("-")),
            drift_label,
        ]);
    }
    println!("{t}");
    Ok(0)
}

fn short_sha(s: &str) -> String {
    s.chars().take(7).collect()
}

fn short_sha_opt(s: Option<&str>) -> String {
    match s {
        Some(v) => short_sha(v),
        None => "-".into(),
    }
}

fn prompt_yes_no(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Batch reinstall every plugin in `plugins-core/` into $MAKAKOO_HOME/plugins/.
///
/// Walks the plugins-core/ source tree, reads each plugin's manifest to
/// pull its prior enabled flag off the current lock (default: enabled),
/// calls `install_from_path` per-plugin, and restores the enabled flag
/// if it was disabled. Per-plugin failures (missing manifest, malformed
/// toml, native-task collision) log + skip rather than aborting the
/// batch — one bad plugin can't block the rest.
fn sync(ctx: &CliContext, dry_run: bool, force: bool) -> anyhow::Result<i32> {
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
        let install_result = install_from_path(&req, ctx.home());
        let install_result = match install_result {
            Ok(o) => Ok(o),
            // Guard on the exact variant rather than matching the
            // display string — immune to future StagingError variants
            // that happen to mention "already" in their message.
            Err(InstallError::Staging(StagingError::TargetExists { .. })) if force => {
                // --force: uninstall + reinstall. Must be race-safe
                // against another concurrent sync, otherwise the retry
                // loop could silently wipe a plugin the other process
                // just placed. Take a per-plugin lock file that lives
                // for the full uninstall+reinstall window. If it's
                // already held, surface ConcurrentSync — caller sees
                // the error in the `failures:` summary and can retry.
                match acquire_sync_lock(ctx.home(), &name) {
                    Ok(_lock_guard) => {
                        match core_uninstall(&name, ctx.home(), false) {
                            Ok(_) => install_from_path(&req, ctx.home()),
                            Err(ue) => Err(InstallError::UninstallFailed {
                                plugin: name.clone(),
                                source: Box::new(ue),
                            }),
                        }
                        // lock_guard drops at end of match arm,
                        // releasing the file.
                    }
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        };

        match install_result {
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

/// RAII lock file for the `plugin sync --force` uninstall+reinstall
/// window. Written as JSON with PID + timestamp so stale locks from
/// crashed processes get reaped instead of blocking future syncs
/// forever (kill -9, panic, power loss all leave zombie empty-files
/// without liveness metadata — that's what pi flagged in Phase 2 v2
/// review as the "zombie lock" failure mode).
///
/// Acquire sequence:
///   1. If no lock file → create with our PID + now, return guard.
///   2. If lock file exists → read + parse. If parse fails or PID is
///      dead, unlink and retry step 1 once. If PID is alive, return
///      `ConcurrentSync`.
///
/// Drop unlinks the file unconditionally (whether install succeeded
/// or failed — the lock is just a mutex, not a transaction log).
#[derive(Debug)]
struct SyncLockGuard {
    path: PathBuf,
}

impl Drop for SyncLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct SyncLockFile {
    pid: u32,
    acquired_at: u64, // seconds since epoch
}

/// Best-effort liveness check. POSIX `kill(pid, 0)` returns 0 if the
/// target process exists and the caller has permission to signal it.
/// On Windows we conservatively treat every existing lock as live
/// (Windows sync flow lands with Phase J signing + winget).
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    // kill(pid, 0) — probe without sending a real signal.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    let err = std::io::Error::last_os_error();
    // EPERM: process exists but we can't signal it — still alive.
    // ESRCH: no such process — dead.
    err.raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    true
}

fn acquire_sync_lock(home: &Path, name: &str) -> Result<SyncLockGuard, InstallError> {
    let lock_dir = home.join("plugins").join(".stage").join("locks");
    if let Err(e) = std::fs::create_dir_all(&lock_dir) {
        return Err(InstallError::Io {
            path: lock_dir,
            source: e,
        });
    }
    let lock_path = lock_dir.join(format!("{name}.sync.lock"));

    // Two attempts: first try create_new, then if the existing file
    // is stale, unlink and try once more.
    for attempt in 0..2 {
        match try_create_lock(&lock_path) {
            Ok(g) => return Ok(g),
            Err(LockError::Exists) => {
                // On the first attempt, try to reap a stale lock. On
                // the second attempt (after we already unlinked once),
                // the file must have been created by a live peer — give up.
                if attempt == 1 {
                    return Err(InstallError::ConcurrentSync {
                        name: name.to_string(),
                    });
                }
                match read_and_classify_lock(&lock_path) {
                    LockClassification::Live => {
                        return Err(InstallError::ConcurrentSync {
                            name: name.to_string(),
                        });
                    }
                    LockClassification::Stale | LockClassification::Unparseable => {
                        // Reap and loop for a fresh acquire attempt.
                        let _ = std::fs::remove_file(&lock_path);
                    }
                }
            }
            Err(LockError::Io(e)) => {
                return Err(InstallError::Io {
                    path: lock_path,
                    source: e,
                });
            }
        }
    }
    // Unreachable — the loop above returns on every path.
    Err(InstallError::ConcurrentSync {
        name: name.to_string(),
    })
}

enum LockError {
    Exists,
    Io(std::io::Error),
}

enum LockClassification {
    Live,
    Stale,
    Unparseable,
}

fn try_create_lock(lock_path: &Path) -> Result<SyncLockGuard, LockError> {
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(mut f) => {
            let body = SyncLockFile {
                pid: std::process::id(),
                acquired_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            };
            let _ = f.write_all(serde_json::to_string(&body).unwrap_or_default().as_bytes());
            Ok(SyncLockGuard {
                path: lock_path.to_path_buf(),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(LockError::Exists),
        Err(e) => Err(LockError::Io(e)),
    }
}

fn read_and_classify_lock(lock_path: &Path) -> LockClassification {
    let body = match std::fs::read_to_string(lock_path) {
        Ok(s) => s,
        Err(_) => return LockClassification::Unparseable,
    };
    if body.trim().is_empty() {
        // Legacy empty-file sentinel from pre-PID-upgrade sync. Stale.
        return LockClassification::Stale;
    }
    let parsed: Result<SyncLockFile, _> = serde_json::from_str(&body);
    match parsed {
        Ok(f) if pid_is_alive(f.pid) => LockClassification::Live,
        Ok(_) => LockClassification::Stale,
        Err(_) => LockClassification::Unparseable,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Locking a plugin name once succeeds. Locking it again before
    /// the guard drops raises `ConcurrentSync`. Dropping the first
    /// guard releases the lock so a third acquire succeeds.
    ///
    /// Locks the race pi flagged in Phase 2 review: without this guard
    /// `plugin sync --force` could uninstall + reinstall on top of a
    /// concurrent sync, silently wiping whatever the other process
    /// just placed. The guard file forces the second sync to bail out
    /// visibly with ConcurrentSync rather than racing.
    #[test]
    fn acquire_sync_lock_prevents_concurrent_retries() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        let first = acquire_sync_lock(&home, "skill-foo").expect("initial lock must succeed");

        let err = acquire_sync_lock(&home, "skill-foo")
            .expect_err("second lock during held window must error");
        assert!(
            matches!(err, InstallError::ConcurrentSync { ref name } if name == "skill-foo"),
            "expected ConcurrentSync, got {err:?}"
        );

        // A different name in the same home is independently lockable.
        let sibling = acquire_sync_lock(&home, "skill-bar").expect("different name must succeed");
        drop(sibling);

        // Releasing the first guard lets a third acquire succeed.
        drop(first);
        let third = acquire_sync_lock(&home, "skill-foo").expect("re-acquire after drop ok");
        drop(third);
    }

    /// Zombie-lock recovery: if a lock file exists with a PID that
    /// is no longer alive (process crashed, kill -9, panic, power
    /// loss), the next acquire must reap the stale file instead of
    /// failing forever with ConcurrentSync. pi flagged this as the
    /// "empty-file sentinel is unreliable in production" issue in
    /// the Phase 2 v2 review.
    #[test]
    #[cfg(unix)]
    fn acquire_sync_lock_reaps_stale_pid_file() {
        use std::io::Write;

        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        // Simulate a zombie lock by writing a JSON file with a PID
        // that is guaranteed not to exist. 1 is init/launchd which IS
        // alive, so pick 0 — POSIX reserves 0 for the current process
        // group and `kill(0, 0)` on macOS returns ESRCH, classifying
        // the lock as stale.
        //
        // More defensive: use a PID 2^31-1 which is above the configured
        // pid_max on every platform we ship to.
        let lock_dir = home.join("plugins").join(".stage").join("locks");
        std::fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("skill-zombie.sync.lock");
        let stale = SyncLockFile {
            pid: 2_147_483_640, // above pid_max on any sane kernel
            acquired_at: 0,
        };
        let mut f = std::fs::File::create(&lock_path).unwrap();
        f.write_all(serde_json::to_string(&stale).unwrap().as_bytes())
            .unwrap();
        drop(f);
        assert!(lock_path.exists(), "pre-test: stale lock must be on disk");

        // Acquire should succeed — it reads the lock, sees the PID is
        // dead, unlinks, and writes a fresh one in its place.
        let guard = acquire_sync_lock(&home, "skill-zombie")
            .expect("stale PID lock must be reaped, not block forever");

        // Double-check: the replacement lock file carries OUR pid.
        let body = std::fs::read_to_string(&lock_path).unwrap();
        let parsed: SyncLockFile = serde_json::from_str(&body).unwrap();
        assert_eq!(
            parsed.pid,
            std::process::id(),
            "reaped lock must be replaced by current-pid lock"
        );

        drop(guard);
    }

    /// Empty legacy lock files (from the v1 sentinel-file scheme
    /// before pi demanded PID metadata) must also be reaped.
    #[test]
    fn acquire_sync_lock_reaps_empty_legacy_lock() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        let lock_dir = home.join("plugins").join(".stage").join("locks");
        std::fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("skill-legacy.sync.lock");
        std::fs::File::create(&lock_path).unwrap(); // empty file

        let guard = acquire_sync_lock(&home, "skill-legacy")
            .expect("empty legacy lock file must be treated as stale");

        let body = std::fs::read_to_string(&lock_path).unwrap();
        assert!(!body.is_empty(), "reaped lock must be replaced with real metadata");

        drop(guard);
    }
}
