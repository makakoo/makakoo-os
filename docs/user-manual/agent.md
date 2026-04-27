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

**Starter templates** for the most common archetypes live at
[`templates/agents/`](../../templates/agents/) — copy one,
replace the `<PLACEHOLDER>` fields, then run
`makakoo agent create <slot> --from-toml <copy>.toml`. The gallery
ships 11 archetypes in 3 tiers:

- **Tier 1 (highest payback):** secretary-freelance, invoice-chaser,
  expense-receipts, meeting-prep, lead-qualifier
- **Tier 2 (situational):** client-boundary-bouncer, subscription-watch,
  career-manager, support-inbox
- **Tier 3 (narrow):** alerts-bot, community-bot

Tiering and curation methodology are documented in
[`templates/agents/README.md`](../../templates/agents/README.md).

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
