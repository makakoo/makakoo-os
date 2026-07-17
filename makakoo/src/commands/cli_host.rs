//! `makakoo cli` — manage runtime-registered custom CLI hosts.
//!
//! The built-in infect roster is compiled in; this command writes to the
//! runtime registry (`$MAKAKOO_HOME/config/cli_hosts.json`) so any future
//! AI CLI can be onboarded without a rebuild. `add` autodetects the
//! bootstrap + MCP files under `~/.<name>/`, with explicit-flag and
//! `--from <known-host>` overrides for CLIs whose layout it can't sniff.

use std::path::Path;

use anyhow::{anyhow, Result};

use crate::cli::CliHostCmd;
use crate::infect::custom::{self, CustomHost, CustomMcpFormat};
use crate::infect::slots::SLOTS;

/// Bootstrap files we probe for, highest priority first. Mirrors the
/// instruction-file conventions agent CLIs actually use.
const BOOTSTRAP_CANDIDATES: &[&str] = &["AGENTS.md", "CLAUDE.md", "GEMINI.md", "AGENT.md"];

/// MCP config files we probe for, highest priority first.
const MCP_CANDIDATES: &[&str] = &["config.toml", "mcp.json", "settings.json"];

pub fn run(cmd: CliHostCmd) -> Result<i32> {
    match cmd {
        CliHostCmd::Add {
            name,
            config_dir,
            bootstrap_file,
            mcp_file,
            mcp_format,
            from,
            force,
            no_mcp,
        } => add(
            name,
            config_dir,
            bootstrap_file,
            mcp_file,
            mcp_format,
            from,
            force,
            no_mcp,
        ),
        CliHostCmd::List { json } => list(json),
        CliHostCmd::Remove { name } => remove(name),
    }
}

/// A `--from` preset: the bootstrap + MCP shape of a known host, used as
/// defaults that explicit flags still override.
struct Preset {
    bootstrap: &'static str,
    mcp: Option<(&'static str, CustomMcpFormat)>,
}

fn preset_for(host: &str) -> Result<Preset> {
    Ok(match host.to_ascii_lowercase().as_str() {
        "grok" => Preset {
            bootstrap: "AGENTS.md",
            mcp: Some(("config.toml", CustomMcpFormat::TomlSimple)),
        },
        "codex" => Preset {
            bootstrap: "AGENTS.md",
            mcp: Some(("config.toml", CustomMcpFormat::TomlCodex)),
        },
        "vibe" => Preset {
            bootstrap: "instructions.md",
            mcp: Some(("config.toml", CustomMcpFormat::TomlVibe)),
        },
        "gemini" | "qwen" => Preset {
            bootstrap: "GEMINI.md",
            mcp: Some(("settings.json", CustomMcpFormat::JsonMcpServers)),
        },
        "claude" => Preset {
            bootstrap: "CLAUDE.md",
            mcp: Some(("mcp.json", CustomMcpFormat::JsonMcpServers)),
        },
        "opencode" => Preset {
            bootstrap: "AGENTS.md",
            mcp: Some(("opencode.json", CustomMcpFormat::JsonOpencode)),
        },
        other => {
            return Err(anyhow!(
                "unknown --from host '{other}' (known: grok, codex, vibe, gemini, qwen, claude, opencode)"
            ))
        }
    })
}

fn parse_format(s: &str) -> Result<CustomMcpFormat> {
    Ok(match s.to_ascii_lowercase().replace('_', "-").as_str() {
        "json-mcp-servers" | "json-mcpservers" => CustomMcpFormat::JsonMcpServers,
        "json-opencode" => CustomMcpFormat::JsonOpencode,
        "toml-codex" => CustomMcpFormat::TomlCodex,
        "toml-vibe" => CustomMcpFormat::TomlVibe,
        "toml-simple" => CustomMcpFormat::TomlSimple,
        other => {
            return Err(anyhow!(
                "unknown --mcp-format '{other}' (json-mcp-servers | json-opencode | toml-codex | toml-vibe | toml-simple)"
            ))
        }
    })
}

fn format_label(f: CustomMcpFormat) -> &'static str {
    match f {
        CustomMcpFormat::JsonMcpServers => "json-mcp-servers",
        CustomMcpFormat::JsonOpencode => "json-opencode",
        CustomMcpFormat::TomlCodex => "toml-codex",
        CustomMcpFormat::TomlVibe => "toml-vibe",
        CustomMcpFormat::TomlSimple => "toml-simple",
    }
}

/// First existing candidate filename inside `dir`, if any.
fn first_present(dir: &Path, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|f| dir.join(f).is_file())
        .map(|f| f.to_string())
}

