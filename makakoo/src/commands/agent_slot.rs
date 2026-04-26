//! `makakoo agent {list,show,validate,inventory,create}` —
//! multi-bot subagent slot lifecycle.
//!
//! Phase 2 deliverable per SPRINT-MULTI-BOT-SUBAGENTS.  All five
//! subcommands operate on TOML files at
//! `$MAKAKOO_HOME/config/agents/<slot_id>.toml` via the
//! `makakoo_core::agents::AgentRegistry`.

use std::path::PathBuf;

use makakoo_core::agents::{slot_path, AgentRegistry, AgentSlot};
use makakoo_core::transport::config::{TelegramConfig, TransportConfig, TransportEntry};
use makakoo_core::transport::{
    config::SlackConfig,
    secrets::SecretsAdapter,
    slack::SlackAdapter,
    telegram::TelegramAdapter,
    Transport, TransportContext,
};

use crate::context::CliContext;
use crate::output;

/// `makakoo agent list` — enumerate every TOML slot in the
/// registry directory.
pub fn list(ctx: &CliContext, json: bool) -> anyhow::Result<i32> {
    let home = ctx.home();
    let registry = AgentRegistry::load(home)?;
    if json {
        let rows: Vec<_> = registry
            .slots
            .iter()
            .map(|s| {
                serde_json::json!({
                    "slot_id": s.slot_id,
                    "name": s.name,
                    "configured": s.is_configured(),
                    "transports": s.transport_summary(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    if registry.slots.is_empty() {
        println!("No agent slots configured. Run `makakoo agent create <slot>` to add one.");
        return Ok(0);
    }
    println!("{:<24}{:<24}{:<14}{}", "SLOT", "NAME", "STATUS", "TRANSPORTS");
    for slot in &registry.slots {
        let status = if slot.is_configured() {
            "OK"
        } else {
            "UNCONFIGURED"
        };
        let transports = slot
            .transport_summary()
            .into_iter()
            .map(|(id, kind)| format!("{}({})", id, kind))
            .collect::<Vec<_>>()
            .join(", ");
        let transports = if transports.is_empty() {
            "—".into()
        } else {
            transports
        };
        println!(
            "{:<24}{:<24}{:<14}{}",
            slot.slot_id, slot.name, status, transports
        );
    }
    Ok(0)
}

/// `makakoo agent show <slot>` — print the resolved TOML with
/// every secret-bearing field redacted.
pub fn show(ctx: &CliContext, slot_id: &str, json: bool) -> anyhow::Result<i32> {
    let path = slot_path(ctx.home(), slot_id);
    if !path.exists() {
        output::print_error(format!(
            "agent slot '{}' not found at {}",
            slot_id,
            path.display()
        ));
        return Ok(1);
    }
    let slot = AgentSlot::load_from_file(&path)?;
    let redacted = slot.redacted();
    if json {
        println!("{}", serde_json::to_string_pretty(&redacted)?);
    } else {
        let toml_text = toml::to_string_pretty(&redacted)
            .map_err(|e| anyhow::anyhow!("agent show: serialise: {}", e))?;
        println!("{}", toml_text);
        // Phase 4: render the effective LLM config with per-field
        // source attribution (override vs system default).
        let defaults = makakoo_core::agents::llm_override::LlmDefaults::builtin_fallback();
        let over = slot.llm.as_ref().and_then(|s| s.effective_override());
        let eff = makakoo_core::agents::llm_override::resolve_effective(
            over.as_ref(),
            &defaults,
        );
        print!("{}", eff.render_human());
    }
    Ok(0)
}

/// `makakoo agent validate <slot>` — run per-transport credential
/// verifiers WITHOUT starting the agent. Reports first failure.
pub fn validate(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let path = slot_path(ctx.home(), slot_id);
    if !path.exists() {
        output::print_error(format!(
            "agent slot '{}' not found at {}",
            slot_id,
            path.display()
        ));
        return Ok(1);
    }
    let slot = AgentSlot::load_from_file(&path)?;
    // We're already inside `#[tokio::main]`'s runtime — use the
    // current handle plus block_in_place rather than spawning a
    // nested runtime (which panics).
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async move {
        let mut failures = Vec::new();
        for entry in &slot.transports {
            if !entry.enabled {
                continue;
            }
            match verify_one(slot_id, entry).await {
                Ok((account_id, tenant_id)) => {
                    let tenant = tenant_id
                        .map(|t| format!(", tenant={}", t))
                        .unwrap_or_default();
                    println!(
                        "  ✓ {} ({}): account={}{}",
                        entry.id, entry.kind, account_id, tenant
                    );
                }
                Err(e) => {
                    failures.push(format!("  ✗ {} ({}): {}", entry.id, entry.kind, e));
                }
            }
        }
        if failures.is_empty() {
            println!("agent slot '{}' validate OK", slot_id);
            Ok(0)
        } else {
            for f in failures {
                eprintln!("{f}");
            }
            output::print_error(format!(
                "agent slot '{}' has failing transports — fix before `agent start`",
                slot_id
            ));
            Ok(2)
        }
    }))
}

async fn verify_one(
    slot_id: &str,
    entry: &TransportEntry,
) -> anyhow::Result<(String, Option<String>)> {
    let secrets = makakoo_core::transport::secrets::KeyringSecrets;
    let ctx_inner = TransportContext {
        slot_id: slot_id.to_string(),
        transport_id: entry.id.clone(),
    };
    match &entry.config {
        TransportConfig::Telegram(cfg) => {
            let bot_token = secrets
                .resolve(&entry.bot_token_ref())
                .map_err(|e| anyhow::anyhow!("resolve bot token: {}", e))?;
            let adapter =
                TelegramAdapter::new(ctx_inner, cfg.clone(), bot_token.value, entry.allowed_users.clone());
            let id = adapter
                .verify_credentials()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            Ok((id.account_id, id.tenant_id))
        }
        TransportConfig::Slack(cfg) => {
            let bot_token = secrets
                .resolve(&entry.bot_token_ref())
                .map_err(|e| anyhow::anyhow!("resolve slack bot token: {}", e))?;
            let app_token = secrets
                .resolve(&entry.app_token_ref())
                .map_err(|e| anyhow::anyhow!("resolve slack app token: {}", e))?;
            let adapter = SlackAdapter::new(
                ctx_inner,
                cfg.clone(),
                bot_token.value,
                app_token.value,
                entry.allowed_users.clone(),
            );
            let id = adapter
                .verify_credentials()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            Ok((id.account_id, id.tenant_id))
        }
    }
}

/// `makakoo agent inventory` — Q8 reduced-scope helper: enumerate
/// existing `agent-*` plugins with migration status (active /
/// migrated / pending) WITHOUT migrating them.
pub fn inventory(ctx: &CliContext, json: bool) -> anyhow::Result<i32> {
    use makakoo_core::plugin::PluginRegistry;

    let plugins =
        PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let registry = AgentRegistry::load(ctx.home())?;
    let migrated_slot_ids: std::collections::HashSet<String> =
        registry.slots.iter().map(|s| s.slot_id.clone()).collect();

    use makakoo_core::plugin::manifest::PluginKind;
    // `active` = the legacy plugin still has a live process. We detect
    // it by pgrep on the plugin name. A plugin can be both `active` AND
    // `migrated` (the operator hasn't shut down the legacy process yet),
    // so the status string captures both: `active+migrated`, `active`,
    // `migrated`, or `pending`.
    let agent_plugins: Vec<_> = plugins
        .plugins()
        .iter()
        .filter(|p| p.manifest.plugin.kind == PluginKind::Agent)
        .map(|p| {
            let plugin_name = p.manifest.plugin.name.clone();
            let slot_guess = plugin_name
                .strip_prefix("agent-")
                .map(|s| s.to_string())
                .unwrap_or_else(|| plugin_name.clone());
            let migrated = migrated_slot_ids.contains(&slot_guess);
            let active = is_plugin_process_active(&plugin_name);
            let status = match (active, migrated) {
                (true, true) => "active+migrated",
                (true, false) => "active",
                (false, true) => "migrated",
                (false, false) => "pending",
            };
            (plugin_name, slot_guess, status.to_string())
        })
        .collect();

    if json {
        let rows: Vec<_> = agent_plugins
            .iter()
            .map(|(plugin, slot, status)| {
                serde_json::json!({
                    "plugin": plugin,
                    "slot_id_guess": slot,
                    "status": status,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    if agent_plugins.is_empty() {
        println!("No legacy agent-* plugins installed.");
        return Ok(0);
    }
    println!("{:<32}{:<24}{}", "PLUGIN", "SLOT_ID_GUESS", "STATUS");
    for (plugin, slot, status) in &agent_plugins {
        println!("{:<32}{:<24}{}", plugin, slot, status);
    }
    println!();
    println!("'pending' plugins have NOT been migrated (Q8 — only harveychat ships in v1).");
    Ok(0)
}

/// Args for `makakoo agent create`.
pub struct CreateArgs {
    pub slot: String,
    pub name: Option<String>,
    pub persona: Option<String>,
    pub allowed_paths: Vec<String>,
    pub forbidden_paths: Vec<String>,
    pub tools: Vec<String>,
    pub from_toml: Option<PathBuf>,
    pub telegram_token: Option<String>,
    pub telegram_allowed: Vec<String>,
    pub slack_bot_token: Option<String>,
    pub slack_app_token: Option<String>,
    pub slack_team: Option<String>,
    pub slack_allowed: Vec<String>,
    pub skip_credential_check: bool,
}

/// `makakoo agent create <slot> ...` — write a new TOML to the
/// registry. Pre-validates credentials via the per-transport
/// verifier (unless --skip-credential-check) BEFORE writing files.
pub fn create(ctx: &CliContext, args: CreateArgs) -> anyhow::Result<i32> {
    makakoo_core::agents::validate_slot_id(&args.slot)?;
    let target = slot_path(ctx.home(), &args.slot);
    if target.exists() {
        output::print_error(format!(
            "agent slot '{}' already exists at {} — refusing to overwrite",
            args.slot,
            target.display()
        ));
        return Ok(1);
    }

    let slot = if let Some(path) = args.from_toml.as_ref() {
        if args.telegram_token.is_some() || args.slack_bot_token.is_some() {
            anyhow::bail!(
                "--from-toml is mutually exclusive with --telegram-token / --slack-bot-token"
            );
        }
        let raw = std::fs::read_to_string(path)?;
        let mut s: AgentSlot = toml::from_str(&raw)?;
        // Reject slot_id mismatch — caller intent is unambiguous
        // when a slot_id is in the source file.  Empty slot_id in
        // the source is treated as "use the CLI argument".
        if !s.slot_id.is_empty() && s.slot_id != args.slot {
            anyhow::bail!(
                "--from-toml file has slot_id '{}' but CLI requested slot '{}' — they must match",
                s.slot_id,
                args.slot
            );
        }
        s.slot_id = args.slot.clone();
        if let Some(n) = args.name.clone() {
            s.name = n;
        }
        if let Some(p) = args.persona.clone() {
            s.persona = Some(p);
        }
        // Override scope flags only if explicitly passed (non-empty
        // CLI list takes precedence over the file).
        if !args.allowed_paths.is_empty() {
            s.allowed_paths = args.allowed_paths.clone();
        }
        if !args.forbidden_paths.is_empty() {
            s.forbidden_paths = args.forbidden_paths.clone();
        }
        if !args.tools.is_empty() {
            s.tools = args.tools.clone();
        }
        s.validate()?;
        s
    } else {
        build_slot_from_flags(&args)?
    };

    if !args.skip_credential_check {
        let result =
            tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async {
                for entry in slot.transports.iter().filter(|t| t.enabled) {
                    verify_one(&slot.slot_id, entry).await?;
                }
                Ok::<(), anyhow::Error>(())
            }));
        if let Err(e) = result {
            output::print_error(format!(
                "agent create '{}': credential check failed: {} (run with --skip-credential-check to scaffold without verifying)",
                args.slot, e
            ));
            return Ok(2);
        }
    }

    AgentRegistry::create(ctx.home(), &slot)?;
    output::print_info(format!(
        "agent slot '{}' created at {}",
        slot.slot_id,
        target.display()
    ));
    println!("Next: `makakoo agent validate {}` then `makakoo agent start {}`.", slot.slot_id, slot.slot_id);
    Ok(0)
}

fn build_slot_from_flags(args: &CreateArgs) -> anyhow::Result<AgentSlot> {
    let has_telegram = args.telegram_token.is_some();
    let has_slack = args.slack_bot_token.is_some()
        || args.slack_app_token.is_some()
        || args.slack_team.is_some();
    if !has_telegram && !has_slack {
        anyhow::bail!(
            "agent create needs at least one transport: pass --telegram-token <T> OR --slack-bot-token + --slack-app-token + --slack-team OR --from-toml <path>"
        );
    }
    let mut transports: Vec<TransportEntry> = Vec::new();
    if has_telegram {
        let token = args.telegram_token.as_ref().unwrap();
        transports.push(TransportEntry {
            id: "telegram-main".into(),
            kind: "telegram".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: Some(token.clone()),
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: args.telegram_allowed.clone(),
            config: TransportConfig::Telegram(TelegramConfig {
                polling_timeout_seconds: 30,
                allowed_chat_ids: args.telegram_allowed.clone(),
                allowed_group_ids: vec![],
                support_thread: false,
            }),
        });
    }
    if has_slack {
        let bot = args
            .slack_bot_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Slack transport requires --slack-bot-token"))?;
        let app = args
            .slack_app_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Slack transport requires --slack-app-token"))?;
        let team = args
            .slack_team
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Slack transport requires --slack-team"))?;
        transports.push(TransportEntry {
            id: "slack-main".into(),
            kind: "slack".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: Some(bot.clone()),
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: Some(app.clone()),
            allowed_users: args.slack_allowed.clone(),
            config: TransportConfig::Slack(SlackConfig {
                team_id: team.clone(),
                mode: "socket".into(),
                dm_only: true,
                channels: vec![],
                support_thread: false,
            }),
        });
    }
    let slot = AgentSlot {
        slot_id: args.slot.clone(),
        name: args.name.clone().unwrap_or_else(|| args.slot.clone()),
        persona: args.persona.clone(),
        inherit_baseline: true,
        allowed_paths: args.allowed_paths.clone(),
        forbidden_paths: args.forbidden_paths.clone(),
        tools: args.tools.clone(),
        process_mode: "supervised_pair".into(),
        transports,
    };
    slot.validate()?;
    Ok(slot)
}

/// Best-effort check for a live plugin process via pgrep on its
/// canonical plugin name.  Returns `false` on any pgrep error
/// (missing binary, unsupported platform) — the inventory output
/// is informational, never gates other commands.
fn is_plugin_process_active(plugin_name: &str) -> bool {
    use std::process::Command;
    Command::new("pgrep")
        .arg("-f")
        .arg(plugin_name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `makakoo agent migrate-harveychat` — runs the
/// HarveyChat→harveychat-slot migration once.  Idempotent.  All
/// side effects (DB archive, config archive, fresh DB seeding,
/// backfill on re-run) live in
/// `makakoo_core::agents::migrate::harveychat::migrate` so library
/// callers and the CLI behave identically.
pub fn migrate_harveychat(ctx: &CliContext) -> anyhow::Result<i32> {
    use makakoo_core::agents::migrate::harveychat::{migrate, MigrationOutcome};

    match migrate(ctx.home())? {
        MigrationOutcome::Migrated {
            toml_path,
            archived_db,
            archived_config,
            new_db,
        } => {
            output::print_info(format!(
                "harveychat migrated: {} ← data/chat/config.json",
                toml_path.display()
            ));
            if let Some(db) = archived_db {
                println!("  legacy conversations.db archived at {}", db.display());
            }
            if let Some(cfg) = archived_config {
                println!("  legacy config.json archived at {}", cfg.display());
            }
            if let Some(db) = new_db {
                println!("  fresh conversations.db seeded at {}", db.display());
            }
            Ok(0)
        }
        MigrationOutcome::AlreadyMigrated {
            backfilled_artifacts,
        } => {
            if backfilled_artifacts.is_empty() {
                output::print_info(
                    "harveychat already migrated — nothing to do (re-run safe)".to_string(),
                );
            } else {
                output::print_info(format!(
                    "harveychat already migrated — backfilled {} missing artifact(s)",
                    backfilled_artifacts.len()
                ));
                for path in &backfilled_artifacts {
                    println!("  + {}", path.display());
                }
            }
            Ok(0)
        }
        MigrationOutcome::NothingToMigrate => {
            output::print_warn(
                "no legacy data/chat/config.json found — nothing to migrate".to_string(),
            );
            Ok(0)
        }
    }
}

