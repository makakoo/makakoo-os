//! `makakoo agent {list,show,validate,inventory,create}` —
//! multi-bot subagent slot lifecycle.
//!
//! Phase 2 deliverable per SPRINT-MULTI-BOT-SUBAGENTS.  All five
//! subcommands operate on TOML files at
//! `$MAKAKOO_HOME/config/agents/<slot_id>.toml` via the
//! `makakoo_core::agents::AgentRegistry`.

use std::path::PathBuf;

use makakoo_core::agents::spec::AgentSpec;
use makakoo_core::agents::{slot_path, AgentRegistry, AgentSlot};
use makakoo_core::transport::config::TransportConfig;
use makakoo_core::transport::config::TransportEntry;
use makakoo_core::transport::secrets::SecretsAdapter;

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
                let runtime_ready =
                    s.runtime.is_some() && crate::commands::agent_runtime::preflight(s).is_ok();
                serde_json::json!({
                    "slot_id": s.slot_id,
                    "name": s.name,
                    "configured": s.is_configured(),
                    "ready": runtime_ready || s.is_configured(),
                    "runtime_ready": runtime_ready,
                    "transport_configured": s.is_configured(),
                    "engine": s.runtime.as_ref().map(|runtime| runtime.engine.to_string()),
                    "project_dir": s.runtime.as_ref().map(|runtime| &runtime.project_dir),
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
    println!(
        "{:<24}{:<24}{:<14}{:<20}TRANSPORTS",
        "SLOT", "NAME", "STATUS", "ENGINE"
    );
    for slot in &registry.slots {
        let status =
            if slot.runtime.as_ref().is_some_and(|runtime| {
                runtime.engine == makakoo_core::agents::AgentRuntimeEngine::Flue
            }) {
                "MANUAL"
            } else if slot.runtime.is_some() {
                if crate::commands::agent_runtime::preflight(slot).is_ok() {
                    "READY"
                } else {
                    "GENERATED"
                }
            } else if slot.is_configured() {
                "OK"
            } else {
                "UNCONFIGURED"
            };
        let engine = slot
            .runtime
            .as_ref()
            .map(|runtime| runtime.engine.to_string())
            .unwrap_or_else(|| "legacy".into());
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
            "{:<24}{:<24}{:<14}{:<20}{}",
            slot.slot_id, slot.name, status, engine, transports
        );
    }
    Ok(0)
}

/// `makakoo agent show <slot>` — print the resolved TOML with
/// every secret-bearing field redacted.
pub fn show(ctx: &CliContext, slot_id: &str, json: bool) -> anyhow::Result<i32> {
    let path = makakoo_core::agents::checked_slot_path(ctx.home(), slot_id)?;
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
        let eff = makakoo_core::agents::llm_override::resolve_effective(over.as_ref(), &defaults);
        print!("{}", eff.render_human());
    }
    Ok(0)
}

/// `makakoo agent validate <slot>` — config-only validation of each
/// transport: confirms the entry parses and its secret references
/// resolve from the local secret store, WITHOUT a network call or
/// starting the agent. Reports first failure.
pub fn validate(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let path = makakoo_core::agents::checked_slot_path(ctx.home(), slot_id)?;
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
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
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
            if slot.runtime.as_ref().is_some_and(|runtime| {
                runtime.engine == makakoo_core::agents::AgentRuntimeEngine::Flue
            }) {
                println!("  - runtime: Flue is manual-only; preflight skipped");
            } else if let Err(error) = crate::commands::agent_runtime::preflight(&slot) {
                failures.push(format!("  ✗ runtime: {}", error));
            } else if slot.runtime.is_some() {
                println!("  ✓ runtime: generated project and dependencies present");
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
        })
    })
}

