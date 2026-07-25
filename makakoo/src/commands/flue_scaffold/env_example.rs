//! .env.example — environment variables the Flue project reads at
//! runtime. Generated from the spec's channels and triggers.

use makakoo_core::agents::spec::{ChannelSpec, TriggerSpec};

use super::context::RenderContext;

pub fn render(ctx: &RenderContext) -> String {
    let mut out = String::new();
    out.push_str("# Makakoo MCP endpoint exposed by `npm run proxy` (mcp-proxy.mjs)\n");
    out.push_str("MAKAKOO_MCP_URL=http://127.0.0.1:8808/mcp\n");
    out.push_str("\n");
    out.push_str("# Optional model override (defaults to the value from the spec)\n");
    out.push_str(&format!("AGENT_MODEL={}\n", ctx.spec.model));
    out.push_str("\n");

    out.push_str("# Channels\n");
    if ctx.spec.channels.is_empty() {
        out.push_str("# (no channels declared in the spec — only triggers are wired)\n");
    }
    for c in &ctx.spec.channels {
        for (var, hint) in channel_env(c) {
            out.push_str(&format!("# {}\n", hint));
            out.push_str(&format!("{}=\n", var));
        }
    }
    out.push_str("\n");

    out.push_str("# Triggers\n");
    if ctx.spec.triggers.is_empty() {
        out.push_str("# (no triggers declared in the spec — agent only reacts to channels)\n");
    }
    for t in &ctx.spec.triggers {
        for (var, hint) in trigger_env(t) {
            out.push_str(&format!("# {}\n", hint));
            out.push_str(&format!("{}=\n", var));
        }
    }

    out
}

fn channel_env(c: &ChannelSpec) -> Vec<(String, String)> {
    match c {
        ChannelSpec::Telegram { token_env, .. } => vec![
            (token_env.clone(), "Telegram bot token (from @BotFather)".into()),
            ("TELEGRAM_WEBHOOK_SECRET_TOKEN".into(), "Random string; set the same value on setWebhook".into()),
        ],
        ChannelSpec::Slack { token_env, app_token_env, team_id_env, .. } => vec![
            (token_env.clone(), "Slack bot token (xoxb-…)".into()),
            (app_token_env.clone(), "Slack app token (xapp-…)".into()),
            (team_id_env.clone(), "Slack workspace team_id (T0123ABCD)".into()),
        ],
        ChannelSpec::Discord { token_env, .. } => vec![
            (token_env.clone(), "Discord bot token".into()),
        ],
        ChannelSpec::Webhook { secret_env, .. } => vec![
            (secret_env.clone(), "HMAC secret for inbound webhook signature verification".into()),
        ],
        ChannelSpec::Email { secret_env, .. } => vec![
            (secret_env.clone(), "Email credential JSON ({\"user\":\"…\",\"pass\":\"…\"})".into()),
        ],
        ChannelSpec::Voice { twilio_account_sid_env, secret_env, .. } => vec![
            (twilio_account_sid_env.clone(), "Twilio Account SID (AC…)".into()),
            (secret_env.clone(), "Twilio Auth Token".into()),
        ],
    }
}

fn trigger_env(t: &TriggerSpec) -> Vec<(String, String)> {
    match t {
        TriggerSpec::Cron { .. } => vec![],
        TriggerSpec::Webhook { secret_env, .. } => vec![
            (secret_env.clone(), "HMAC secret for trigger webhook signature verification".into()),
        ],
    }
}
