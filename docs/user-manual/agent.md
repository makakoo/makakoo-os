# `makakoo agent` — CLI reference

The `agent` subcommand group manages multi-bot subagents. Each
subagent ("slot") has its own persona, tools, paths, and one or
more chat-transport attachments (Telegram, Slack, …).

## Slot lifecycle

| Command | Purpose |
|---|---|
| `makakoo agent create <slot> [flags|--from-toml]` | Create a new slot. Pre-validates credentials before writing. |
| `makakoo agent list [--json]` | Enumerate every slot in `~/MAKAKOO/config/agents/*.toml`. |
| `makakoo agent show <slot> [--json]` | Print the resolved TOML with all secrets redacted. |
| `makakoo agent validate <slot>` | Run per-transport credential verifiers WITHOUT starting the agent. |
| `makakoo agent inventory [--json]` | List legacy `agent-*` plugins with their migration status. |
| `makakoo agent migrate-harveychat` | One-shot: migrate the legacy Olibia bot config to the `harveychat` slot. Idempotent. |
| `makakoo agent start <slot>` | Run the slot's `[entrypoint].start` (Phase 1 plugin start). |
| `makakoo agent stop <slot>` | Stop the slot's process pair. |
| `makakoo agent status <slot>` | Per-transport.id status: connection state, last_inbound, errors_1h, queue_depth. |
| `makakoo agent health <slot>` | Run the slot's health hook (exit 0 = up). |

## `agent create` modes

Three mutually exclusive modes:

### Single-Telegram

```sh
makakoo agent create career \
  --name "Career Manager" \
  --persona "Tracks job leads. Drafts replies; never auto-sends." \
  --allowed-paths "~/CV/,~/MAKAKOO/data/career/" \
  --tools "brain_search,write_file,linkedin,gmail" \
  --telegram-token '<bot-token>' \
  --telegram-allowed "746496145"
```

### Single-Slack

```sh
makakoo agent create alerts \
  --name "Alerts" \
  --slack-bot-token 'xoxb-…' \
  --slack-app-token 'xapp-…' \
  --slack-team T0123ABCD \
  --slack-allowed "U0123ABCD"
```

### Multi-transport (any combo)

Build a TOML by hand and load it:

```sh
makakoo agent create secretary --from-toml ~/secretary.toml
```

`--from-toml` lets you wire any number of `[[transport]]` blocks
in any combination. The CLI validates the file (schema + per-
transport credential check) before copying it into the registry.

`--from-toml` is mutually exclusive with `--telegram-token` and
`--slack-bot-token`. The CLI's `--allowed-paths`, `--forbidden-paths`,
`--tools`, `--persona`, `--name` flags override the source file
when explicitly passed.

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

See `docs/walkthroughs/multi-transport-subagents.md` for an
end-to-end walkthrough; `docs/troubleshooting/agents.md` for
common failure modes; `docs/roadmap/adapters.md` for follow-on
transports.
