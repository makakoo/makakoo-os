//! `makakoo agent create --specs <PATH>` — scaffold a Flue (TypeScript)
//! agent project wired to Makakoo's MCP server. The agent's runtime is
//! declared in a spec (model, instructions, tools, channels, triggers,
//! scope); this module renders the spec into a runnable Flue project.
//!
//! Flue is the only creation engine on Makakoo OS. The `agent create`
//! command always invokes this scaffolder; there is no native-only path.
//!
//! Per-channel / per-trigger renderers live in `channels/` and `triggers/`.
//! Each renderer emits a single TS module that exports:
//!
//! * `tool(id: string)` — returns a Flue tool the agent can call to send
//!   to that channel's bound conversation (channels only).
//! * `trigger()` — returns a Flue trigger definition (triggers only).
//! * `channel` — the channel runtime object (channels only; used by the
//!   webhook handler).
//!
//! Phase 4 of SPRINT-FLUE-DEFAULT-AGENT-SPECS.

pub mod app;
pub mod assistant;
pub mod channels;
pub mod context;
pub mod env_example;
pub mod gitignore;
pub mod mcp_proxy;
pub mod package_json;
pub mod readme;
pub mod triggers;

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use makakoo_core::agents::llm_provider::DiscoveredProvider;
use makakoo_core::agents::spec::AgentSpec;

use self::context::RenderContext;

/// Inline secrets to pre-fill `.env` (in addition to the always-emitted
/// `.env.example`). Used by the quick-start path: when the operator
/// supplies a token via `--telegram-token <T>`, that value lands in
/// the generated `.env` so `npx flue dev` works without manual copy.
///
/// The spec path passes an empty map — spec env-var names are
/// resolved from the operator's environment, not the spec.
pub type InlineSecrets = HashMap<String, String>;

/// Write a runnable Flue agent project into `out_dir`. Refuses to clobber a
/// non-empty directory. The spec is the source of truth — every file
/// emitted is derived from it.
///
/// Phase 6: when `llm_provider` is `Some`, the scaffolder emits
/// `src/app.ts` with the right `registerProvider` call. When `None`,
/// the scaffolder skips `app.ts` and the caller surfaces a clear error.
pub fn scaffold_flue_project(
    spec: &AgentSpec,
    out_dir: &Path,
    inline_secrets: &InlineSecrets,
    llm_provider: Option<&DiscoveredProvider>,
) -> Result<()> {
    if out_dir.exists()
        && out_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        anyhow::bail!(
            "flue output dir {} already exists and is non-empty — refusing to overwrite",
            out_dir.display()
        );
    }

    let ctx = RenderContext {
        spec,
        out_dir,
        llm_provider: llm_provider.cloned(),
    };
    let _ = ctx.project_name();

    ctx.write("package.json", &package_json::render(&ctx))?;
    ctx.write("mcp-proxy.mjs", mcp_proxy::MCP_PROXY)?;
    ctx.write(
        "src/agents/assistant.ts",
        &assistant::render(&ctx),
    )?;

    // Phase 6: write src/app.ts with the chosen LLM provider's
    // registerProvider call. If no provider was chosen, skip
    // (caller will surface a clear error).
    if let Some(provider) = ctx.llm_provider.as_ref() {
        ctx.write("src/app.ts", &app::render(provider))?;
    }
    ctx.write("instructions.txt", &ctx.spec.instructions)?;
    ctx.write(".env.example", &env_example::render(&ctx))?;
    if !inline_secrets.is_empty() {
        ctx.write(".env", &render_dotenv(&env_example::render(&ctx), inline_secrets))?;
    }
    ctx.write(".gitignore", gitignore::GITIGNORE)?;
    ctx.write("README.md", &readme::render(&ctx))?;
    ctx.write("spec.yaml", &ctx.spec.to_yaml()?)?;

    // Channel modules — one per spec.channels[] entry.
    for (i, c) in ctx.spec.channels.iter().enumerate() {
        let rel = channels::rel_path(i, c);
        let body = channels::render(i, c).with_context(|| {
            format!("rendering channel {} for spec '{}'", i, ctx.spec.name)
        })?;
        ctx.write(&rel, &body)?;
    }

    // Trigger modules — one per spec.triggers[] entry.
    for (i, t) in ctx.spec.triggers.iter().enumerate() {
        let rel = triggers::rel_path(i, t);
        let body = triggers::render(i, t).with_context(|| {
            format!("rendering trigger {} for spec '{}'", i, ctx.spec.name)
        })?;
        ctx.write(&rel, &body)?;
    }

    Ok(())
}

/// Build `.env` from the `.env.example` template, filling in any
/// inline secrets. Lines that don't match a key are left as comments.
fn render_dotenv(template: &str, secrets: &InlineSecrets) -> String {
    let mut out = String::new();
    for line in template.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(|c: char| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()) {
            // Possible `KEY=value` line — split on first '='.
            if let Some((key, _)) = rest.split_once('=') {
                let key = trimmed.split_once('=').map(|(k, _)| k.trim()).unwrap_or(key.trim());
                if let Some(value) = secrets.get(key) {
                    out.push_str(&format!("{}={}\n", key, value));
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
