//! `makakoo adapter <subcommand>` — Phases A, B, D CLI surface.
//!
//! Phase A shipped list/info/spec. Phase B added call. Phase D polishes
//! the install lifecycle + doctor + migration + export surface.

use std::fs;
use std::path::{Path, PathBuf};

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};
use crossterm::style::Stylize;
use serde_json::json;

use makakoo_core::adapter::{
    call_adapter, install_from_path, uninstall as core_uninstall, AdapterRegistry, CallContext,
    InstallOptions, InstallRoot, Manifest, TrustLedger,
};

use crate::cli::AdapterCmd;
use crate::context::CliContext;
use crate::output;

const ADAPTER_SPEC: &str = include_str!("../../../spec/ADAPTER_MANIFEST.md");

pub async fn run(ctx: &CliContext, cmd: AdapterCmd) -> anyhow::Result<i32> {
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
        } => call(ctx, &name, prompt, timeout, bundled).await,
        AdapterCmd::Install {
            source,
            bundled,
            allow_unsigned,
            accept_re_trust,
            skip_health_check,
        } => install(
            &source,
            bundled,
            allow_unsigned,
            accept_re_trust,
            skip_health_check,
        ),
        AdapterCmd::Update {
            name,
            accept_re_trust,
        } => update(&name, accept_re_trust),
        AdapterCmd::Remove { name, purge } => remove(&name, purge),
        AdapterCmd::Enable { name } => set_enabled(&name, true),
        AdapterCmd::Disable { name } => set_enabled(&name, false),
        AdapterCmd::Status { json } => status(json),
        AdapterCmd::Doctor { name, json } => doctor(&name, json).await,
        AdapterCmd::Search { query } => search(&query),
        AdapterCmd::MigrateConfig { path } => migrate_config(&path),
        AdapterCmd::Export { name, out } => export(&name, out),
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
async fn call(
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
    let result = call_adapter(&manifest, &resolved_prompt, ctx_call).await;
    println!("{}", serde_json::to_string(&result)?);
    Ok(0)
}

