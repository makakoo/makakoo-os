# `makakoo agent` — CLI reference

The `agent` subcommand group manages scoped agent slots and legacy agent
plugin lifecycle hooks. New slots compile an AgentSpec into a supervised
DeepSeek Harness (DSH) runtime. Each slot owns its persona, model, tool
whitelist, and path scope.


Important distinction: `makakoo infect` is for local agentic hosts that can
load Makakoo instructions and MCP tools. Telegram, Slack, Discord, WhatsApp,
voice, email, and web are transports, not infectable hosts. AgentSpec declares
them; the supervisor starts the `telegram` ones and preserves the rest as
declarations only.

## Slot lifecycle

| Command | Purpose |
|---|---|
| `makakoo agent create [--specs <PATH>]` | Create a slot and compile its AgentSpec into a pinned DeepSeek Harness runtime project. `<SLOT>` is optional with `--specs`; the spec's `name` becomes the slot id. |
| `makakoo agent list [--json]` | Enumerate every slot in `~/MAKAKOO/config/agents/*.toml`. |
| `makakoo agent show <slot> [--json]` | Print the resolved TOML with all secrets redacted. |
| `makakoo agent validate <slot>` | Validate the declared runtime, generated files, installed dependencies, transport configuration, and resolvable secret references without starting the agent or calling a channel. |
| `makakoo agent validate-spec <PATH>` | Parse and validate one or more spec files (file, directory, or `.`) without creating anything. Exits 0 on all-pass, 1 on any failure. |
| `makakoo agent inventory [--json]` | List legacy `agent-*` plugins with their migration status. |
| `makakoo agent migrate-harveychat` | One-shot: migrate the legacy Olibia bot config to the `harveychat` slot. Idempotent. |
| `makakoo agent start <slot>` | Preflight the generated project, then hand the DSH runtime to launchd (macOS) / systemd-user (Linux). |
| `makakoo agent prompt <slot> "<text>" [--session <id>]` | Run one turn through the authenticated loopback API. Reuse a session id for continuation. |
| `makakoo agent stop <slot>` | Stop the slot supervisor and runtime. |
| `makakoo agent restart <slot>` | Stop + start through the per-slot supervisor. A failed stop blocks the restart. |
| `makakoo agent status <slot>` | Show the supervisor and runtime process state. Legacy gateway slots also show per-transport status. |
| `makakoo agent health <slot>` | Probe the running DSH loopback endpoint (exit 0 = healthy). Legacy slots use supervisor status; legacy plugin names use their health hook. |
| `makakoo agent health <slot> --probe` | Also ask the slot's LLM route whether it can serve a multi-turn tool call. See [Capability probe](#capability-probe). |
| `makakoo agent destroy <slot>` | Interactive teardown. Stops the supervisor and archives TOML, data, and a managed generated runtime under `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/`. Custom runtime paths are preserved and reported. `--yes` skips the prompt. `--revoke-secrets` clears detected keyring entries (off by default). |
| `makakoo agent audit [--last N] [--kind K] [--json]` | Tail the per-machine audit log. Filter by `--kind scope_tool / webhook_invalid_signature / rate_limit / fault_test / ...`. |
| `makakoo agent test-faults [--scenario S] [--json]` | Run the fault-injection scenario suite. Gated behind `MAKAKOO_DEV_FAULTS=1`. |

## Runtime support matrix

| Surface | DSH V1 status | Notes |
|---|---|---|
| Local prompt API | Supported | Authenticated loopback endpoint, normally called through `makakoo agent prompt`. |
| Durable sessions | Supported, bounded | Reuse `--session <id>` for continuation. Defaults: 128 sessions, 1,000 turns/session, 512 MiB admission ceiling. |
| Makakoo MCP tools | Supported | Server-side filtered by the slot tool whitelist. |
| Background supervision | macOS + Linux | launchd or systemd-user. |
| Telegram / Slack / Discord | Declaration only | No DSH listener or outbound adapter yet. |
| WhatsApp / email / voice / web | Declaration or legacy metadata only | No DSH listener or outbound adapter yet. |
| Cron / webhook triggers | Declaration only | No DSH scheduler or trigger ingress yet. |