/// Config-only validation of a single transport entry.
///
/// Confirms the entry's secret references resolve from the local
/// secret store (catches a missing or blank token before
/// `agent start`). Makes NO network call: the live `getMe` /
/// `auth.test` probe was removed together with the unused Rust
/// transport runtime — the live gateway (the Python harveychat
/// path) is what actually authenticates against the provider at
/// start. Returns a placeholder identity so the caller's display
/// stays uniform across transport kinds.
async fn verify_one(
    _slot_id: &str,
    entry: &TransportEntry,
) -> anyhow::Result<(String, Option<String>)> {
    let secrets = makakoo_core::transport::secrets::KeyringSecrets;
    match &entry.config {
        TransportConfig::Telegram(_) => {
            secrets
                .resolve(&entry.bot_token_ref())
                .map_err(|e| anyhow::anyhow!("resolve bot token: {}", e))?;
            Ok((format!("config-ok-{}", entry.id), None))
        }
        TransportConfig::Slack(_) => {
            secrets
                .resolve(&entry.bot_token_ref())
                .map_err(|e| anyhow::anyhow!("resolve slack bot token: {}", e))?;
            secrets
                .resolve(&entry.app_token_ref())
                .map_err(|e| anyhow::anyhow!("resolve slack app token: {}", e))?;
            Ok((format!("config-ok-{}", entry.id), None))
        }
        TransportConfig::Discord(_)
        | TransportConfig::WhatsApp(_)
        | TransportConfig::Web(_)
        | TransportConfig::VoiceTwilio(_)
        | TransportConfig::Email(_) => {
            // These kinds carry no secret-ref check in the simplified
            // registry path. Return the placeholder so duplicate
            // detection skips them.
            Ok((format!("v2-pending-{}", entry.id), None))
        }
    }
}

/// `makakoo agent inventory` — Q8 reduced-scope helper: enumerate
/// existing `agent-*` plugins with migration status (active /
/// migrated / pending) WITHOUT migrating them.
pub fn inventory(ctx: &CliContext, json: bool) -> anyhow::Result<i32> {
    use makakoo_core::plugin::PluginRegistry;

    let plugins = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
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
    println!("{:<32}{:<24}STATUS", "PLUGIN", "SLOT_ID_GUESS");
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
    pub telegram_token: Option<String>,
    pub telegram_allowed: Vec<String>,
    pub slack_bot_token: Option<String>,
    pub slack_app_token: Option<String>,
    pub slack_team: Option<String>,
    pub slack_allowed: Vec<String>,
    pub skip_credential_check: bool,
    pub out: Option<PathBuf>,
    pub specs: Option<PathBuf>,
}

/// `makakoo agent create <slot> ...` — write a new TOML to the
/// registry. Pre-validates each transport's config + secret
/// references (unless --skip-credential-check) BEFORE writing files.
pub fn create(ctx: &CliContext, args: CreateArgs) -> anyhow::Result<i32> {
    // Branch: --specs drives spec-based creation. The other supported
    // path is the inline Telegram/Slack quick-start shorthand.
    if let Some(specs_path) = args.specs.as_ref() {
        return create_from_specs(ctx, specs_path, &args);
    }

    // Quick-start path needs a slot id. --specs path uses the
    // spec's name; the early return above skips this check.
    if args.slot.is_empty() {
        anyhow::bail!("agent create requires a <SLOT> argument when --specs is not used");
    }
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

    // Phase 4: the spec is the source of truth. The quick-start path
    // (Telegram/Slack flags) still works — it builds a synthetic spec
    // from the flags, derives a slot from it, and pre-fills `.env`
    // with the inline tokens.
    let spec = build_spec_from_flags(&args)?;
    let mut slot = spec.to_slot().map_err(|e| anyhow::anyhow!("{}", e))?;

    // The credential check still runs for the quick-start path
    // (inline tokens) unless the operator passes --skip-credential-check.
    // The --specs path returns early above and never reaches this check.
    if !args.skip_credential_check {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                for entry in slot.transports.iter().filter(|t| t.enabled) {
                    verify_one(&slot.slot_id, entry).await?;
                }
                Ok::<(), anyhow::Error>(())
            })
        });
        if let Err(e) = result {
            output::print_error(format!(
                "agent create '{}': credential check failed: {} (run with --skip-credential-check to scaffold without verifying)",
                args.slot, e
            ));
            return Ok(2);
        }
    }

    let engine = crate::commands::agent_engine::selected_engine()?;
    let out_dir = args.out.clone().unwrap_or_else(|| {
        crate::commands::agent_engine::default_project_dir(ctx.home(), &slot.slot_id, engine)
    });
    if !out_dir.is_absolute() {
        anyhow::bail!(
            "agent runtime output must be an absolute path: {}",
            out_dir.display()
        );
    }
    crate::commands::agent_engine::validate_model(engine, &spec.model)?;
    slot.runtime = Some(makakoo_core::agents::AgentRuntime {
        engine,
        project_dir: out_dir.clone(),
    });
    let inline_secrets = collect_inline_secrets(&args);
    let runtime =
        crate::commands::agent_engine::scaffold(engine, &spec, &out_dir, &inline_secrets, None)?;
    crate::commands::agent_engine::persist_slot(ctx.home(), &slot, &runtime)?;
    output::print_info(format!(
        "agent slot '{}' created at {}",
        slot.slot_id,
        target.display()
    ));
    output::print_info(format!(
        "{} agent project scaffolded at {}",
        runtime.engine,
        out_dir.display()
    ));
    if engine == makakoo_core::agents::AgentRuntimeEngine::DeepseekHarness
        && !spec.channels.is_empty()
    {
        output::print_warn(
            "DSH V1 exposes the authenticated runtime API; declared channel ingress still requires the Makakoo/Flue channel-adapter slice.",
        );
    }
    crate::commands::agent_engine::print_next(&slot.slot_id, &runtime);
    Ok(0)
}