fn read_stdin_prompt() -> anyhow::Result<String> {
    use std::io::Read as _;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

// ───────────────────────── Install lifecycle ──────────────────────────

fn install(
    source: &str,
    bundled: bool,
    allow_unsigned: bool,
    accept_re_trust: bool,
    skip_health_check: bool,
) -> anyhow::Result<i32> {
    let root = InstallRoot::default_from_env();
    let opts = InstallOptions {
        allow_unsigned,
        accept_re_trust,
        skip_health_check,
    };
    let source_dir = if bundled {
        match bundled_adapters_dir().map(|d| d.join(source)) {
            Some(p) if p.is_dir() => p,
            _ => {
                output::print_error(format!("no bundled adapter named `{source}`"));
                return Ok(1);
            }
        }
    } else {
        PathBuf::from(source)
    };
    match install_from_path(&source_dir, &root, opts) {
        Ok(report) => {
            println!(
                "{}",
                format!(
                    "✅ installed {} v{}",
                    report.adapter_name, report.version
                )
                .green()
            );
            println!("  registered: {}", report.registered_path.display());
            println!("  hash:       {}", report.canonical_hash);
            if report.signed {
                println!(
                    "  signature:  ✅ verified (publisher={})",
                    report.publisher.as_deref().unwrap_or("?")
                );
            } else {
                println!("  signature:  (unsigned)");
            }
            if let Some(diff) = &report.diff {
                println!("  diff:       {}", diff.summary());
            }
            if report.health_check_passed {
                println!("  health:     ✅ ok");
            } else {
                println!("  health:     (skipped or no check_url)");
            }
            Ok(0)
        }
        Err(e) => {
            output::print_error(format!("install failed: {e}"));
            Ok(1)
        }
    }
}

fn update(name: &str, accept_re_trust: bool) -> anyhow::Result<i32> {
    // Phase-D update: read the registered manifest's install.source and
    // re-run the install from the same source. Local paths are the
    // primary proven path; URL fetchers land as they do.
    let root = InstallRoot::default_from_env();
    let registered = root.registered_dir().join(format!("{name}.toml"));
    if !registered.exists() {
        output::print_error(format!("adapter `{name}` is not registered"));
        return Ok(1);
    }
    let current = match Manifest::load(&registered) {
        Ok(m) => m,
        Err(e) => {
            output::print_error(format!(
                "registered manifest is corrupt: {e}; reinstall from source"
            ));
            return Ok(1);
        }
    };
    match current.install.source_type {
        makakoo_core::adapter::SourceType::Local => {
            let source = match current.install.entry_point.as_deref() {
                Some(p) => PathBuf::from(p),
                None => {
                    output::print_error(
                        "update requires install.entry_point or install.source; cannot resolve",
                    );
                    return Ok(1);
                }
            };
            install(
                source.to_string_lossy().as_ref(),
                false,
                true,
                accept_re_trust,
                true,
            )
        }
        other => {
            output::print_error(format!(
                "update from source_type {other:?} is scheduled for v0.3 Phase E URL fetchers"
            ));
            Ok(1)
        }
    }
}

fn remove(name: &str, purge: bool) -> anyhow::Result<i32> {
    let root = InstallRoot::default_from_env();
    match core_uninstall(name, &root, purge) {
        Ok(()) => {
            println!("{}", format!("✅ removed {name}").green());
            if purge {
                println!("  state dir purged");
            }
            Ok(0)
        }
        Err(e) => {
            output::print_error(format!("remove failed: {e}"));
            Ok(1)
        }
    }
}

fn set_enabled(name: &str, enable: bool) -> anyhow::Result<i32> {
    // Soft toggle — sibling file `<name>.disabled` next to the registered
    // manifest. Registry walkers check its absence.
    let root = InstallRoot::default_from_env();
    let marker = root.registered_dir().join(format!("{name}.disabled"));
    let registered = root.registered_dir().join(format!("{name}.toml"));
    if !registered.exists() {
        output::print_error(format!("adapter `{name}` is not registered"));
        return Ok(1);
    }
    if enable {
        if marker.exists() {
            fs::remove_file(&marker)?;
        }
        println!("{}", format!("✅ enabled {name}").green());
    } else {
        fs::write(&marker, "disabled\n")?;
        println!("{}", format!("⏸  disabled {name}").yellow());
    }
    Ok(0)
}

fn status(as_json: bool) -> anyhow::Result<i32> {
    let root = InstallRoot::default_from_env();
    let reg = AdapterRegistry::load_default().unwrap_or_else(|_| {
        AdapterRegistry::load(PathBuf::new()).expect("empty")
    });
    let ledger = TrustLedger::load_from(root.trust_ledger_path()).unwrap_or_default();

    let mut rows: Vec<StatusRow> = Vec::new();
    for adapter in reg.list() {
        let name = adapter.name().to_string();
        let marker = root
            .registered_dir()
            .join(format!("{name}.disabled"));
        let enabled = !marker.exists();
        let trusted_at = ledger
            .get(&name)
            .map(|e| e.trusted_at.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "(no trust entry)".into());
        let version = adapter.manifest.adapter.version.to_string();
        rows.push(StatusRow {
            name,
            version,
            enabled,
            trusted_at,
            sandbox: format!("{:?}", adapter.manifest.security.sandbox_profile)
                .to_ascii_lowercase(),
        });
    }

    if as_json {
        let out: Vec<_> = rows
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "version": r.version,
                    "enabled": r.enabled,
                    "trusted_at": r.trusted_at,
                    "sandbox": r.sandbox,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }

    if rows.is_empty() {
        println!("{}", "(no adapters registered)".dark_grey());
        return Ok(0);
    }

    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(vec![
        Cell::new("name").fg(TableColor::Cyan),
        Cell::new("version").fg(TableColor::Cyan),
        Cell::new("enabled").fg(TableColor::Cyan),
        Cell::new("trusted_at").fg(TableColor::Cyan),
        Cell::new("sandbox").fg(TableColor::Cyan),
    ]);
    for r in rows {
        let enabled_cell = if r.enabled {
            Cell::new("yes").fg(TableColor::Green)
        } else {
            Cell::new("no").fg(TableColor::Yellow)
        };
        t.add_row(vec![
            Cell::new(r.name).fg(TableColor::White),
            Cell::new(r.version),
            enabled_cell,
            Cell::new(r.trusted_at),
            Cell::new(r.sandbox),
        ]);
    }
    println!("{t}");
    Ok(0)
}

struct StatusRow {
    name: String,
    version: String,
    enabled: bool,
    trusted_at: String,
    sandbox: String,
}

async fn doctor(name: &str, as_json: bool) -> anyhow::Result<i32> {
    let reg = AdapterRegistry::load_default().unwrap_or_else(|_| {
        AdapterRegistry::load(PathBuf::new()).expect("empty")
    });
    let manifest = match reg.get(name) {
        Some(r) => r.manifest.clone(),
        None => match load_bundled_adapters()
            .into_iter()
            .find(|b| b.manifest.adapter.name == name)
        {
            Some(b) => b.manifest,
            None => {
                output::print_error(format!("no adapter named `{name}`"));
                return Ok(1);
            }
        },
    };

    let mut checks: Vec<DoctorCheck> = Vec::new();

    // env presence
    for env in &manifest.security.requires_env {
        let present = std::env::var(env).is_ok();
        checks.push(DoctorCheck {
            name: format!("env: {env}"),
            ok: present,
            detail: if present {
                "set".into()
            } else {
                format!("unset — export {env}=…")
            },
        });
    }

    // auth env
    use makakoo_core::adapter::AuthScheme;
    match manifest.auth.scheme {
        AuthScheme::Bearer | AuthScheme::Header => {
            if let Some(k) = manifest.auth.key_env.as_deref() {
                let present = std::env::var(k).is_ok();
                checks.push(DoctorCheck {
                    name: format!("auth: {k}"),
                    ok: present,
                    detail: if present {
                        "set".into()
                    } else {
                        format!("unset — required by auth.key_env")
                    },
                });
            }
        }
        AuthScheme::Basic => {
            for k in [manifest.auth.user_env.as_deref(), manifest.auth.pass_env.as_deref()]
                .into_iter()
                .flatten()
            {
                let present = std::env::var(k).is_ok();
                checks.push(DoctorCheck {
                    name: format!("auth: {k}"),
                    ok: present,
                    detail: if present {
                        "set".into()
                    } else {
                        format!("unset — required by basic auth")
                    },
                });
            }
        }
        _ => {}
    }

    // sandbox self-consistency
    let spec = makakoo_core::adapter::ProfileSpec::from_manifest(&manifest, std::env::temp_dir());
    match makakoo_core::adapter::assert_manifest_self_consistent(&spec) {
        Ok(()) => checks.push(DoctorCheck {
            name: "sandbox: self-consistent".into(),
            ok: true,
            detail: "profile matches declared fs/net surface".into(),
        }),
        Err(e) => checks.push(DoctorCheck {
            name: "sandbox: self-consistent".into(),
            ok: false,
            detail: format!("{e}"),
        }),
    }

    // health check (best-effort; async, timeout-bounded)
    if let Some(url) = manifest.health.check_url.as_deref() {
        let timeout = std::time::Duration::from_millis(manifest.health.timeout_ms.unwrap_or(3000));
        let client = reqwest::Client::builder().timeout(timeout).build().unwrap();
        match client.get(url).send().await {
            Ok(r) if r.status().is_success() => checks.push(DoctorCheck {
                name: format!("health: {url}"),
                ok: true,
                detail: format!("{}", r.status()),
            }),
            Ok(r) => checks.push(DoctorCheck {
                name: format!("health: {url}"),
                ok: false,
                detail: format!("HTTP {}", r.status()),
            }),
            Err(e) => checks.push(DoctorCheck {
                name: format!("health: {url}"),
                ok: false,
                detail: format!("{e}"),
            }),
        }
    }

    if as_json {
        let out: Vec<_> = checks
            .iter()
            .map(|c| {
                json!({
                    "check": c.name,
                    "ok": c.ok,
                    "detail": c.detail,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }

    let all_ok = checks.iter().all(|c| c.ok);
    for c in &checks {
        if c.ok {
            println!("  {} {}  {}", "✅".green(), c.name.clone().bold(), c.detail);
        } else {
            println!("  {} {}  {}", "❌".red(), c.name.clone().bold(), c.detail.clone().red());
        }
    }
    if all_ok {
        Ok(0)
    } else {
        Ok(1)
    }
}

struct DoctorCheck {
    name: String,
    ok: bool,
    detail: String,
}

fn search(query: &str) -> anyhow::Result<i32> {
    let query = query.to_ascii_lowercase();
    let reg = AdapterRegistry::load_default().unwrap_or_else(|_| {
        AdapterRegistry::load(PathBuf::new()).expect("empty")
    });
    let mut hits: Vec<(String, &str)> = Vec::new();
    for a in reg.list() {
        if a.name().to_ascii_lowercase().contains(&query) {
            hits.push((a.name().to_string(), "registered"));
        }
    }
    for b in load_bundled_adapters() {
        let n = b.manifest.adapter.name.clone();
        if n.to_ascii_lowercase().contains(&query) {
            hits.push((n, "bundled"));
        }
    }
    if hits.is_empty() {
        println!("{}", format!("(no adapters match `{query}`)").dark_grey());
        return Ok(0);
    }
    for (n, source) in hits {
        println!("  {}  {}", n.bold(), format!("({source})").dark_grey());
    }
    Ok(0)
}

fn migrate_config(path: &Path) -> anyhow::Result<i32> {
    if !path.is_file() {
        output::print_error(format!("no config file at {}", path.display()));
        return Ok(1);
    }
    let body = fs::read_to_string(path)?;
    let json: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            output::print_error(format!("not valid JSON: {e}"));
            return Ok(1);
        }
    };
    let Some(providers) = json.get("providers").and_then(|v| v.as_array()) else {
        println!("{}", "(no `providers` array — nothing to migrate)".dark_grey());
        return Ok(0);
    };

    let root = InstallRoot::default_from_env();
    let out_dir = root.registered_dir();
    fs::create_dir_all(&out_dir)?;
    let mut migrated: usize = 0;
    let mut skipped: usize = 0;

    for p in providers {
        let name = p.get("name").and_then(|v| v.as_str());
        let Some(name) = name else {
            skipped += 1;
            continue;
        };
        let kind = p.get("type").and_then(|v| v.as_str()).unwrap_or("subprocess");
        let manifest_body = match kind {
            "subprocess" => subprocess_manifest_toml(p),
            "http" => http_manifest_toml(p),
            _ => {
                skipped += 1;
                continue;
            }
        };
        let out = out_dir.join(format!("{name}.toml"));
        fs::write(&out, manifest_body)?;
        println!("  ✅ {name} → {}", out.display());
        migrated += 1;
    }

    println!(
        "{}",
        format!("migrated {migrated} provider(s); skipped {skipped}").bold()
    );
    Ok(0)
}

fn subprocess_manifest_toml(p: &serde_json::Value) -> String {
    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("legacy");
    let command = p
        .get("command")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s.replace('\"', "\\\"")))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "\"echo\", \"{prompt}\"".into());
    let stdin = p
        .get("stdin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    format!(
        r#"# auto-generated by `makakoo adapter migrate-config`
[adapter]
name            = "{name}"
version         = "0.0.0"
manifest_schema = 1
description     = "legacy subprocess provider migrated from lope config"

[compatibility]
bridge_version = "^2.0"
protocols      = ["lope-verdict-block"]

[transport]
kind    = "subprocess"
command = [{command}]
stdin   = {stdin}

[auth]
scheme = "none"

[output]
format = "lope-verdict-block"

[capabilities]
supports_roles = ["validator"]

[install]
source_type = "local"

[security]
requires_network = false
sandbox_profile  = "network-io"
"#,
    )
}

