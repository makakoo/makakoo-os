# Create and run a supervised agent with DeepSeek Harness

**Time:** about 10 minutes. **Platforms:** macOS and Linux for background
supervision. **Prerequisites:** Makakoo OS, Node.js 22.9 or newer,
`makakoo-mcp` on `PATH`, and switchAILocal listening on
`http://127.0.0.1:18080/v1`.

This walkthrough creates a local research agent, starts it under the OS
service manager, and sends two turns through one durable session.

## 1. Check prerequisites

```sh
makakoo --version
node --version
makakoo agent --help
```

Node must report `v22.9` or newer. If switchAILocal requires authentication,
export `AIL_API_KEY` before starting the slot. The generated runner maps that
value to the DSH provider automatically.

## 2. Write a local-only AgentSpec

```sh
mkdir -p "$MAKAKOO_HOME/tmp"
cat > "$MAKAKOO_HOME/tmp/local-researcher.yaml" <<'YAML'
name: local-researcher
description: Researches the local Makakoo Brain with a narrow tool set
model: switchailocal/ail-compound
instructions: |
  Answer from the Makakoo Brain. Cite the relevant note or journal date.
  State plainly when the available evidence does not answer the question.
tools:
  - brain_search
channels: []
triggers: []
scope:
  allowed_paths:
    - "~/MAKAKOO/data/Brain/**"
  forbidden_paths:
    - "~/.ssh/**"
    - "~/.aws/**"
YAML

makakoo agent validate-spec "$MAKAKOO_HOME/tmp/local-researcher.yaml"
```

Expected result:

```text
[OK]   local-researcher
```

Read the file before creating anything. The `tools` list is the MCP
whitelist. `tools: []` means no model-facing tools, not an implicit baseline.
Source checkouts also ship the same starter at
`examples/agents/local-researcher.yaml`.

## 3. Compile the runtime

```sh
makakoo agent create --specs "$MAKAKOO_HOME/tmp/local-researcher.yaml"
```
<!-- verify: skip reason="requires live switchAILocal credential preflight; validate-spec is executed above" -->

Makakoo writes:

- `$MAKAKOO_HOME/config/agents/local-researcher.toml`, the policy record;
- `$MAKAKOO_HOME/agents-dsh/local-researcher/`, the generated Node runtime.

The project pins every direct DSH package to `0.1.1-rc.2`. It exposes no DSH
shell or native filesystem tools. `makakoo-mcp` is the only model-facing tool
source and receives `MAKAKOO_AGENT_SLOT=local-researcher` for server-side
filtering.

## 4. Install the pinned dependencies

```sh
cd "$MAKAKOO_HOME/agents-dsh/local-researcher"
npm install
npm run check
makakoo agent validate local-researcher
```
<!-- verify: skip reason="downloads pinned npm dependencies and requires Node.js 22.9 or newer" -->

`validate` refuses to start an incomplete generated project. An explicit
model such as `anthropic/...` is also rejected because the DSH runtime routes
through switchAILocal only.

## 5. Start and verify

```sh
makakoo agent start local-researcher
makakoo agent status local-researcher
makakoo agent health local-researcher
```
<!-- verify: skip reason="registers a real per-user service and requires a live switchAILocal gateway" -->

`start` registers a LaunchAgent on macOS or a systemd-user unit on Linux. The
runtime binds to `127.0.0.1` on an ephemeral port and stores a per-start
mode-0600 bearer token inside the generated project. Use the CLI instead of
reading or copying that token.

## 6. Continue a durable session

```sh
makakoo agent prompt local-researcher \
  "Summarize the latest decisions in my Brain" --session daily

makakoo agent prompt local-researcher \
  "Which decision is still unresolved?" --session daily
```
<!-- verify: skip reason="requires the supervised runtime and local switchAILocal service started above" -->

The same session id continues the JSONL-backed conversation. Pick a new id
for an isolated conversation. Default admission limits are 128 durable
sessions, 1,000 turns per session, 512 MiB across `.sessions/`, and 128 KiB per
prompt; the generated `.env.example` exposes each limit.

## 7. Stop or remove it

Stop without deleting:

```sh
makakoo agent stop local-researcher
```
<!-- verify: skip reason="mutates a real per-user service created by the skipped start step" -->

Destroy and archive the managed files:

```sh
makakoo agent destroy local-researcher
```
<!-- verify: skip reason="archives and deletes the real agent slot created by earlier operator steps" -->

`destroy` requires confirmation unless `--yes` is explicit. It first proves
the service stopped, removes the LaunchAgent/systemd-user definition so the
slot cannot resurrect on login, then archives the slot and generated runtime under
`$MAKAKOO_HOME/archive/agents/`. Secret revocation is a separate opt-in.

## Current channel boundary

DSH V1 is a supervised local agent loop. AgentSpec accepts channel and trigger
declarations, but it does not start Telegram, Slack, Discord, WhatsApp, email,
voice, webhook, or cron listeners yet. Creation prints a warning when such
declarations are present.

For the operator-only legacy Flue renderer:

```sh
MAKAKOO_AGENT_ENGINE=flue makakoo agent create --specs ./agent.yaml
```
<!-- verify: skip reason="operator compatibility example requires a caller-provided AgentSpec" -->

That path is manually run with `npm run proxy` plus `npx flue dev`; Makakoo's
slot supervisor does not launch it.

## Troubleshooting

| Error | Fix |
|---|---|
| `DeepSeek Harness dependencies missing` | Run `npm install` in the generated project. |
| `DeepSeek Harness runner missing` | Recreate from the AgentSpec or restore the archived project. |
| `runtime metadata unavailable ... is the slot started?` | Run `makakoo agent start <slot>`, then retry. |
| `routes through switchAILocal` | Change the spec model to `switchailocal/<model>`. |
| `agent runtime output ... already exists` | Inspect the existing slot. Do not overwrite it blindly. |

Full reference: [`../user-manual/agent.md`](../user-manual/agent.md).
