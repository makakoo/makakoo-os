use super::context::RenderContext;

pub fn render(ctx: &RenderContext<'_>) -> String {
    format!(
        r#"# {name}

DeepSeek Harness execution project compiled from `spec.yaml` by Makakoo OS.
Do not edit generated identity, model or scope here and expect it to become
canonical; update the AgentSpec and recreate the slot.

## Install and run

Prerequisites: Node.js 22.9 or newer, a running switchAILocal endpoint, and
`makakoo-mcp` on `PATH` (or set `MAKAKOO_MCP_BIN` in `.env`).

```bash
npm install
makakoo agent start {name}
makakoo agent status {name}
makakoo agent prompt {name} "Summarize the current project state"
```

The runtime exposes an authenticated loopback API. `runtime.json` contains the
ephemeral port and points to the per-start mode-0600 bearer-token file. Normal callers
should use `makakoo agent prompt`, not read those files directly.
Cross-session execution is bounded by `MAKAKOO_DSH_MAX_CONCURRENT` (default 4);
turns within one session remain serialized and at most 32 turns wait in the
global queue by default.
Durable state is admission-bounded to 128 session ids, 1,000 turns per
session, 512 MiB of session storage, and 128 KiB per prompt by default. Tune
the corresponding `MAKAKOO_DSH_MAX_*` values in `.env` deliberately.

## Trust boundary

- DSH owns the model loop and JSONL sessions under `.sessions/`.
- `makakoo-mcp` is the only generated tool source.
- `MAKAKOO_AGENT_SLOT={name}` is passed into `makakoo-mcp`.
- Makakoo filters tool discovery and rejects out-of-scope calls server-side.
- No DSH shell or native filesystem tool is mounted.
- DSH V1 does not start channel listeners or trigger schedulers.

## Configuration

Copy `.env.example` to `.env` only when overriding defaults. The runner maps
`AIL_API_KEY` to the DSH provider automatically; `DEEPSEEK_API_KEY` can override
it. The fixed model endpoint is switchAILocal at
`http://127.0.0.1:18080/v1`. DSH dependencies are
pinned to a release candidate; upgrades require the Makakoo integration suite.
"#,
        name = ctx.spec.name,
    )
}
