//! `makakoo adapter list|info|spec` — Phase A.
//!
//! Minimal surface: enough to prove the manifest parser + registry walker
//! work end-to-end. Phase D expands to install/update/remove/status/doctor.

use std::path::{Path, PathBuf};

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};
use crossterm::style::Stylize;
use serde_json::json;

use makakoo_core::adapter::{
    call_adapter, AdapterRegistry, CallContext, Manifest,
};

use crate::cli::AdapterCmd;
use crate::context::CliContext;
use crate::output;

const ADAPTER_SPEC: &str = include_str!("../../../spec/ADAPTER_MANIFEST.md");

pub fn run(ctx: &CliContext, cmd: AdapterCmd) -> anyhow::Result<i32> {
    match cmd {
        AdapterCmd::List {
            json,
            include_bundled,
        } => list(json, include_bundled),
        AdapterCmd::Info { name, json } => info(&name, json),
        AdapterCmd::Spec => spec(),
        AdapterCmd::Call {
            name,
            prompt,
            timeout,
            bundled,
        } => call(ctx, &name, prompt, timeout, bundled),
    }
}

/// Walk the default registry dir + optionally the bundled reference dir,
/// emit one row per adapter. Bundled entries are flagged so they can't be
/// confused with truly-registered ones.
fn list(as_json: bool, include_bundled: bool) -> anyhow::Result<i32> {
    let registry = AdapterRegistry::load_default()
        .unwrap_or_else(|_| AdapterRegistry::load(PathBuf::new()).expect("empty"));

    let mut rows: Vec<AdapterRow> = registry
        .list()
        .map(|a| AdapterRow {
            name: a.manifest.adapter.name.clone(),
            version: a.manifest.adapter.version.to_string(),
            transport: transport_str(&a.manifest),
            roles: roles_str(&a.manifest),
            signed_by: a.manifest.security.signed_by.clone(),
            source_kind: SourceKind::Registered,
            hash_short: short_hash(&a.manifest.canonical_hash()),
            manifest_path: a.manifest_path.clone(),
        })
        .collect();

    if include_bundled {
        for bundled in load_bundled_adapters() {
            if rows.iter().any(|r| r.name == bundled.manifest.adapter.name) {
                continue; // already registered — don't double-list
            }
            rows.push(AdapterRow {
                name: bundled.manifest.adapter.name.clone(),
                version: bundled.manifest.adapter.version.to_string(),
                transport: transport_str(&bundled.manifest),
                roles: roles_str(&bundled.manifest),
                signed_by: bundled.manifest.security.signed_by.clone(),
                source_kind: SourceKind::Bundled,
                hash_short: short_hash(&bundled.manifest.canonical_hash()),
                manifest_path: bundled.manifest_path,
            });
        }
    }

    if as_json {
        let out: Vec<_> = rows
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "version": r.version,
                    "transport": r.transport,
                    "roles": r.roles,
                    "signed_by": r.signed_by,
                    "source": r.source_kind.as_str(),
                    "canonical_hash": r.hash_short,
                    "manifest_path": r.manifest_path.display().to_string(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }

    if rows.is_empty() {
        println!(
            "{}",
            "(no adapters registered — run `makakoo adapter list --include-bundled` to see reference adapters)".dark_grey()
        );
        return Ok(0);
    }

    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(vec![
        Cell::new("name").fg(TableColor::Cyan),
        Cell::new("version").fg(TableColor::Cyan),
        Cell::new("transport").fg(TableColor::Cyan),
        Cell::new("roles").fg(TableColor::Cyan),
        Cell::new("source").fg(TableColor::Cyan),
        Cell::new("hash").fg(TableColor::Cyan),
    ]);
    for r in rows {
        let source_cell = match r.source_kind {
            SourceKind::Registered => Cell::new("registered").fg(TableColor::Green),
            SourceKind::Bundled => Cell::new("bundled").fg(TableColor::Yellow),
        };
        t.add_row(vec![
            Cell::new(&r.name).fg(TableColor::White),
            Cell::new(r.version),
            Cell::new(r.transport),
            Cell::new(r.roles),
            source_cell,
            Cell::new(r.hash_short),
        ]);
    }
    println!("{t}");
    Ok(0)
}

fn info(name: &str, as_json: bool) -> anyhow::Result<i32> {
    let registry = AdapterRegistry::load_default()
        .unwrap_or_else(|_| AdapterRegistry::load(PathBuf::new()).expect("empty"));
    let mut resolved: Option<(Manifest, PathBuf, bool)> = registry
        .get(name)
        .map(|a| (a.manifest.clone(), a.manifest_path.clone(), false));
    if resolved.is_none() {
        for bundled in load_bundled_adapters() {
            if bundled.manifest.adapter.name == name {
                resolved = Some((bundled.manifest, bundled.manifest_path, true));
                break;
            }
        }
    }

    let Some((manifest, path, bundled)) = resolved else {
        output::print_error(format!("no adapter named `{name}`"));
        return Ok(1);
    };

    if as_json {
        let body = json!({
            "manifest": manifest,
            "manifest_path": path.display().to_string(),
            "bundled": bundled,
            "canonical_hash": manifest.canonical_hash(),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(0);
    }

    println!("{}", format!("─── Adapter: {} v{} ───", name, manifest.adapter.version).bold());
    println!("  source:         {}", if bundled { "bundled (not installed)" } else { "registered" });
    println!("  manifest:       {}", path.display());
    println!("  description:    {}", manifest.adapter.description);
    if let Some(h) = &manifest.adapter.homepage {
        println!("  homepage:       {}", h);
    }
    println!("  transport:      {}", transport_str(&manifest));
    println!("  output.format:  {:?}", manifest.output.format);
    println!("  roles:          {}", roles_str(&manifest));
    println!(
        "  sandbox:        {:?}",
        manifest.security.sandbox_profile
    );
    if let Some(s) = &manifest.security.signed_by {
        println!("  signed_by:      {}", s);
    }
    if manifest.security.requires_network {
        println!(
            "  allowed_hosts:  {}",
            manifest.security.allowed_hosts.join(", ")
        );
    }
    println!("  canonical_hash: {}", manifest.canonical_hash());
    Ok(0)
}

fn spec() -> anyhow::Result<i32> {
    print!("{ADAPTER_SPEC}");
    Ok(0)
}

/// Run the adapter with a prompt, emit a single JSON `ValidatorResult` on
/// stdout. Never exits nonzero on transport/parse failure — a verdict with
/// `INFRA_ERROR` status is still a valid result row for lope/swarm.
/// Exits nonzero only when the adapter cannot be resolved at all.
fn call(
    _ctx: &CliContext,
    name: &str,
    prompt: Option<String>,
    timeout: u64,
    bundled: bool,
) -> anyhow::Result<i32> {
    let registry = AdapterRegistry::load_default()
        .unwrap_or_else(|_| AdapterRegistry::load(PathBuf::new()).expect("empty"));
    let manifest: Manifest = if let Some(r) = registry.get(name) {
        r.manifest.clone()
    } else if bundled {
        match load_bundled_adapters().into_iter().find(|b| b.manifest.adapter.name == name) {
            Some(b) => b.manifest,
            None => {
                output::print_error(format!(
                    "no adapter named `{name}` (neither registered nor bundled)"
                ));
                return Ok(1);
            }
        }
    } else {
        output::print_error(format!(
            "no adapter named `{name}` registered — pass `--bundled` to call a reference adapter"
        ));
        return Ok(1);
    };

    // Resolve the prompt. Stdin read is blocking so we spin up a runtime
    // only when we actually need one.
    let resolved_prompt = match prompt {
        Some(p) => p,
        None => read_stdin_prompt()?,
    };

    let ctx_call = CallContext::default().with_timeout(timeout);
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async { call_adapter(&manifest, &resolved_prompt, ctx_call).await });

    println!("{}", serde_json::to_string(&result)?);
    Ok(0)
}

fn read_stdin_prompt() -> anyhow::Result<String> {
    use std::io::Read as _;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

struct AdapterRow {
    name: String,
    version: String,
    transport: String,
    roles: String,
    signed_by: Option<String>,
    source_kind: SourceKind,
    hash_short: String,
    manifest_path: PathBuf,
}

#[derive(Clone, Copy)]
enum SourceKind {
    Registered,
    Bundled,
}

impl SourceKind {
    fn as_str(self) -> &'static str {
        match self {
            SourceKind::Registered => "registered",
            SourceKind::Bundled => "bundled",
        }
    }
}

fn transport_str(m: &Manifest) -> String {
    use makakoo_core::adapter::TransportKind;
    match m.transport.kind {
        TransportKind::OpenAiCompatible => format!(
            "openai-compatible @ {}",
            m.transport.base_url.as_deref().unwrap_or("?")
        ),
        TransportKind::Subprocess => format!("subprocess {:?}", m.transport.command),
        TransportKind::McpStdio => format!("mcp-stdio {:?}", m.transport.command),
        TransportKind::McpHttp => format!(
            "mcp-http @ {}",
            m.transport.url.as_deref().unwrap_or("?")
        ),
    }
}

fn roles_str(m: &Manifest) -> String {
    let mut r: Vec<_> = m
        .capabilities
        .supports_roles
        .iter()
        .map(|r| r.as_str())
        .collect();
    r.sort();
    r.join(", ")
}

fn short_hash(full: &str) -> String {
    full.chars().take(12).collect()
}

fn _unused_hint(_: &Path) {} // silence import if Path goes unused

/// Resolve the bundled reference adapters dir. First tries
/// `$MAKAKOO_BUNDLED_ADAPTERS`, then the repo sibling of `CARGO_MANIFEST_DIR`,
/// then the install-time path the `makakoo` binary was shipped with
/// (`$MAKAKOO_HOME/plugins-core/adapters`). Missing dir = empty result.
fn bundled_adapters_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MAKAKOO_BUNDLED_ADAPTERS") {
        return Some(PathBuf::from(p));
    }
    // When running from the repo (tests, `cargo run`), CARGO_MANIFEST_DIR is
    // .../makakoo-os/makakoo — its parent is the workspace root.
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
    let candidate = workspace_root.join("plugins-core/adapters");
    if candidate.is_dir() {
        return Some(candidate);
    }
    None
}

struct BundledAdapter {
    manifest: Manifest,
    manifest_path: PathBuf,
}

fn load_bundled_adapters() -> Vec<BundledAdapter> {
    let Some(root) = bundled_adapters_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let manifest_path = entry.path().join("adapter.toml");
        if !manifest_path.is_file() {
            continue;
        }
        if let Ok(manifest) = Manifest::load(&manifest_path) {
            out.push(BundledAdapter {
                manifest,
                manifest_path,
            });
        }
    }
    out.sort_by(|a, b| {
        a.manifest
            .adapter
            .name
            .cmp(&b.manifest.adapter.name)
    });
    out
}