/// `makakoo agent create --specs <PATH>` — create one or more agents
/// from declarative spec files. PATH may be a single `.yaml`/
/// `.yml`/`.toml` file, a directory of specs (non-recursive, sorted),
/// or `.` to scan the current folder.
///
/// DeepSeek Harness is the default runtime engine. Operators can use
/// `MAKAKOO_AGENT_ENGINE=flue` as a compatibility escape hatch.
fn create_from_specs(
    ctx: &CliContext,
    specs_path: &std::path::Path,
    args: &CreateArgs,
) -> anyhow::Result<i32> {
    use makakoo_core::agents::spec::convert::triggers_warning;
    use makakoo_core::agents::spec::discovery::discover_specs;
    use makakoo_core::agents::spec::AgentSpec;

    // Refuse combinations that don't make sense with --specs.
    if args.telegram_token.is_some() || args.slack_bot_token.is_some() {
        anyhow::bail!(
            "--specs is mutually exclusive with --telegram-token / --slack-* (use a spec file for declarative channels)"
        );
    }

    // Step 1: discover all specs. Pre-flight validates each one
    // and surfaces parse/validation errors with file paths.
    let specs = discover_specs(specs_path).map_err(|e| anyhow::anyhow!("{}", e))?;

    // Step 2: pre-flight duplicate-name check across the batch
    // (atomicity — if two specs produce the same slot_id, refuse
    // the whole batch rather than writing half of it).
    let mut seen: std::collections::HashMap<String, &AgentSpec> = std::collections::HashMap::new();
    for spec in &specs {
        if let Some(prev) = seen.get(&spec.name) {
            anyhow::bail!(
                "duplicate spec name '{}' in batch: first seen at {}, again at {}",
                spec.name,
                prev.name,
                spec.name
            );
        }
        seen.insert(spec.name.clone(), spec);
    }

    // Step 3: pre-flight existing-slot check across the batch.
    for spec in &specs {
        let target = slot_path(ctx.home(), &spec.name);
        if target.exists() {
            output::print_error(format!(
                "agent slot '{}' already exists at {} — refusing to overwrite",
                spec.name,
                target.display()
            ));
            return Ok(1);
        }
    }

    let engine = crate::commands::agent_engine::selected_engine()?;

    // Step 4: DSH always routes through switchAILocal. Provider discovery
    // only exists for the operator-selected legacy Flue scaffolder.
    let providers = if engine == makakoo_core::agents::AgentRuntimeEngine::Flue {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(makakoo_core::agents::llm_provider::discover_providers())
        })
    } else {
        Vec::new()
    };

    // Prepare every slot before mutating either registry or project tree.
    let mut prepared = Vec::new();
    for original in &specs {
        let (spec, provider) = if engine == makakoo_core::agents::AgentRuntimeEngine::Flue {
            crate::commands::agent_provider::choose(original, &providers)
        } else {
            (original.clone(), None)
        };
        crate::commands::agent_engine::validate_model(engine, &spec.model)?;
        let mut slot = spec.to_slot().map_err(|e| anyhow::anyhow!("{}", e))?;
        let out_dir =
            crate::commands::agent_engine::default_project_dir(ctx.home(), &slot.slot_id, engine);
        if out_dir.exists() {
            anyhow::bail!(
                "agent runtime output {} already exists — refusing to overwrite",
                out_dir.display()
            );
        }
        slot.runtime = Some(makakoo_core::agents::AgentRuntime {
            engine,
            project_dir: out_dir.clone(),
        });
        prepared.push((spec, slot, out_dir, provider));
    }

    // Scaffold all projects first. A render failure removes every project
    // produced by this batch, leaving the canonical registry unchanged.
    let mut runtimes = Vec::new();
    for (spec, _, out_dir, provider) in &prepared {
        match crate::commands::agent_engine::scaffold(
            engine,
            spec,
            out_dir,
            &std::collections::HashMap::new(),
            provider.as_ref(),
        ) {
            Ok(runtime) => runtimes.push(runtime),
            Err(error) => {
                for runtime in &runtimes {
                    crate::commands::agent_engine::remove_generated(runtime);
                }
                return Err(error);
            }
        }
    }

    // Persist registry only after every project renders. Roll back both
    // surfaces if an unexpected registry write fails midway.
    let mut written_slots: Vec<std::path::PathBuf> = Vec::new();
    for (_, slot, _, _) in &prepared {
        if let Err(error) = AgentRegistry::create(ctx.home(), slot) {
            for path in &written_slots {
                let _ = std::fs::remove_file(path);
            }
            for runtime in &runtimes {
                crate::commands::agent_engine::remove_generated(runtime);
            }
            return Err(anyhow::anyhow!(error));
        }
        written_slots.push(slot_path(ctx.home(), &slot.slot_id));
    }

    for ((spec, slot, out_dir, _), runtime) in prepared.iter().zip(&runtimes) {
        output::print_info(format!(
            "agent slot '{}' created at {}",
            slot.slot_id,
            slot_path(ctx.home(), &slot.slot_id).display()
        ));
        output::print_info(format!(
            "{} agent project scaffolded at {}",
            runtime.engine,
            out_dir.display()
        ));
        if engine == makakoo_core::agents::AgentRuntimeEngine::DeepseekHarness
            && !spec.channels.is_empty()
        {
            output::print_warn(
                "DSH V1 exposes the authenticated runtime API; declared channel ingress still requires the Makakoo/Flue channel-adapter slice.",
            );
        }
        if engine == makakoo_core::agents::AgentRuntimeEngine::DeepseekHarness {
            if let Some(warning) = triggers_warning(spec) {
                output::print_warn(warning);
            }
        }
    }

    let count = prepared.len();
    if count == 1 {
        let s = &specs[0];
        let out_dir =
            crate::commands::agent_engine::default_project_dir(ctx.home(), &s.name, engine);
        crate::commands::agent_engine::print_next(
            &s.name,
            &makakoo_core::agents::AgentRuntime {
                engine,
                project_dir: out_dir,
            },
        );
    } else {
        println!(
            "Created {} agent(s) from spec(s). See `makakoo agent list` for the registry view.",
            count
        );
    }
    Ok(0)
}

