//! Channel renderers — one TS module per spec.channels[] entry.
//!
//! Each renderer emits a module that exports:
//!
//! * `tool(id: string)` — returns a Flue tool the agent can call to send
//!   a message back to the bound conversation.
//! * `channel` — the channel runtime object (used by `app.ts` to wire
//!   up the webhook handler).
//!
//! Index `i` is the channel's position in `spec.channels[]`. The
//! emitted filename is `<kind>-<i>.ts` and the import alias is
//! `<kind><i>` (e.g. `telegram0`, `slack1`).

pub mod discord;
pub mod email;
pub mod slack;
pub mod telegram;
pub mod voice;
pub mod webhook;

use anyhow::{Context as _, Result};
use makakoo_core::agents::spec::ChannelSpec;

use super::context::RenderContext;

/// Relative path under the Flue project root, e.g.
/// `src/channels/telegram-0.ts`.
pub fn rel_path(i: usize, c: &ChannelSpec) -> String {
    format!("src/channels/{}-{}.ts", kind_slug(c), i)
}

/// JS import alias for the assistant.ts static import. Prefixed
/// with `ch_` so channels and triggers can't collide (a `webhook`
/// channel and a `webhook` trigger would otherwise both alias to
/// `webhook0`).
pub fn import_alias(i: usize, c: &ChannelSpec) -> String {
    format!("ch_{}{}", kind_slug(c), i)
}

fn kind_slug(c: &ChannelSpec) -> &'static str {
    match c {
        ChannelSpec::Telegram { .. } => "telegram",
        ChannelSpec::Slack { .. } => "slack",
        ChannelSpec::Discord { .. } => "discord",
        ChannelSpec::Webhook { .. } => "webhook",
        ChannelSpec::Email { .. } => "email",
        ChannelSpec::Voice { .. } => "voice",
    }
}

/// Dispatch a channel spec to its renderer. The renderer returns
/// the full TS module body.
pub fn render(i: usize, c: &ChannelSpec) -> Result<String> {
    let _ = i; // index is encoded by the caller via rel_path/import_alias
    let _ = RenderContext::new; // silence unused import when not all paths use ctx
    let body = match c {
        ChannelSpec::Telegram {
            token_env,
            allowed_users,
        } => telegram::render(token_env, allowed_users),
        ChannelSpec::Slack {
            token_env,
            app_token_env,
            team_id_env,
            allowed_users,
        } => slack::render(token_env, app_token_env, team_id_env, allowed_users),
        ChannelSpec::Discord {
            token_env,
            allowed_users,
        } => discord::render(token_env, allowed_users),
        ChannelSpec::Webhook { path, secret_env } => webhook::render(path, secret_env),
        ChannelSpec::Email {
            smtp_host,
            imap_host,
            secret_env,
        } => email::render(smtp_host, imap_host, secret_env),
        ChannelSpec::Voice {
            twilio_account_sid_env,
            secret_env,
        } => voice::render(twilio_account_sid_env, secret_env),
    };
    body.with_context(|| format!("rendering channel {:?}", c))
}