/// Sniff an MCP config file's format from its contents.
fn detect_format(path: &Path) -> Option<CustomMcpFormat> {
    let body = std::fs::read_to_string(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".toml") {
        if body.contains("[[mcp_servers]]") {
            Some(CustomMcpFormat::TomlVibe)
        } else if body.contains("env_vars") {
            Some(CustomMcpFormat::TomlCodex)
        } else {
            Some(CustomMcpFormat::TomlSimple)
        }
    } else if name.ends_with(".json") {
        // OpenCode nests servers under "mcp"; everyone else uses "mcpServers".
        if body.contains("\"mcp\"") && !body.contains("\"mcpServers\"") {
            Some(CustomMcpFormat::JsonOpencode)
        } else {
            Some(CustomMcpFormat::JsonMcpServers)
        }
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn add(
    name: String,
    config_dir: Option<String>,
    bootstrap_file: Option<String>,
    mcp_file: Option<String>,
    mcp_format: Option<String>,
    from: Option<String>,
    force: bool,
    no_mcp: bool,
) -> Result<i32> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(anyhow!("host name must not be empty"));
    }
    if SLOTS.iter().any(|s| s.name.eq_ignore_ascii_case(&name)) {
        eprintln!("'{name}' is a built-in host — already covered by `makakoo infect`. Nothing to do.");
        return Ok(1);
    }

    let preset = from.as_deref().map(preset_for).transpose()?;

    let config_dir = config_dir.unwrap_or_else(|| format!(".{name}"));
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no $HOME"))?;
    let cfg_abs = home.join(&config_dir);

    if !force && !cfg_abs.exists() {
        eprintln!(
            "~/{config_dir} not found — is '{name}' installed? Pass --config-dir to point elsewhere, or --force to register anyway."
        );
        return Ok(1);
    }

    // Bootstrap file: explicit flag → preset → autodetect → AGENTS.md.
    let bootstrap_file = bootstrap_file
        .or_else(|| preset.as_ref().map(|p| p.bootstrap.to_string()))
        .or_else(|| first_present(&cfg_abs, BOOTSTRAP_CANDIDATES))
        .unwrap_or_else(|| "AGENTS.md".to_string());
    let bootstrap_path = format!("{config_dir}/{bootstrap_file}");

    // MCP: explicit flag → preset → autodetect. `--no-mcp` skips entirely.
    let (mcp_path, mcp_fmt): (Option<String>, Option<CustomMcpFormat>) = if no_mcp {
        (None, None)
    } else {
        let file = mcp_file
            .or_else(|| preset.as_ref().and_then(|p| p.mcp.map(|(f, _)| f.to_string())))
            .or_else(|| first_present(&cfg_abs, MCP_CANDIDATES));
        match file {
            Some(f) => {
                let fmt = match mcp_format.as_deref() {
                    Some(s) => parse_format(s)?,
                    None => preset
                        .as_ref()
                        .and_then(|p| p.mcp.map(|(_, fmt)| fmt))
                        .or_else(|| detect_format(&cfg_abs.join(&f)))
                        .ok_or_else(|| {
                            anyhow!(
                                "couldn't infer MCP format for ~/{config_dir}/{f} — pass --mcp-format explicitly"
                            )
                        })?,
                };
                (Some(format!("{config_dir}/{f}")), Some(fmt))
            }
            None => (None, None),
        }
    };

    let host = CustomHost {
        name: name.clone(),
        bootstrap_path: bootstrap_path.clone(),
        mcp_path: mcp_path.clone(),
        mcp_format: mcp_fmt,
        binary: None,
    };

    let mk_home = makakoo_core::platform::makakoo_home();
    let mut hosts = custom::load(&mk_home);
    let replacing = hosts.iter().any(|h| h.name.eq_ignore_ascii_case(&name));
    hosts.retain(|h| !h.name.eq_ignore_ascii_case(&name));
    hosts.push(host);
    custom::save(&mk_home, &hosts)?;

    println!(
        "{} custom host '{name}'",
        if replacing { "updated" } else { "registered" }
    );
    println!("  bootstrap  ~/{bootstrap_path}  [markdown]");
    match (&mcp_path, mcp_fmt) {
        (Some(p), Some(f)) => println!("  mcp        ~/{p}  [{}]", format_label(f)),
        _ => println!("  mcp        (none — bootstrap only)"),
    }
    println!("\nRun `makakoo infect` to write the bootstrap block + MCP server into it.");
    Ok(0)
}

fn list(json: bool) -> Result<i32> {
    let mk_home = makakoo_core::platform::makakoo_home();
    let hosts = custom::load(&mk_home);

    if json {
        println!("{}", serde_json::to_string_pretty(&hosts)?);
        return Ok(0);
    }

    if hosts.is_empty() {
        println!("No custom CLI hosts registered.");
        println!("Add one with: makakoo cli add <name>");
        return Ok(0);
    }

    println!("Registered custom CLI hosts ({}):", hosts.len());
    for h in &hosts {
        let mcp = match (h.mcp_path.as_deref(), h.mcp_format_enum()) {
            (Some(p), Some(_)) => {
                let label = h.mcp_format.map(format_label).unwrap_or("?");
                format!("~/{p} [{label}]")
            }
            _ => "(none)".to_string(),
        };
        println!("  {:<12} bootstrap ~/{}", h.name, h.bootstrap_path);
        println!("  {:<12} mcp       {}", "", mcp);
    }
    Ok(0)
}

fn remove(name: String) -> Result<i32> {
    let mk_home = makakoo_core::platform::makakoo_home();
    let mut hosts = custom::load(&mk_home);
    let before = hosts.len();
    hosts.retain(|h| !h.name.eq_ignore_ascii_case(&name));
    if hosts.len() == before {
        eprintln!("no custom host named '{name}' is registered.");
        return Ok(1);
    }
    custom::save(&mk_home, &hosts)?;
    println!("removed custom host '{name}'.");
    println!("Note: its bootstrap block + MCP entry are left in place. Run `makakoo uninfect --target {name}` to strip them (once supported), or edit the files by hand.");
    Ok(0)
}
