# SPRINT-DSH-AGENT-RUNTIME-V1

## Goal

Keep `AgentSpec` as Makakoo's only agent-definition surface and make
DeepSeek Harness (DSH) the default cognitive runtime behind it. Flue remains
available as an emergency compatibility renderer; it no longer owns the
default agent loop.

## Architecture

```text
AgentSpec
  -> AgentSlot + runtime metadata
  -> DSH project compiler
      -> Cordis composition
      -> switchAILocal-compatible model route
      -> dsh-mcp-client -> makakoo-mcp (stdio)
      -> JSONL sessions + semantic checkpoints
      -> authenticated loopback runtime API
  -> Makakoo slot supervisor owns runner.mjs
  -> makakoo agent prompt drives a named durable session
```

Makakoo remains authoritative for identity, tool allowlists, path/grant
policy, secrets, lifecycle and channels. DSH owns model turns, tool-loop
execution and session durability. `makakoo-mcp` enforces the slot tool scope
server-side; hiding tools in a prompt is not a security boundary.

## Implementation slices

1. Add optional runtime metadata to `AgentSlot` without changing legacy-slot
   behavior.
2. Add a modular DSH scaffolder that emits pinned dependencies, a Cordis
   composition, an authenticated runner and operator documentation.
3. Route `agent create` through DSH by default. Preserve Flue through the
   operator-only `MAKAKOO_AGENT_ENGINE=flue` escape hatch.
4. Teach the slot supervisor to launch the runtime declared by the slot.
5. Add `makakoo agent prompt <slot> <text>` for direct end-to-end use.
6. Filter `tools/list` and gate `tools/call` in `makakoo-mcp` using
   `MAKAKOO_AGENT_SLOT`.

## Explicit V1 boundary

Existing Telegram/Slack/Discord ingress remains a Makakoo/Flue transport
concern. This sprint delivers the DSH execution engine and local runtime API;
it does not pretend DSH has native channel adapters. Channel adapters can call
the authenticated `/v1/run` endpoint in the next slice without changing
`AgentSpec` or the runtime contract.

## Acceptance gates

- Generated project installs from the public pinned DSH RC packages.
- Cordis config mounts only Makakoo MCP tools; no DSH shell/filesystem tools.
- Model requests target switchAILocal's OpenAI-compatible endpoint.
- Slot identity reaches the spawned `makakoo-mcp` process.
- Out-of-scope tools are absent from `tools/list` and rejected by `tools/call`.
- Supervisor selects DSH only for slots declaring the DSH runtime.
- Legacy slots still launch the legacy gateway.
- Focused Rust tests, generated-project syntax checks and a live local
  switchAILocal runtime smoke test pass.
