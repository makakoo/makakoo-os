---
name: deepseek-harness-agent-runtime
version: 0.1.0
description: |
  Create, validate, run, prompt, inspect, restart, and destroy supervised
  Makakoo agent slots backed by DeepSeek Harness. Use when the user asks to
  create an agent, run an AgentSpec, choose an agent harness, continue a
  durable agent session, or diagnose a generated agents-dsh runtime.
allowed-tools:
  - shell
category: agents
tags:
  - agent-runtime
  - deepseek-harness
  - agentspec
  - switchailocal
  - supervision
---

# deepseek-harness-agent-runtime

Makakoo owns identity, policy, lifecycle, and tool scope. DeepSeek Harness
owns the model loop and durable sessions. The default flow is:

```sh
makakoo agent validate-spec ./agent.yaml
makakoo agent create --specs ./agent.yaml
cd "$MAKAKOO_HOME/agents-dsh/<slot>"
npm install
makakoo agent validate <slot>
makakoo agent start <slot>
makakoo agent prompt <slot> "hello" --session cli-default
```

Use the shell tool for these commands. Do not edit a generated runtime and
pretend it is canonical. Change the AgentSpec, destroy or archive the old
slot, then recreate it.

## Trigger phrases

Use this skill when the user says:

- "create an agent" or "make me an agent"
- "run this AgentSpec"
- "use DeepSeek Harness"
- "which agent harness does Makakoo use?"
- "continue agent session X"
- "why won't my Makakoo agent start?"
- "list, stop, restart, or destroy my agent"

Do not use this skill for:

- bounded coding subagents recorded by `makakoo agent-session`;
- long-running plugin processes from `plugins-core/agent-*`;
- installing a prebuilt agent plugin with `makakoo plugin install`.

## Source of truth

An AgentSpec is YAML or TOML with these core fields:

```yaml
name: local-researcher
description: Researches the local Brain with a narrow tool set
model: switchailocal/ail-compound
instructions: |
  Answer from the Makakoo Brain. State when evidence is missing.
tools:
  - brain_search
channels: []
triggers: []
scope:
  allowed_paths:
    - "~/MAKAKOO/data/Brain/**"
  forbidden_paths:
    - "~/.ssh/**"
```

Validate before mutation:

```sh
makakoo agent validate-spec ./local-researcher.yaml
```

Then compile it:

```sh
makakoo agent create --specs ./local-researcher.yaml
```

`--specs` also accepts a directory or `.`. Batch creation preflights the
whole set and refuses duplicate names or existing slots before writing.

## Generated runtime

Default output:

```text
$MAKAKOO_HOME/agents-dsh/<slot>/
```

The generated project requires Node.js 22.9 or newer. Run `npm install` once.
Direct DSH packages are exact-pinned to `0.1.1-rc.2`; do not upgrade them
independently of Makakoo's integration suite.

The runtime:

- binds an authenticated API to `127.0.0.1` on an ephemeral port;
- stores runtime metadata and a mode-0600 bearer token inside the project;
- routes model calls only through switchAILocal;
- mounts `makakoo-mcp` as the only model-facing tool source;
- passes `MAKAKOO_AGENT_SLOT` so MCP filters tools server-side;
- does not mount DSH shell or native filesystem tools;
- serializes turns within a session and bounds cross-session concurrency.

The runner uses `DEEPSEEK_API_KEY`, then `AIL_API_KEY`, then a local
placeholder. Model traffic is pinned to `http://127.0.0.1:18080/v1`;
project `.env` files cannot redirect the switchAILocal endpoint.

## Lifecycle

```sh
makakoo agent validate local-researcher
makakoo agent start local-researcher
makakoo agent status local-researcher
makakoo agent health local-researcher
makakoo agent prompt local-researcher "Summarize today's decisions"
makakoo agent prompt local-researcher "What changed?" --session daily
makakoo agent restart local-researcher
makakoo agent stop local-researcher
```

Reuse the same `--session` id for durable continuation. Use a different id
for an isolated conversation.

`start` installs a launchd service on macOS or a systemd-user service on
Linux after preflight. On unsupported platforms, use the foreground
supervisor escape printed by the CLI.

## Destruction

```sh
makakoo agent destroy local-researcher
```

Destruction first proves the service is stopped. It then archives managed
slot data and the generated runtime under
`$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/`. It does not silently revoke
referenced secrets. Use `--revoke-secrets` only when the user explicitly
wants that.

## Channel boundary

DSH V1 does **not** start Telegram, Slack, Discord, WhatsApp, email, voice,
webhook, or cron listeners. AgentSpec keeps those declarations for the next
Makakoo adapter slice, and creation prints a warning when they are present.
Until then, invoke the runtime with `makakoo agent prompt` or a trusted local
adapter that calls the authenticated loopback endpoint.

The legacy Flue renderer remains available only when the operator explicitly
sets:

```sh
MAKAKOO_AGENT_ENGINE=flue makakoo agent create --specs ./agent.yaml
```

Flue is a manual `npm run proxy` plus `npx flue dev` path. It is not managed
by `makakoo agent start`.

## Fast diagnosis

| Symptom | Action |
|---|---|
| DSH dependencies missing | `cd "$MAKAKOO_HOME/agents-dsh/<slot>" && npm install` |
| Runner missing | Recreate the slot from its AgentSpec or restore the archive |
| Runtime metadata unavailable | Start the slot, then retry `health` or `prompt` |
| Explicit cloud provider rejected | Use `switchailocal/<model>` or an unprefixed switchAILocal model id |
| Slot exists already | Inspect with `makakoo agent show`; destroy only with user approval, then recreate |
| Channel declared but no messages arrive | Expected in DSH V1; no channel listener is connected |
| Flue slot refuses supervised start | Run its generated proxy/dev scripts manually |

Deep reference: `docs/user-manual/agent.md`. Runnable walkthrough:
`docs/walkthroughs/dsh-agent-runtime.md`.