/// `makakoo agent validate-spec <PATH>` — parse and validate one or
/// more spec files without creating anything. Exits 0 if all
/// validate, 1 if any fail.
pub fn validate_spec(ctx: &CliContext, path: &std::path::Path) -> anyhow::Result<i32> {
    use makakoo_core::agents::spec::convert::triggers_warning;
    use makakoo_core::agents::spec::discovery::discover_specs;

    let _ = ctx; // reserved for future $MAKAKOO_HOME-aware checks
    let engine = crate::commands::agent_engine::selected_engine()?;

    let specs = discover_specs(path).map_err(|e| anyhow::anyhow!("{}", e))?;
    if specs.is_empty() {
        output::print_warn(format!("no specs found at {}", path.display()));
        return Ok(0);
    }

    let mut ok = 0;
    let mut failed = 0;
    for spec in &specs {
        match validate_spec_for_engine(spec, engine) {
            Ok(()) => {
                println!("[OK]   {}", spec.name);
                if engine == makakoo_core::agents::AgentRuntimeEngine::DeepseekHarness {
                    if !spec.channels.is_empty() {
                        println!(
                            "       note: DSH V1 does not execute declared channel ingress; add the Makakoo/Flue channel-adapter slice"
                        );
                    }
                    if let Some(w) = triggers_warning(spec) {
                        println!("       note: {}", w);
                    }
                }
                ok += 1;
            }
            Err(e) => {
                println!("[FAIL] {}: {}", spec.name, e);
                failed += 1;
            }
        }
    }

    println!();
    println!("{} ok, {} failed", ok, failed);
    if failed > 0 {
        Ok(1)
    } else {
        Ok(0)
    }
}