## `agent create` modes

Two modes — spec-driven (preferred) or quick-start (ergonomic).

### From a spec (preferred)

Write a YAML or TOML spec that declares the agent's cognitive core,
channels, triggers, and scope. See [`docs/agents/spec.md`](../agents/spec.md)
for the full schema and [`examples/agents/`](../../examples/agents/)
for starters.

```sh
# One agent from one spec file:
makakoo agent create --specs ./weather-bot.yaml

# N agents from a directory of specs (sorted, atomic):
makakoo agent create --specs ./agents/

# Scan the current folder:
makakoo agent create --specs .

# Validate without creating:
makakoo agent validate-spec ./weather-bot.yaml
```

`<SLOT>` is optional with `--specs` — the spec's `name` becomes the
slot id. For directory mode, each spec produces one agent.

### Inline Telegram / Slack shorthand

For ad-hoc metadata creation without writing a spec file. The CLI generates a
synthetic spec from the flags, then scaffolds the same runtime. **These flags
do not make Telegram or Slack delivery operational in DSH V1.** Use a normal
spec without channels for a local prompt-driven agent.

```sh
# Single-Telegram quick-start:
makakoo agent create career \
  --name "Career Manager" \
  --persona "Tracks job leads. Drafts replies; never auto-sends." \
  --allowed-paths "~/CV/,~/MAKAKOO/data/career/" \
  --tools "brain_search,write_file,linkedin,gmail" \
  --telegram-token '<bot-token>' \
  --telegram-allowed "746496145"

# Single-Slack quick-start:
makakoo agent create alerts \
  --slack-bot-token 'xoxb-…' \
  --slack-app-token 'xapp-…' \
  --slack-team T0123ABCD \
  --slack-allowed "U0123ABCD"
```

The quick-start flags are mutually exclusive with `--specs`. They
generate a synthetic spec with a single Telegram or Slack channel;
the agent's env var names are the well-known defaults
(`TELEGRAM_BOT_TOKEN`, `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`,
`SLACK_TEAM_ID`, plus the `*_WEBHOOK_SECRET_TOKEN` for Telegram and
`SLACK_SIGNING_SECRET` for Slack). The `.env` file in the generated
runtime project is pre-populated with the inline tokens. DSH V1 reserves
them for the Makakoo channel-adapter slice; DSH does not consume channel
credentials directly.

## Agent runtime: DeepSeek Harness

`makakoo agent create` keeps AgentSpec and AgentSlot as Makakoo's control
plane, then compiles a Node.js project under
`$MAKAKOO_HOME/agents-dsh/<slot>/`. DeepSeek Harness owns the model/tool
loop and durable JSONL sessions. The generated runtime:

- routes model calls through switchAILocal's OpenAI-compatible endpoint;
- mounts `makakoo-mcp` as its only model-facing tool source;
- passes `MAKAKOO_AGENT_SLOT` to MCP for server-side tool filtering;
- mounts no DSH shell or filesystem tools;
- exposes only an authenticated API on `127.0.0.1`;
- stores runtime metadata and a mode-0600 bearer token inside the project.
- limits cross-session execution to four concurrent turns by default while
  serializing turns within each session;
- admission-bounds durable state to 128 sessions, 1,000 turns per session,
  512 MiB total session storage, and 128 KiB per prompt by default;
- watches its supervisor parent and shuts down if orphaned.

Prerequisites: Node.js 22.9 or newer, a running switchAILocal endpoint, and
`makakoo-mcp` on `PATH` (or set `MAKAKOO_MCP_BIN`).

`agent create` installs the generated project's Node dependencies for you,
so a fresh slot is startable immediately:

