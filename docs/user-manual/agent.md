# `makakoo agent` — CLI reference

The `agent` subcommand group manages multi-bot subagents. Each
subagent ("slot") has its own persona, tools, paths, and one or
more chat-transport attachments (Telegram, Slack, …).


Important distinction: `makakoo infect` is for local agentic hosts that can
load Makakoo instructions and MCP tools. Telegram, Slack, Discord, WhatsApp,
voice, email, and web are transports. They do not get infected. They attach
to a scoped agent slot.

## Slot lifecycle

| Command | Purpose |
|---|---|
| `makakoo agent create [--specs <PATH>]` | Create a new slot from a spec (default) or from the Telegram/Slack quick-start flags. Always scaffolds a Flue (TypeScript) agent project. `<SLOT>` is optional with `--specs` (the spec's `name` becomes the slot id). |
| `makakoo agent list [--json]` | Enumerate every slot in `~/MAKAKOO/config/agents/*.toml`. |
| `makakoo agent show <slot> [--json]` | Print the resolved TOML with all secrets redacted. |
| `makakoo agent validate <slot>` | Run per-transport credential verifiers WITHOUT starting the agent. |
| `makakoo agent validate-spec <PATH>` | Parse and validate one or more spec files (file, directory, or `.`) without creating anything. Exits 0 on all-pass, 1 on any failure. |
| `makakoo agent inventory [--json]` | List legacy `agent-*` plugins with their migration status. |
| `makakoo agent migrate-harveychat` | One-shot: migrate the legacy Olibia bot config to the `harveychat` slot. Idempotent. |
| `makakoo agent start <slot>` | Hand the slot to launchd (macOS) / systemd-user (Linux). Supervisor + Python gateway come up. |
| `makakoo agent stop <slot>` | Stop the slot's process pair. |
| `makakoo agent restart <slot>` | Stop + start. v2-mega: graceful via the per-slot supervisor. |
| `makakoo agent status <slot>` | Per-transport.id status: connection state, last_inbound, errors_1h, queue_depth, RSS. |
| `makakoo agent health <slot>` | Run the slot's health hook (exit 0 = up). |
| `makakoo agent destroy <slot>` | Interactive teardown. Stops the supervisor, archives TOML + data dir under `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/`, lists detected secret refs. `--yes` skips the prompt. `--revoke-secrets` also clears the keyring entries (off by default). |
| `makakoo agent audit [--last N] [--kind K] [--json]` | Tail the per-machine audit log. Filter by `--kind scope_tool / webhook_invalid_signature / rate_limit / fault_test / ...`. |
| `makakoo agent test-faults [--scenario S] [--json]` | Run the fault-injection scenario suite. Gated behind `MAKAKOO_DEV_FAULTS=1`. |

## Supported transports (v2.0)

| Kind | Direction | Listener | Auth | Notes |
|---|---|---|---|---|
| `telegram` | inbound (long-poll) + outbound REST | Per-task `getUpdates` | bot token | Per-chat allowlist. Forum topics via `support_thread`. |
| `slack` | inbound (Socket Mode WS) + outbound REST | Per-task wss | bot + app token | `dm_only` default; `channels` allowlist when false. |
| `discord` | inbound (gateway WS) + outbound REST | Per-task wss | bot token | MESSAGE_CONTENT default OFF; `guild_ids` allowlist; intents auto-computed. |
| `whatsapp` | inbound (webhook) + outbound REST | Shared webhook router | access token + verify token + app secret | X-Hub-Signature-256; media → drop-reply. |
| `voice_twilio` | inbound (webhook) + TwiML response | Shared webhook router | account_sid + auth_token | HMAC-SHA1 signature; recording-callback URL embeds CallSid. |
| `email` | outbound SMTP (v2.0) + inbound IMAP IDLE (v2.1) | (v2.1) | OAuth2 / app password | Plain IMAP/SMTP rejected. |
| `web` | inbound + outbound WS | Shared WS upgrade | HMAC-SHA256 visitor cookie | Origin allowlist required in production. |

## `agent create` modes

Two modes — spec-driven (preferred) or quick-start (ergonomic).

### From a spec (preferred)

Write a YAML or TOML spec that declares the agent's cognitive core,
channels, triggers, and scope. See [`docs/agents/spec.md`](../agents/spec.md)
for the full schema and [`examples/agents/`](../../../examples/agents/)
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

### Quick-start (Telegram / Slack only)

For ad-hoc creation without writing a spec file. The CLI generates a
synthetic spec from the flags, then scaffolds the same way.

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
Flue project is pre-populated with the inline tokens so
`npx flue dev` works without a manual copy.

## Agent runtime: Flue (the only engine)

Every `makakoo agent create` scaffolds a runnable
[Flue](https://flueframework.com) (TypeScript) agent project. Makakoo
stays the **control plane** (identity, scope, secrets, registry — the
slot) and Flue becomes the **data plane** (the agent loop + every
channel + every trigger).

The two are bridged by a generated `mcp-proxy.mjs`, which re-exposes
the local `makakoo-mcp` stdio server over StreamableHTTP so the
agent's `connectMcpServer()` can consume every Makakoo tool as
`mcp__harvey__*`.

The `--runtime` flag (native / flue) was removed in Phase 3 of
SPRINT-FLUE-DEFAULT-AGENT-SPECS. Flue is now the only creation
engine. Native execution paths (`agent_lifecycle`,
`agent_destroy`, `agent_audit`) are preserved for re-running
existing native slots.


The Flue project layout is driven by the spec: one file per channel
under `src/channels/`, one per trigger under `src/triggers/`,
`package.json` deps pulled in based on what the spec declares. See
[`docs/agents/spec.md`](../agents/spec.md) for the full schema and
[`examples/agents/`](../../../examples/agents/) for working starters.

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
secretary
  gateway:   alive   pid=12345     last_frame=2s ago
  transport slack-main:     connected     last_inbound=3m ago    errors_1h=0  queue_depth=0
  transport telegram-main:  connected     last_inbound=8s ago    errors_1h=0  queue_depth=0
```

Per-transport states: `connected | reconnecting | failed`.
`errors_1h` is a sliding-window count (1 hour rolling). `queue_depth`
is the per-transport asyncio queue depth on the Python gateway side
(0 means LLM is keeping up).

## Identity propagation

A running slot's persona system prompt always includes:

> *"You are Olibia. Your slot id is harveychat. This message arrived
> via telegram. Your allowed tools are brain_search, write_file. Your
> allowed paths are ~/MAKAKOO/data/harveychat/."*

Empty allowed-tools renders as `(baseline)` (when `inherit_baseline =
true`) or `(none — least-privilege default)`. Empty allowed-paths
always renders as `(none — least-privilege default)`.

## Cross-subsystem awareness

| Subsystem | How agent-id flows |
|---|---|
| **MCP HTTP** | `X-Makakoo-Agent-Id` header → `tokio::task_local AGENT_ID` → `dispatch::current_agent_id()` available to every tool handler |
| **MCP stdio** | `MAKAKOO_AGENT_SLOT` env var read once at startup → same task-local |
| **User grants** | New grants populate `bound_to_agent` from `current_agent_id()`; `visible_to(caller)` returns false unless the caller matches |
| **Brain journal** | Lines from agents get `[agent:<slot_id>]` prefix (Phase 4 dogfood) |

## Files & paths

| What | Where |
|---|---|
| Slot TOMLs | `~/MAKAKOO/config/agents/<slot>.toml` |
| Per-agent state dir | `~/MAKAKOO/data/agents/<slot>/` |
| Per-agent conversation DB | `~/MAKAKOO/data/agents/<slot>/conversations.db` |
| IPC socket | `~/MAKAKOO/run/agents/<slot>/ipc.sock` (parent dir 0700) |
| LaunchAgent / systemd unit | `com.makakoo.agent.<slot>.plist` |
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

See `docs/walkthroughs/multi-transport-subagents.md` for an
end-to-end multi-transport walkthrough; per-transport recipes at
`discord-bot.md`, `whatsapp-business.md`, `voice-quickstart.md`,
`email-secretary.md`, `web-chat-demo.html`. Failure modes:
`docs/troubleshooting/agents.md`. Locked HTTP-server contract
(signatures, status codes, redaction): `docs/specs/http-server-security.md`.