fn validate_spec_for_engine(
    spec: &AgentSpec,
    engine: makakoo_core::agents::AgentRuntimeEngine,
) -> anyhow::Result<()> {
    spec.validate().map_err(|error| anyhow::anyhow!(error))?;
    crate::commands::agent_engine::validate_model(engine, &spec.model)
}

/// Build a `AgentSpec` from the quick-start CLI flags. The spec is
/// the source of truth; the slot is derived from it via
/// `spec.to_slot()`. The scaffold receives the spec directly.
fn build_spec_from_flags(args: &CreateArgs) -> anyhow::Result<AgentSpec> {
    use makakoo_core::agents::spec::{ChannelSpec, ScopeSpec};

    let has_telegram = args.telegram_token.is_some();
    let has_slack = args.slack_bot_token.is_some()
        || args.slack_app_token.is_some()
        || args.slack_team.is_some();
    if !has_telegram && !has_slack {
        anyhow::bail!(
            "agent create needs at least one transport: pass --telegram-token <T> OR --slack-bot-token + --slack-app-token + --slack-team OR --specs <path-to-spec>"
        );
    }

    let mut channels: Vec<ChannelSpec> = Vec::new();
    if has_telegram {
        if args.telegram_token.is_none() {
            anyhow::bail!("telegram transport requires --telegram-token");
        }
        channels.push(ChannelSpec::Telegram {
            token_env: "TELEGRAM_BOT_TOKEN".into(),
            allowed_users: args.telegram_allowed.clone(),
        });
    }
    if has_slack {
        if args.slack_bot_token.is_none()
            || args.slack_app_token.is_none()
            || args.slack_team.is_none()
        {
            anyhow::bail!(
                "slack transport requires --slack-bot-token + --slack-app-token + --slack-team"
            );
        }
        channels.push(ChannelSpec::Slack {
            token_env: "SLACK_BOT_TOKEN".into(),
            app_token_env: "SLACK_APP_TOKEN".into(),
            team_id_env: "SLACK_TEAM_ID".into(),
            allowed_users: args.slack_allowed.clone(),
        });
    }

    let description = args
        .name
        .clone()
        .unwrap_or_else(|| format!("Quick-start agent: {}", args.slot));

    let spec = AgentSpec {
        name: args.slot.clone(),
        description,
        model: makakoo_core::agents::llm_provider_default::get_default()
            .unwrap_or_else(|| "switchailocal/ail-compound".into()),
        instructions: args
            .persona
            .clone()
            .unwrap_or_else(|| "You are a Makakoo agent. Use the available `mcp__harvey__*` tools to act on the user's behalf.".into()),
        tools: args.tools.clone(),
        channels,
        triggers: vec![],
        scope: ScopeSpec {
            allowed_paths: args.allowed_paths.clone(),
            forbidden_paths: args.forbidden_paths.clone(),
        },
    };
    spec.validate()?;
    Ok(spec)
}

