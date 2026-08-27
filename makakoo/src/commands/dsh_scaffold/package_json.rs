use super::{context::RenderContext, DSH_VERSION};

pub fn render(ctx: &RenderContext<'_>) -> String {
    let packages = [
        "@deepseek-ai/dsh-agent-spine-demo",
        "@deepseek-ai/dsh-compaction-basic",
        "@deepseek-ai/dsh-llm-deepseek",
        "@deepseek-ai/dsh-mcp-client",
        "@deepseek-ai/dsh-sdk-client",
        "@deepseek-ai/dsh-sdk-jsonrpc-demo",
        "@deepseek-ai/dsh-sdk-jsonrpc-server",
        "@deepseek-ai/dsh-session-checkpoint-policy",
        "@deepseek-ai/dsh-session-persistence-jsonl",
        "@deepseek-ai/dsh-subprocess",
        "@deepseek-ai/dsh-token-meter",
    ];
    let dependencies = packages
        .into_iter()
        .map(|name| (name.to_string(), serde_json::json!(DSH_VERSION)))
        .collect::<serde_json::Map<_, _>>();
    let value = serde_json::json!({
        "name": format!("{}-makakoo-agent", ctx.spec.name),
        "private": true,
        "version": "0.1.0",
        "type": "module",
        "engines": { "node": ">=22.9" },
        "scripts": {
            "start": "node --env-file-if-exists=.env runner.mjs",
            "check": "node --check runner.mjs"
        },
        "dependencies": dependencies
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("JSON render")
    )
}