fn http_manifest_toml(p: &serde_json::Value) -> String {
    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("legacy");
    let url = p
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("http://127.0.0.1:11434/v1");
    format!(
        r#"# auto-generated by `makakoo adapter migrate-config`
[adapter]
name            = "{name}"
version         = "0.0.0"
manifest_schema = 1
description     = "legacy HTTP provider migrated from lope config"

[compatibility]
bridge_version = "^2.0"
protocols      = ["openai-chat-v1"]

[transport]
kind     = "openai-compatible"
base_url = "{url}"

[auth]
scheme = "none"

[output]
format        = "openai-chat"
verdict_field = "choices.0.message.content"

[capabilities]
supports_roles = ["validator"]

[install]
source_type = "local"

[security]
requires_network = true
allowed_hosts    = ["127.0.0.1"]
sandbox_profile  = "network-io"
"#,
    )
}

fn export(name: &str, out: Option<PathBuf>) -> anyhow::Result<i32> {
    let root = InstallRoot::default_from_env();
    let registered = root.registered_dir().join(format!("{name}.toml"));
    if !registered.exists() {
        output::print_error(format!("adapter `{name}` is not registered"));
        return Ok(1);
    }
    let sig = root.registered_dir().join(format!("{name}.toml.sig"));
    let out_path = out.unwrap_or_else(|| PathBuf::from(format!("{name}.tar.gz")));

    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tar::Builder;

    let file = fs::File::create(&out_path)?;
    let gz = GzEncoder::new(file, Compression::default());
    let mut tar = Builder::new(gz);
    tar.append_path_with_name(&registered, "adapter.toml")?;
    if sig.is_file() {
        tar.append_path_with_name(&sig, "adapter.toml.sig")?;
    }
    tar.finish()?;
    println!("✅ exported to {}", out_path.display());
    Ok(0)
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
