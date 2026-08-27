//! package.json — emits the runnable Flue project's manifest with
//! dependencies that match the spec's channels and triggers. Specs
//! that don't use a given channel don't pull in its npm package.

use std::collections::BTreeMap;

use makakoo_core::agents::spec::{ChannelSpec, TriggerSpec};

use super::context::RenderContext;

pub fn render(ctx: &RenderContext) -> String {
    let project = ctx.project_name();
    let mut deps: BTreeMap<&'static str, &'static str> = BTreeMap::new();
    let mut dev_deps: BTreeMap<&'static str, &'static str> = BTreeMap::new();

    // Base deps — always present.
    deps.insert("@flue/runtime", "^1.0.0-beta.9");
    deps.insert("@modelcontextprotocol/sdk", "^1.29.0");
    deps.insert("@hono/node-server", "^2.0.3");
    deps.insert("hono", "^4.6.0");
    // Valibot is the schema library Flue's `defineTool` uses for
    // typed input. We declare it as a direct dep (it's already a
    // transitive dep of @flue/runtime) so it's not tree-shaken
    // away and the operator can `import * as v from 'valibot'`.
    deps.insert("valibot", "^1.0.0");

    // Per-channel deps.
    for c in &ctx.spec.channels {
        for (k, v) in channel_deps(c) {
            deps.insert(k, v);
        }
    }

    // Per-trigger deps.
    for t in &ctx.spec.triggers {
        for (k, v) in trigger_deps(t) {
            deps.insert(k, v);
        }
    }

    dev_deps.insert("@flue/cli", "^1.0.0-beta.1");
    dev_deps.insert("@types/node-cron", "^3.0.0");

    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"name\": \"{}\",\n", project));
    s.push_str("  \"private\": true,\n");
    s.push_str("  \"type\": \"module\",\n");
    s.push_str("  \"scripts\": {\n");
    s.push_str("    \"proxy\": \"node mcp-proxy.mjs\",\n");
    s.push_str("    \"dev\": \"flue dev\",\n");
    s.push_str("    \"build\": \"flue build --target node\"\n");
    s.push_str("  },\n");
    s.push_str("  \"dependencies\": {\n");
    for (i, (k, v)) in deps.iter().enumerate() {
        let suffix = if i + 1 < deps.len() { "," } else { "" };
        s.push_str(&format!("    \"{}\": \"{}\"{}\n", k, v, suffix));
    }
    s.push_str("  },\n");
    s.push_str("  \"devDependencies\": {\n");
    for (i, (k, v)) in dev_deps.iter().enumerate() {
        let suffix = if i + 1 < dev_deps.len() { "," } else { "" };
        s.push_str(&format!("    \"{}\": \"{}\"{}\n", k, v, suffix));
    }
    s.push_str("  }\n");
    s.push_str("}\n");
    s
}

fn channel_deps(c: &ChannelSpec) -> Vec<(&'static str, &'static str)> {
    match c {
        ChannelSpec::Telegram { .. } => {
            vec![("@flue/telegram", "^1.0.0-beta.1"), ("grammy", "^1.0.0")]
        }
        ChannelSpec::Slack { .. } => vec![
            // V1: @flue/slack doesn't expose a bot client; operators
            // wire their own outbound tool. No @slack/bolt needed.
            ("@flue/slack", "^1.0.0-beta.1"),
        ],
        ChannelSpec::Discord { .. } => vec![
            // V1: @flue/discord is interaction-only; no discord.js.
            ("@flue/discord", "^1.0.0-beta.1"),
        ],
        ChannelSpec::Webhook { .. } => vec![],
        // V1: email and voice are deferred — no first-party @flue/*
        // adapter. Operators use a webhook channel + their own
        // defineTool for SMTP/IMAP/Twilio.
        ChannelSpec::Email { .. } => vec![],
        ChannelSpec::Voice { .. } => vec![],
    }
}

fn trigger_deps(t: &TriggerSpec) -> Vec<(&'static str, &'static str)> {
    match t {
        TriggerSpec::Cron { .. } => vec![("node-cron", "^3.0.0")],
        TriggerSpec::Webhook { .. } => vec![],
    }
}
