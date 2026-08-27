use std::collections::{BTreeSet, HashMap};

use makakoo_core::agents::spec::{AgentSpec, ChannelSpec, TriggerSpec};

pub fn render(spec: &AgentSpec) -> String {
    let mut keys = BTreeSet::new();
    for channel in &spec.channels {
        match channel {
            ChannelSpec::Telegram { token_env, .. } | ChannelSpec::Discord { token_env, .. } => {
                keys.insert(token_env.as_str());
            }
            ChannelSpec::Slack {
                token_env,
                app_token_env,
                team_id_env,
                ..
            } => {
                keys.extend([
                    token_env.as_str(),
                    app_token_env.as_str(),
                    team_id_env.as_str(),
                ]);
            }
            ChannelSpec::Webhook { secret_env, .. }
            | ChannelSpec::Email { secret_env, .. }
            | ChannelSpec::Voice { secret_env, .. } => {
                keys.insert(secret_env.as_str());
            }
        }
    }
    for trigger in &spec.triggers {
        if let TriggerSpec::Webhook { secret_env, .. } = trigger {
            keys.insert(secret_env.as_str());
        }
    }

    let mut out = String::from(
        "# Makakoo/DSH runtime\n\
MAKAKOO_MCP_BIN=makakoo-mcp\n\
# DEEPSEEK_API_KEY=use-AIL_API_KEY-or-a-local-placeholder\n\
DSH_CONTEXT_WINDOW=262144\n\
MAKAKOO_DSH_PORT=0\n\
MAKAKOO_DSH_MAX_CONCURRENT=4\n\
MAKAKOO_DSH_MAX_QUEUED=32\n\
MAKAKOO_DSH_MAX_SESSIONS=128\n\
MAKAKOO_DSH_MAX_TURNS_PER_SESSION=1000\n\
MAKAKOO_DSH_MAX_SESSION_BYTES=536870912\n\
MAKAKOO_DSH_MAX_PROMPT_BYTES=131072\n",
    );
    if !keys.is_empty() {
        out.push_str("\n# Reserved for Makakoo channel adapters\n");
        for key in keys {
            out.push_str(key);
            out.push_str("=\n");
        }
    }
    out
}

pub fn fill(template: &str, secrets: &HashMap<String, String>) -> String {
    template
        .lines()
        .map(|line| {
            let Some((key, _)) = line.split_once('=') else {
                return line.to_string();
            };
            secrets
                .get(key)
                .map(|value| format!("{key}={value}"))
                .unwrap_or_else(|| line.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}