```sh
makakoo agent validate weather-bot
makakoo agent start weather-bot
makakoo agent prompt weather-bot "Check current priorities" --session daily
makakoo agent status weather-bot
```

Pass `--no-install` (or export `MAKAKOO_SKIP_DEPS_INSTALL=1` for scripted or
offline provisioning) to skip it and run the install yourself:

```sh
cd "$MAKAKOO_HOME/agents-dsh/weather-bot"
npm install
npm run check
```

A failed install never rolls back the create — the slot and the generated
project are already durable, and the command prints the exact
`cd … && npm install` to finish by hand.

Direct DSH runtime packages are pinned to `0.1.1-rc.2`; the full DSH CLI
bundle is deliberately excluded because it multiplies install size and is not
part of the runtime contract. An upgrade is an integration decision, not
automatic semver drift. `MAKAKOO_AGENT_ENGINE=flue` keeps
the old Flue renderer available for operators, but Flue projects remain a
manual `npm run proxy` + `npm run dev` path and are not launched by the slot
supervisor.

DSH model calls always use switchAILocal. Specs may use
`switchailocal/<model>` (recommended) or an unprefixed switchAILocal model id;
an explicit different provider prefix is rejected during creation and start.
The runner uses `DEEPSEEK_API_KEY`, falls back to `AIL_API_KEY`, then uses a
local placeholder for gateways that do not require authentication.

### Capability probe

Plain `agent health` answers one question: is the runtime process up. That is
not the question that breaks agents. A slot whose process is perfectly healthy
still fails every real request if its model route can no longer accept a
conversation that already contains a tool call — which is what every agent turn
after the first one looks like.

`makakoo agent health <slot> --probe` asks that question directly:

```sh
makakoo agent health weather-bot --probe
```

It builds a synthetic request containing an assistant `tool_call` and its
`tool` result and sends it **straight to the slot's provider endpoint** —
not through the agent runtime. That makes it deterministic (it does not
depend on the model choosing to call a tool), free (no agent turn is
consumed), and usable on a stopped slot.

The endpoint is resolved from the slot's effective model specifier, so the
probe is provider-agnostic:

| Model specifier | Endpoint | Protocol |
|---|---|---|
| `ail-compound` (bare) or `switchailocal/<model>` | `http://127.0.0.1:18080/v1` | OpenAI-compatible |
| `ollama/<model>` | `http://127.0.0.1:11434/v1` | OpenAI-compatible |
| `openai/<model>` | `https://api.openai.com/v1` | OpenAI-compatible |
| `anthropic/<model>` | `https://api.anthropic.com/v1` | Anthropic Messages |

On refusal the upstream status and message are printed verbatim, because the
provider's own words identify the cause better than any paraphrase:

```
error: weather-bot: route cannot serve multi-turn tool calls — ollama smollm2:135m returned HTTP 400
upstream: {"error":{"message":"registry.ollama.ai/library/smollm2:135m does not support tools", …}}
```

Exit codes: `0` route is good (and, if the slot is running, so is it), `1`
liveness or capability failed, `2` the target is a legacy slot or a plugin
and has no LLM route to probe. Under `--probe` a liveness failure is reported
as a warning rather than aborting, so a stopped slot still gets its capability
answer. `--probe` only ever adds a reason to fail, never removes one.

### Telegram channels

`agent start` hosts a slot's enabled `telegram` transports in the supervisor,
next to the runtime. Each one long-polls `getUpdates`; an allowlisted message
becomes a `POST /v1/run` on the slot's loopback API under a stable per-chat
session id (`tg:<chat_id>`), and the answer goes back with `sendMessage`. The
bridge only ever replies to an inbound message.

