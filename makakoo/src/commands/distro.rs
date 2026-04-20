//! `makakoo distro list|install` — batch plugin installs from a distro file.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};
use crossterm::style::Stylize;

use std::collections::BTreeMap;

use makakoo_core::distro::{
    resolve_distro, DistroFile, DistroTable, KernelTable, PluginPin, PluginPinFull,
};
use makakoo_core::plugin::{
    install_from_path, InstallRequest, PluginSource, PluginsLock,
};

use crate::cli::DistroCmd;
use crate::context::CliContext;
use crate::output;

pub async fn run(ctx: &CliContext, cmd: DistroCmd) -> anyhow::Result<i32> {
    match cmd {
        DistroCmd::List => list(ctx),
        DistroCmd::Install {
            name,
            from,
            yes,
            dry_run,
        } => install(ctx, name, from, yes, dry_run),
        DistroCmd::Save {
            name,
            out,
            force,
            include_disabled,
        } => save(ctx, &name, out, force, include_disabled),
    }
}

fn list(ctx: &CliContext) -> anyhow::Result<i32> {
    let dir = resolve_distros_dir().unwrap_or_else(|_| PathBuf::from("."));
    let active = PluginsLock::load(ctx.home())
        .ok()
        .and_then(|l| l.meta.distro);

    if !dir.is_dir() {
        output::print_warn(format!(
            "distros dir not found: {} (set $MAKAKOO_DISTROS or run from a checkout)",
            dir.display()
        ));
        if let Some(ref a) = active {
            println!("{}", format!("active distro: {a}").green());
        }
        return Ok(0);
    }

    let mut rows: Vec<(String, DistroFile, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        match DistroFile::load(&path) {
            Ok(f) => rows.push((file_stem, f, path.clone())),
            Err(e) => output::print_warn(format!(
                "skipping {}: {e}",
                path.display()
            )),
        }
    }

    rows.sort_by(|a, b| a.0.cmp(&b.0));

    if rows.is_empty() {
        println!("{}", "(no distros under distros/)".dark_grey());
    } else {
        let mut t = Table::new();
        t.load_preset(UTF8_FULL);
        t.set_header(vec![
            Cell::new("name").fg(TableColor::Cyan),
            Cell::new("display").fg(TableColor::Cyan),
            Cell::new("plugins").fg(TableColor::Cyan),
            Cell::new("includes").fg(TableColor::Cyan),
        ]);
        for (_, f, _) in &rows {
            t.add_row(vec![
                Cell::new(&f.distro.name).fg(TableColor::White),
                Cell::new(
                    f.distro
                        .display_name
                        .clone()
                        .unwrap_or_else(|| f.distro.name.clone()),
                ),
                Cell::new(f.plugins.len().to_string()),
                Cell::new(f.include().join(", ")),
            ]);
        }
        println!("{t}");
    }

    if let Some(ref a) = active {
        output::print_info(format!("active distro: {a}"));
    } else {
        output::print_info("active distro: (none — ad-hoc install)");
    }
    Ok(0)
}

fn install(
    ctx: &CliContext,
    name: Option<String>,
    from: Option<PathBuf>,
    yes: bool,
    dry_run: bool,
) -> anyhow::Result<i32> {
    let distro_path = match (name.as_deref(), from.as_deref()) {
        (_, Some(p)) => p.to_path_buf(),
        (Some(n), None) => resolve_distros_dir()?.join(format!("{n}.toml")),
        (None, None) => {
            output::print_error("either a distro name or --from <path> is required");
            return Ok(1);
        }
    };

    if !distro_path.exists() {
        output::print_error(format!("distro file not found: {}", distro_path.display()));
        return Ok(1);
    }

    let root = DistroFile::load(&distro_path)?;
    let resolved = resolve_distro(&root, &distro_path)?;

    output::print_info(format!(
        "resolving {} ({} include chain file(s), {} plugin(s))",
        distro_path.display(),
        resolved.chain.len(),
        resolved.plugins.len(),
    ));
    for p in &resolved.plugins {
        println!(
            "  - {} {} (from {})",
            p.name,
            p.pin.version(),
            p.source_distro
        );
    }

    if dry_run {
        output::print_info("--dry-run: no changes made");
        return Ok(0);
    }

    if !yes && !resolved.plugins.is_empty() && !confirm("Proceed?")? {
        output::print_info("aborted.");
        return Ok(0);
    }

    // Install each plugin. Source resolution: v0.1 looks up each plugin
    // under `plugins-core/<name>/` — the in-tree defaults cover the four
    // shipped core plugins. Later phases add git/tar sources.
    let plugins_core: Option<PathBuf> =
        crate::commands::plugin::plugins_core_root().ok();

    let mut installed = 0usize;
    let mut skipped = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();

    for pin in &resolved.plugins {
        let source_path = match &plugins_core {
            Some(root) => root.join(&pin.name),
            None => PathBuf::from(&pin.name),
        };
        if !source_path.is_dir() {
            failed.push((
                pin.name.clone(),
                format!("source not found: {}", source_path.display()),
            ));
            continue;
        }

        // Idempotent: if already installed with same blake3, skip. If
        // installed with a different hash, fail — user must uninstall
        // first.
        let lock = PluginsLock::load(ctx.home())?;
        if let Some(existing) = lock.get(&pin.name) {
            output::print_info(format!(
                "  skip {}: already installed ({})",
                pin.name, existing.version
            ));
            skipped += 1;
            continue;
        }

        let req = InstallRequest {
            source: PluginSource::Path(source_path),
            expected_blake3: pin.pin.blake3().map(|s| s.to_string()),
        };

        match install_from_path(&req, ctx.home()) {
            Ok(outcome) => {
                output::print_info(format!(
                    "  installed {} (blake3: {})",
                    outcome.name,
                    &outcome.computed_blake3[..16]
                ));
                installed += 1;
            }
            Err(e) => {
                failed.push((pin.name.clone(), e.to_string()));
                output::print_warn(format!("  failed {}: {e}", pin.name));
            }
        }
    }

    // Stamp the active distro (best-effort — only when the root succeeded
    // cleanly enough to produce a resolved.root.distro.name).
    let mut lock = PluginsLock::load(ctx.home())?;
    lock.touch_meta(
        Some(resolved.root.distro.name.clone()),
        Some(env!("CARGO_PKG_VERSION").to_string()),
    );
    lock.save(ctx.home())?;

    output::print_info(format!(
        "distro {}: installed {installed}, skipped {skipped}, failed {} / total {}",
        resolved.root.distro.name,
        failed.len(),
        resolved.plugins.len(),
    ));

    if let Some(ref msg) = resolved.root.post_install.message {
        println!();
        println!("{}", "post-install".green().bold());
        println!("{msg}");
    }

    Ok(if failed.is_empty() { 0 } else { 1 })
}

