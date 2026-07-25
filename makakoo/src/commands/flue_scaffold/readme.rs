//! README.md — operator-facing project documentation. Generated
//! from the spec: lists channels, triggers, env vars, run steps.

use makakoo_core::agents::spec::{ChannelSpec, TriggerSpec};

use super::context::RenderContext;

pub fn render(ctx: &RenderContext) -> String {
    let project = ctx.project_name();
    let spec = ctx.spec;
    let mut s = String::new();

    s.push_str(&format!("# {}\n\n", project));
    s.push_str(&format!(
        "A [Flue](https://flueframework.com) agent scaffolded by `makakoo agent create \
{} --specs ./<spec>.yaml`.\n\n",
        spec.name
    ));
    s.push_str(&format!("> {}\n\n", spec.description));
    s.push_str("Makakoo owns identity, scope, secrets and the registry (the `");
    s.push_str(&spec.name);
    s.push_str("` slot); this project is the runnable agent. It reaches Makakoo's Brain, skills and tools over **MCP** through a local stdio→HTTP proxy.\n\n");

    // Channels
    s.push_str("## Channels\n\n");
    if spec.channels.is_empty() {
        s.push_str("_None — this agent has no inbound channels. It only runs on triggers._\n\n");
    } else {
        s.push_str("| # | Kind | Env vars |\n");
        s.push_str("|---|------|----------|\n");
        for (i, c) in spec.channels.iter().enumerate() {
            let kind = channel_kind_name(c);
            let envs = channel_env_list(c).join(", ");
            s.push_str(&format!("| {} | `{}` | `{}` |\n", i, kind, envs));
        }
        s.push_str("\n");
    }

    // Triggers
    s.push_str("## Triggers\n\n");
    if spec.triggers.is_empty() {
        s.push_str("_None — this agent only responds to channel messages._\n\n");
    } else {
        s.push_str("| # | Kind | Schedule / path |\n");
        s.push_str("|---|------|------------------|\n");
        for (i, t) in spec.triggers.iter().enumerate() {
            let (kind, sched) = trigger_summary(t);
            s.push_str(&format!("| {} | `{}` | `{}` |\n", i, kind, sched));
        }
        s.push_str("\n");
    }

    s.push_str("## Run\n\n```sh\n");
    s.push_str("npm install\n");
    s.push_str("cp .env.example .env          # fill in the values listed above\n");
    s.push_str("npm run proxy                 # terminal 1: makakoo-mcp over http://127.0.0.1:8808/mcp\n");
    s.push_str("npx flue dev                  # terminal 2: runs the agent\n");
    s.push_str("```\n\n");
    s.push_str("## Files\n\n");
    s.push_str("- `src/agents/assistant.ts` — the agent: model + instructions + Makakoo MCP tools (whitelisted to the spec's `tools` list).\n");
    s.push_str("- `src/channels/*.ts` — one module per channel declared in the spec.\n");
    s.push_str("- `src/triggers/*.ts` — one module per trigger declared in the spec.\n");
    s.push_str("- `mcp-proxy.mjs` — stdio→StreamableHTTP bridge to the local `makakoo-mcp` binary.\n");
    s.push_str("- `instructions.txt` — the agent's system instructions (from the spec).\n");
    s.push_str("- `spec.yaml` — the source-of-truth spec used to scaffold this project.\n");

    s
}

fn channel_kind_name(c: &ChannelSpec) -> &'static str {
    match c {
        ChannelSpec::Telegram { .. } => "telegram",
        ChannelSpec::Slack { .. } => "slack",
        ChannelSpec::Discord { .. } => "discord",
        ChannelSpec::Webhook { .. } => "webhook",
        ChannelSpec::Email { .. } => "email",
        ChannelSpec::Voice { .. } => "voice",
    }
}

fn channel_env_list(c: &ChannelSpec) -> Vec<String> {
    match c {
        ChannelSpec::Telegram { token_env, .. } => vec![token_env.clone()],
        ChannelSpec::Slack { token_env, app_token_env, team_id_env, .. } => vec![
            token_env.clone(),
            app_token_env.clone(),
            team_id_env.clone(),
        ],
        ChannelSpec::Discord { token_env, .. } => vec![token_env.clone()],
        ChannelSpec::Webhook { secret_env, .. } => vec![secret_env.clone()],
        ChannelSpec::Email { secret_env, .. } => vec![secret_env.clone()],
        ChannelSpec::Voice { twilio_account_sid_env, secret_env, .. } => vec![
            twilio_account_sid_env.clone(),
            secret_env.clone(),
        ],
    }
}

fn trigger_summary(t: &TriggerSpec) -> (&'static str, String) {
    match t {
        TriggerSpec::Cron { schedule, timezone } => {
            let tz = if timezone.is_empty() { "UTC".to_string() } else { timezone.clone() };
            ("cron", format!("`{}` ({})", schedule, tz))
        }
        TriggerSpec::Webhook { path, .. } => ("webhook", format!("`POST {}`", path)),
    }
}