| Behaviour | Detail |
|---|---|
| Allowlist | Matched against the **sender** id. The union of `allowed_users` and `config.allowed_chat_ids`. Empty = deny-all, and such a transport is not started at all. |
| `allowed_group_ids` | **Ignored.** A group id is not a sender id. `agent start` says so rather than letting the ACL look active. |
| Session | Separated by transport, chat, forum topic, and — in a group — sender, so members do not share a memory. Preserved across a restart. |
| Concurrency | One worker per conversation; 4 *runs* in flight at once. Past 64 live conversations, or 8 queued messages in one, the sender is told the agent is busy rather than queued indefinitely. |
| Long replies | Split into Telegram-sized messages (measured in UTF-16 units, as Telegram counts them), capped at 8 with a truncation marker. |
| Runtime errors | Reported into the chat, not swallowed. |
| Bot token | From `secret_env` / `secret_ref` only — never the spec or the slot TOML. Under launchd the service sources `~/.env`, so a token written there reaches the supervisor. |
| Bad token | Fails `agent start` with the reason. An *unreachable* Telegram is only a warning; the poll loop retries. |
| Failure isolation | A transport that dies never touches the gateway child or the other transports. Its listener restarts on the gateway's budget, then stays down instead of retrying forever. |

Stopping the slot stops the transports with it; an in-flight answer is
abandoned rather than delaying shutdown.

### Cron triggers

A `cron` trigger wakes the agent with no human in the loop. The supervisor
hosts it next to the transports and stops it with them.

| Aspect | Behaviour |
|---|---|
| Schedule | Standard 5-field cron, **Sunday = 0** (as crontab), with a real IANA timezone. A bad expression or timezone fails `agent create`. |
| Session | `cron:<trigger_id>` — the schedule keeps its own history, separate from every chat and preserved across restarts. |
| Delivery | To the channel allowlist only. `deliver_to` narrows it; an unknown name fails the trigger instead of sending nowhere. |
| No channels | Valid. The answer is logged; the agent's output is its side effects. |
| Overrun | Ticks are skipped, never queued — no backlog that can never drain. |
| Missed tick | A tick late by more than 5 minutes (a sleeping laptop) is reported and dropped, not replayed. |
| Bad trigger | Skipped by name with a reason; the agent's other triggers and channels still start. |

### Everything else is still declaration-only

Slack, Discord, email, voice, and webhook declarations remain in AgentSpec and
AgentSlot without a listener. `agent start` names every transport and trigger
it declines to start and why, so a slot never looks connected when it is not.

## Slot id rules

- ASCII alphanumeric + `-` + `_`.
- 1–64 characters.
- Must equal the TOML filename stem (`<slot_id>.toml`).
- The migrated Olibia bot's slot id is `harveychat` — NEVER
  `olibia`. "Olibia" is the display `name` only.

## Secret resolution

Per-transport secret slots accept three flat fields:

| Field | Source | Precedence |
|---|---|---|
| `secret_env`    | Process env var      | Highest |
| `secret_ref`    | `makakoo secret` keyring entry | Middle |
| `inline_secret_dev` | TOML literal     | Lowest (dev-only, logs WARN) |

For Slack (Socket Mode), the same triple applies to the app
token: `app_token_env` / `app_token_ref` / `inline_app_token_dev`.

## `agent status <slot>` output

```
local-researcher
  gateway:   alive   pid=12345     last_frame=never
  state=Running supervisor_pid=12344 restart_count=0
```

DSH V1 has no transport rows because channel listeners are not connected.
Existing legacy gateway slots may still show transport state,
`last_inbound`, `errors_1h`, and `queue_depth` rows.

## Identity propagation

For DSH slots, the AgentSpec `instructions` field is the generated system
prompt. Slot identity and tool authority are also propagated out-of-band via
`MAKAKOO_AGENT_SLOT`, then enforced by `makakoo-mcp`. Editing the generated
runner does not change the canonical AgentSpec or server-side tool scope.

Legacy channel gateways prepend an identity block such as:

> *"You are Olibia. Your slot id is harveychat. This message arrived
> via telegram. Your allowed tools are brain_search, write_file. Your
> allowed paths are ~/MAKAKOO/data/harveychat/."*