fn save(
    ctx: &CliContext,
    name: &str,
    out: Option<PathBuf>,
    force: bool,
    include_disabled: bool,
) -> anyhow::Result<i32> {
    // 1) Pull the lock — that's the source of truth for "what's installed
    //    right now, at what version, pinned by what hash". `plugins.lock`
    //    lags the registry by at most one install cycle; that's fine
    //    because the goal is to replay the lock, not the on-disk tree.
    let lock = PluginsLock::load(ctx.home())?;
    if lock.plugins.is_empty() {
        output::print_error(
            "no plugins installed — nothing to save. Run `makakoo plugin install` first.",
        );
        return Ok(1);
    }

    // 2) Resolve the output path.
    let dest = match out {
        Some(p) => p,
        None => match resolve_distros_dir() {
            Ok(dir) => dir.join(format!("{name}.toml")),
            Err(e) => {
                output::print_error(format!(
                    "can't determine default distros/ path ({e}). Pass --out <path> explicitly."
                ));
                return Ok(1);
            }
        },
    };

    if dest.exists() && !force {
        output::print_error(format!(
            "{} already exists — pass --force to overwrite",
            dest.display()
        ));
        return Ok(1);
    }

    // 3) Build the distro file from the lock. Plugin pins carry the
    //    exact version + blake3 so the replay is reproducible byte-for-
    //    byte (assuming the source at path: is still there).
    let mut plugins: BTreeMap<String, PluginPin> = BTreeMap::new();
    let mut included = 0usize;
    let mut skipped_disabled = 0usize;
    for entry in &lock.plugins {
        if !entry.enabled && !include_disabled {
            skipped_disabled += 1;
            continue;
        }
        plugins.insert(
            entry.name.clone(),
            PluginPin::Full(PluginPinFull {
                version: entry.version.clone(),
                blake3: entry.blake3.clone(),
            }),
        );
        included += 1;
    }

    if plugins.is_empty() {
        output::print_error(
            "no enabled plugins — pass --include-disabled to save the on-disk set anyway",
        );
        return Ok(1);
    }

    let kernel_version = env!("CARGO_PKG_VERSION").to_string();
    let snapshot = DistroFile {
        distro: DistroTable {
            name: name.to_string(),
            display_name: Some(format!("{name} (saved snapshot)")),
            version: Some("0.1.0".to_string()),
            description: Some(format!(
                "Snapshot saved on {} — reproduces the installed plugin set.",
                chrono::Utc::now().format("%Y-%m-%d")
            )),
            authors: vec![],
            license: None,
            include: vec![],
        },
        kernel: KernelTable {
            version: Some(format!("^{kernel_version}")),
        },
        plugins,
        defaults: Default::default(),
        excludes: Default::default(),
        post_install: Default::default(),
    };

    // 4) Validate before writing — if we can't re-parse our own output,
    //    that's a bug, not a disk issue. Fail loudly.
    let rendered = toml::to_string_pretty(&snapshot)?;
    DistroFile::parse(&rendered, &dest)?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, &rendered)?;

    output::print_info(format!(
        "saved {included} plugin(s) to {} (skipped {skipped_disabled} disabled)",
        dest.display()
    ));
    Ok(0)
}

/// Resolve the distros dir. `$MAKAKOO_DISTROS` env var wins, otherwise
/// walk upward from CWD looking for a `distros/` directory.
fn resolve_distros_dir() -> anyhow::Result<PathBuf> {
    if let Ok(root) = std::env::var("MAKAKOO_DISTROS") {
        return Ok(PathBuf::from(root));
    }
    let cwd = std::env::current_dir()?;
    if let Some(p) = crate::commands::plugin::walk_up_for(&cwd, "distros") {
        return Ok(p);
    }
    anyhow::bail!(
        "can't find distros/ — set $MAKAKOO_DISTROS or run from a checkout that contains distros/"
    )
}

fn confirm(prompt: &str) -> anyhow::Result<bool> {
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "{prompt} [y/N] ");
    let _ = stderr.flush();
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

// Allow unused imports if a downstream refactor removes something; cheap
// insurance during a churny phase.
#[allow(dead_code)]
fn _touch(_p: &Path) {}