/// Collect inline secrets from the quick-start flags. Used to
/// pre-fill the generated `.env`. DSH reserves channel credentials
/// for the Makakoo adapter; legacy Flue consumes them directly.
fn collect_inline_secrets(args: &CreateArgs) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Some(t) = &args.telegram_token {
        out.insert("TELEGRAM_BOT_TOKEN".into(), t.clone());
        // Telegram also needs a webhook secret. We don't have one
        // inline; emit a clearly-dev-only placeholder. The operator
        // must replace it before production use.
        out.entry("TELEGRAM_WEBHOOK_SECRET_TOKEN".into())
            .or_insert_with(|| "dev_only_replace_me".into());
    }
    if let Some(t) = &args.slack_bot_token {
        out.insert("SLACK_BOT_TOKEN".into(), t.clone());
    }
    if let Some(t) = &args.slack_app_token {
        out.insert("SLACK_APP_TOKEN".into(), t.clone());
    }
    if let Some(t) = &args.slack_team {
        out.insert("SLACK_TEAM_ID".into(), t.clone());
    }
    out
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
                output::print_info("harveychat already migrated — nothing to do (re-run safe)");
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
            output::print_warn("no legacy data/chat/config.json found — nothing to migrate");
            Ok(0)
        }
    }
}

/// `makakoo agent init-spec <PATH>` — interactive starter that asks
/// the right questions and writes a correct spec. With `--minimal`,
/// emits a 10-line "hello world" spec.
///
/// Project default (`makakoo agent provider-set <p> <m>`) is used as the
/// initial model choice. The user can accept it (just press Enter)
/// or pick a different one.
pub fn init_spec(_ctx: &CliContext, path: &std::path::Path, minimal: bool) -> anyhow::Result<i32> {
    use makakoo_core::agents::llm_provider::{discover_providers, DiscoveredProvider};
    use makakoo_core::agents::llm_provider_default;
    use makakoo_core::agents::spec::{AgentSpec, ChannelSpec, ScopeSpec, TriggerSpec};
    use std::io::{IsTerminal, Write};

    if !std::io::stdin().is_terminal() {
        anyhow::bail!("init-spec requires a TTY (interactive)");
    }

    let project_default = llm_provider_default::get_default();
    let project_default_provider_id = project_default
        .as_ref()
        .and_then(|s| s.split('/').next())
        .map(|s| s.to_string());

    println!("\nInitializing agent spec at {}\n", path.display());

    // 1. Name
    print!("Agent name? > ");
    std::io::stdout().flush()?;
    let mut name = String::new();
    std::io::stdin().read_line(&mut name)?;
    let name = name.trim().to_string();
    if name.is_empty() {
        anyhow::bail!("agent name cannot be empty");
    }
    if let Err(e) = makakoo_core::agents::validate_slot_id(&name) {
        anyhow::bail!("invalid agent name: {}", e);
    }

    // 2. Description
    print!("Description? > ");
    std::io::stdout().flush()?;
    let mut desc = String::new();
    std::io::stdin().read_line(&mut desc)?;
    let desc = desc.trim().to_string();

    // 3–4. DSH is switchAILocal-only. Provider discovery and arbitrary
    // provider selection remain available only for the legacy Flue engine.
    let engine = crate::commands::agent_engine::selected_engine()?;
    let (provider_id, model_id) = if engine
        == makakoo_core::agents::AgentRuntimeEngine::DeepseekHarness
    {
        let configured = project_default
            .as_deref()
            .filter(|model| crate::commands::agent_engine::validate_model(engine, model).is_ok())
            .unwrap_or("switchailocal/ail-compound");
        let model = configured
            .strip_prefix("switchailocal/")
            .unwrap_or(configured)
            .to_string();
        println!("\nDeepSeek Harness uses switchAILocal model: {model}");
        ("switchailocal".to_string(), model)
    } else {
        println!("\nDetecting available LLM providers...");
        let providers = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(discover_providers())
        });
        if providers.is_empty() {
            println!("  ⚠ No LLM providers detected (no switchailocal, ollama, or env vars).");
            println!("  Defaulting to anthropic/claude-sonnet-4-6 (set ANTHROPIC_API_KEY later).");
            ("anthropic".to_string(), "claude-sonnet-4-6".to_string())
        } else {
            let pd_idx = project_default_provider_id
                .as_ref()
                .and_then(|pd| providers.iter().position(|p| &p.id == pd));
            println!("\n  Available providers:");
            for (i, p) in providers.iter().enumerate() {
                let marker = if Some(i) == pd_idx {
                    " ← project default"
                } else {
                    ""
                };
                println!(
                    "    {}. {} — model: {}{}",
                    i + 1,
                    p.display_name,
                    p.default_model,
                    marker
                );
            }
            if let Some(idx) = pd_idx {
                print!(
                    "\nWhich provider? [Enter for project default `{}`] > ",
                    providers[idx].id
                );
            } else {
                print!("\nWhich provider? [1] > ");
            }
            std::io::stdout().flush()?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            let chosen_idx = if input.trim().is_empty() {
                pd_idx.unwrap_or(0)
            } else {
                let choice = input
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| anyhow::anyhow!("provider choice must be a number"))?;
                if !(1..=providers.len()).contains(&choice) {
                    anyhow::bail!(
                        "provider choice {} is out of range 1..={}",
                        choice,
                        providers.len()
                    );
                }
                choice - 1
            };
            let p: &DiscoveredProvider = &providers[chosen_idx];
            (p.id.clone(), p.default_model.clone())
        }
    };

    // 5. Minimal mode → just name, description, model, instructions
    if minimal {
        let spec = AgentSpec {
            name: name.clone(),
            description: desc.clone(),
            model: format!("{}/{}", provider_id, model_id),
            instructions: "Reply with exactly: 'pong'".to_string(),
            tools: vec![],
            triggers: vec![],
            channels: vec![],
            scope: ScopeSpec {
                allowed_paths: vec!["~/MAKAKOO/data/**".to_string()],
                forbidden_paths: vec![
                    "~/.ssh/**".to_string(),
                    "~/.aws/**".to_string(),
                    "~/.gnupg/**".to_string(),
                ],
            },
        };
        if let Err(e) = spec.validate() {
            output::print_warn(format!("validation warning: {}", e));
        }
        let yaml = serde_yaml_ng::to_string(&spec)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            format!(
                "# Generated by `makakoo agent init-spec --minimal`\n{}\n",
                yaml
            ),
        )?;
        println!("\n✓ Wrote {} (minimal)", path.display());
        println!("Next: makakoo agent create --specs {}", path.display());
        return Ok(0);
    }

    // 6. Instructions
    print!("\nAgent instructions? (Enter for default) > ");
    std::io::stdout().flush()?;
    let mut instr = String::new();
    std::io::stdin().read_line(&mut instr)?;
    let instructions = if instr.trim().is_empty() {
        "Reply to the user concisely. Use the available tools when needed.".to_string()
    } else {
        instr.trim().to_string()
    };

    // 7. Channels
    print!("\nChannels? [telegram / slack / discord / webhook / email / voice / none] > ");
    std::io::stdout().flush()?;
    let mut ch = String::new();
    std::io::stdin().read_line(&mut ch)?;
    let channel_kind = ch.trim().to_lowercase();
    let mut channels: Vec<ChannelSpec> = vec![];
    match channel_kind.as_str() {
        "telegram" | "t" => {
            print!("  Bot token env var? [TELEGRAM_BOT_TOKEN] > ");
            std::io::stdout().flush()?;
            let mut te = String::new();
            std::io::stdin().read_line(&mut te)?;
            let token_env = if te.trim().is_empty() {
                "TELEGRAM_BOT_TOKEN".to_string()
            } else {
                te.trim().to_string()
            };
            print!("  Allowed user IDs (comma-sep, Enter to skip)? > ");
            std::io::stdout().flush()?;
            let mut au = String::new();
            std::io::stdin().read_line(&mut au)?;
            let allowed_users: Vec<String> = au
                .trim()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            channels.push(ChannelSpec::Telegram {
                token_env,
                allowed_users,
            });
        }
        "none" | "n" | "" => {}
        _ => {
            println!(
                "  Unknown channel kind `{}`, skipping channels.",
                channel_kind
            );
        }
    }

    // 8. Triggers
    print!("\nTriggers? [cron / webhook / none] > ");
    std::io::stdout().flush()?;
    let mut tr = String::new();
    std::io::stdin().read_line(&mut tr)?;
    let trigger_kind = tr.trim().to_lowercase();
    let mut triggers: Vec<TriggerSpec> = vec![];
    match trigger_kind.as_str() {
        "cron" | "c" => {
            print!("  Schedule? (e.g. '0 9 * * *') > ");
            std::io::stdout().flush()?;
            let mut s = String::new();
            std::io::stdin().read_line(&mut s)?;
            let schedule = s.trim().to_string();
            print!("  Timezone? [UTC] > ");
            std::io::stdout().flush()?;
            let mut tz = String::new();
            std::io::stdin().read_line(&mut tz)?;
            let timezone = if tz.trim().is_empty() {
                "UTC".to_string()
            } else {
                tz.trim().to_string()
            };
            triggers.push(TriggerSpec::Cron { schedule, timezone });
        }
        "none" | "n" | "" => {}
        _ => {
            println!(
                "  Unknown trigger kind `{}`, skipping triggers.",
                trigger_kind
            );
        }
    }

    // 9. Tools
    print!("\nTools? [brain_search, brain_recent, write_file, web_search, none] (comma-sep) > ");
    std::io::stdout().flush()?;
    let mut ti = String::new();
    std::io::stdin().read_line(&mut ti)?;
    let tools: Vec<String> = if ti.trim().is_empty() || ti.trim().eq_ignore_ascii_case("none") {
        vec![]
    } else {
        ti.trim()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    // 10. Scope
    print!("\nScope allowed paths? (glob, Enter for default: ~/MAKAKOO/data/**) > ");
    std::io::stdout().flush()?;
    let mut sp = String::new();
    std::io::stdin().read_line(&mut sp)?;
    let allowed_paths: Vec<String> = if sp.trim().is_empty() {
        vec!["~/MAKAKOO/data/**".to_string()]
    } else {
        sp.trim()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    // 11. Build the spec
    let spec = AgentSpec {
        name: name.clone(),
        description: desc.clone(),
        model: format!("{}/{}", provider_id, model_id),
        instructions,
        tools: tools.clone(),
        channels,
        triggers,
        scope: ScopeSpec {
            allowed_paths: allowed_paths.clone(),
            forbidden_paths: vec![
                "~/.ssh/**".to_string(),
                "~/.aws/**".to_string(),
                "~/.gnupg/**".to_string(),
            ],
        },
    };

    if let Err(e) = spec.validate() {
        output::print_warn(format!("validation warning: {}", e));
    }

    // 12. Write the spec
    let yaml = serde_yaml_ng::to_string(&spec)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        format!("# Generated by `makakoo agent init-spec`\n{}\n", yaml),
    )?;
    println!("\n✓ Wrote {}", path.display());
    println!("  model: {}/{}", provider_id, model_id);
    println!("  channels: {}", spec.channels.len());
    println!("  triggers: {}", spec.triggers.len());
    println!("  tools: {}", spec.tools.len());
    println!("\nNext: makakoo agent create --specs {}", path.display());

    Ok(0)
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use makakoo_core::agents::spec::ScopeSpec;
    use makakoo_core::agents::AgentRuntimeEngine;

    fn spec(model: &str) -> AgentSpec {
        AgentSpec {
            name: "validator-test".into(),
            description: "validation test".into(),
            model: model.into(),
            instructions: "Reply concisely.".into(),
            tools: vec![],
            triggers: vec![],
            channels: vec![],
            scope: ScopeSpec {
                allowed_paths: vec!["~/MAKAKOO/data/test/**".into()],
                forbidden_paths: vec!["~/.ssh/**".into()],
            },
        }
    }

    #[test]
    fn validate_spec_applies_selected_engine_model_contract() {
        validate_spec_for_engine(
            &spec("switchailocal/ail-compound"),
            AgentRuntimeEngine::DeepseekHarness,
        )
        .unwrap();
        assert!(validate_spec_for_engine(
            &spec("ollama/qwen3:8b"),
            AgentRuntimeEngine::DeepseekHarness,
        )
        .is_err());
        validate_spec_for_engine(&spec("ollama/qwen3:8b"), AgentRuntimeEngine::Flue).unwrap();
    }
}