For that legacy block, empty allowed-tools renders as `(baseline)` (when `inherit_baseline =
true`) or `(none — least-privilege default)`. Empty allowed-paths
always renders as `(none — least-privilege default)`.

## Cross-subsystem awareness

| Subsystem | How agent-id flows |
|---|---|
| **MCP HTTP** | Validated `X-Makakoo-Agent-Id` header is covered by the peer request signature → `tokio::task_local AGENT_ID` → `dispatch::current_agent_id()` |
| **MCP stdio** | `MAKAKOO_AGENT_SLOT` env var read once at startup → same task-local |
| **User grants** | New grants populate `bound_to_agent` from `current_agent_id()`; `visible_to(caller)` returns false unless the caller matches |
| **Brain journal** | Lines from agents get `[agent:<slot_id>]` prefix (Phase 4 dogfood) |

Agent-bound signed HTTP clients automatically copy `MAKAKOO_AGENT_SLOT` into
`X-Makakoo-Agent-Id` and sign that attribution. An authenticated HTTP peer
without an agent binding deliberately makes a peer-administrator call and
receives the unscoped peer surface. Changing, adding, or removing a signed
attribution invalidates the request signature; only trusted peer credentials
can make the explicit administrator call.

## Files & paths

| What | Where |
|---|---|
| Slot TOMLs | `~/MAKAKOO/config/agents/<slot>.toml` |
| Generated DSH runtime | `~/MAKAKOO/agents-dsh/<slot>/` |
| DSH sessions | `~/MAKAKOO/agents-dsh/<slot>/.sessions/` |
| Runtime metadata + token | `runtime.json` + per-start token inside the generated project (0600) |
| Per-agent state dir | `~/MAKAKOO/data/agents/<slot>/` |
| Legacy gateway conversation DB | `~/MAKAKOO/data/agents/<slot>/conversations.db` |
| Legacy gateway IPC socket | `~/MAKAKOO/run/agents/<slot>/ipc.sock` (parent dir 0700) |
| LaunchAgent / systemd unit | `~/Library/LaunchAgents/com.makakoo.agent.<slot>.plist` or `~/.config/systemd/user/makakoo-agent-<slot>.service`; removed by `agent stop` |
| User grants | `~/MAKAKOO/config/user_grants.json` (shared, with `bound_to_agent` field) |

## Audit log + redaction

`makakoo agent audit` reads the JSONL log at
`$MAKAKOO_HOME/data/audit/agents.jsonl`. Locked behavior (Q14):

- 100 MB per file, 1 GB total cap, file mode `0600`.
- Secrets, tokens, raw bodies are **never logged** (redacted at the
  writer). Actor + target identifiers (emails, phone numbers, Slack
  user ids) are logged in full — forensics need them.
- Filter via `--kind <name>`. Supported kinds: `scope_tool`,
  `scope_path`, `secret_resolve`, `grant_issue`, `grant_revoke`,
  `slot_create`, `slot_start`, `slot_stop`, `slot_destroy`,
  `transport_verify`, `rate_limit`, `fault_test`, `gateway_crash`,
  `webhook_invalid_signature`, `webhook_bad_origin`,
  `webhook_bad_cookie`, `webhook_bad_request`.

## Fault injection (`agent test-faults`)

Gated behind `MAKAKOO_DEV_FAULTS=1`. Runs the 9 locked Q11
scenarios using mock adapters — no real transport credentials, no
network. Surfaces a pass/fail report; exits non-zero on any FAIL.

```sh
MAKAKOO_DEV_FAULTS=1 makakoo agent test-faults
MAKAKOO_DEV_FAULTS=1 makakoo agent test-faults --scenario rate-limit-burst
```

Start with `docs/walkthroughs/dsh-agent-runtime.md`. The older
multi-transport and per-channel pages document the legacy gateway contract;
they are not DSH V1 deployment guides. Failure modes:
`docs/troubleshooting/agents.md`. Locked HTTP-server contract
(signatures, status codes, redaction): `docs/specs/http-server-security.md`.
