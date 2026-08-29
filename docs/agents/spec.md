# Agent Spec — declarative agent definition

The agent spec is the source of truth for every agent on Makakoo OS. A
spec is a YAML (preferred) or TOML file that declares **what** an agent
is — its cognitive core, communications interfaces, and trigger sources.
Makakoo compiles the spec into an AgentSlot plus a pinned DeepSeek Harness
runtime project. The slot remains the policy authority; generated runtime
files are replaceable build artifacts.

```bash
makakoo agent create --specs ./weather-bot.yaml   # one agent
makakoo agent create --specs ./agents/            # N agents, one per spec
makakoo agent create --specs .                    # current folder
makakoo agent validate-spec ./weather-bot.yaml    # validate without creating
```

## Conceptual model

The spec separates three concerns:

- **AGENT** — the cognitive core: `name`, `description`, `model`,
  `instructions`, `tools`, `scope`
- **CHANNELS** — communications interfaces set ON the agent
- **TRIGGERS** — when the agent starts without a user message

An agent with only cron triggers and zero channels is valid (autonomous
daemon, no UI).

## Schema (YAML)

```yaml
# Identity
name: weather-bot                    # required, regex ^[a-z0-9][a-z0-9-]{0,62}$
description: "Monitor weather…"      # required, non-empty
model: switchailocal/ail-compound    # required, routed through switchAILocal

# Cognitive core
instructions: |                      # required, multi-line, markdown allowed
  You are a weather monitoring agent. Check forecasts every 6 hours,
  alert on severe conditions via the configured channels.
tools:                               # required, list of mcp__harvey__* names
  - brain_search
  - write_file

# Communications interfaces (zero or many). A telegram channel is
# started by the supervisor; the other kinds are declaration-only.
channels:
  - kind: telegram
    token_env: TELEGRAM_BOT_TOKEN
    allowed_users: ["123456"]

  - kind: slack
    token_env: SLACK_BOT_TOKEN
    app_token_env: SLACK_APP_TOKEN
    team_id_env: SLACK_TEAM_ID
    allowed_users: ["U0123"]

  - kind: discord
    token_env: DISCORD_BOT_TOKEN
    allowed_users: ["987654"]

  - kind: webhook
    path: /hooks/weather
    secret_env: WEATHER_WEBHOOK_SECRET

  - kind: email
    smtp_host: smtp.example.com
    imap_host: imap.example.com
    secret_env: EMAIL_CREDS

  - kind: voice
    twilio_account_sid_env: TWILIO_ACCOUNT_SID
    secret_env: TWILIO_AUTH_TOKEN

# Trigger sources (zero or many). Cron triggers are scheduled by the
# supervisor; webhook triggers are declaration-only.
triggers:
  - kind: cron                       # standard 5-field cron expression
    schedule: "0 */6 * * *"          # required
    timezone: "UTC"                  # optional, default "UTC"
    prompt: "Run the six-hourly check."   # optional; default wake message
    deliver_to: []                   # optional; transport IDs, empty = every channel

  - kind: webhook
    path: /triggers/manual
    secret_env: MANUAL_TRIGGER_SECRET

# Filesystem read/write boundaries
scope:
  allowed_paths:                    # filesystem prefixes/globs; absolute and ~/ paths supported
    - "~/MAKAKOO/data/weather/**"
  forbidden_paths:                  # globs, always denied
    - "~/.ssh/**"
    - "~/.aws/**"
```

## Field reference

### `name` (required, string)
Agent identity. Regex: `^[a-z0-9][a-z0-9-]{0,62}$`. It becomes the
`slot_id` in the slot TOML and the DSH project directory name.

### `description` (required, string)
Human-facing one-liner. Shown in `makakoo agent list`. Non-empty after
trim.

### `model` (required, string)
Model route passed through switchAILocal to DSH (for example
`switchailocal/ail-compound`). An unprefixed switchAILocal model id is also
accepted. An explicit different provider prefix is rejected during creation
and start; the `switchailocal/` prefix is removed before the OpenAI-compatible
request. Non-empty after trim.

### `instructions` (required, string)
System prompt / persona. Multi-line string, markdown allowed. No length
limit but be concise.

### `tools` (required, list of strings)
Whitelist of Makakoo MCP tool names this agent may invoke. Raw names such as
`brain_search` and model-facing names such as `mcp__harvey__brain_search` are
treated as the same tool at the server-side enforcement boundary. Each name
must match `^[a-z][a-z0-9_]*$` (which permits the double underscores in
`mcp__harvey__*`). `tools: []` is valid and exposes no model-facing tools;
AgentSpec-generated slots never inherit an implicit baseline.

**Names are validated at create and validate time.** A name that no
registered handler serves is rejected before any slot is written — a
well-formed name for a nonexistent tool used to produce a slot that looked
correct and silently could not do the job, because the server filters the
registry down to the intersection. Run `makakoo-mcp --list-tools` for the
authoritative list; any of those names is valid here, as are the per-machine
`pattern_*` tools discovered from `$MAKAKOO_HOME/plugins/pattern-*/`.

The example above previously listed `web_search`, which has never been a
registered tool.

### `channels` (optional, list)
Communications interfaces. Zero or many. See "Channel kinds" below.

### `triggers` (optional, list)
Trigger sources. Zero or many. See "Trigger kinds" below.

### `scope` (required, object)
Filesystem read/write boundaries. Two fields:
- `allowed_paths` — filesystem prefixes/globs; absolute paths and `~/...` are supported
- `forbidden_paths` — globs, always denied (overrides `allowed_paths`)

V1: overlap detection is exact-string match. Proper glob overlap
detection requires the `globset` crate and is deferred to V2.

## Runtime and model routing

The DSH renderer does not auto-select a cloud provider. Autonomous agent calls
go through the fixed switchAILocal endpoint at `http://127.0.0.1:18080/v1`; the model part after
the first `/` is sent as the OpenAI-compatible model id. This keeps one key,
one routing layer, and one audit surface.

Generated projects pin their direct DSH runtime packages to `0.1.1-rc.2`,
exclude the full DSH CLI bundle, persist sessions as compressed JSONL, apply
semantic checkpoints and compaction, and expose only Makakoo MCP tools. Native
DSH bash and filesystem tools are deliberately not mounted.

The operator-only environment variable `MAKAKOO_AGENT_ENGINE=flue` selects the
legacy Flue renderer. Only that compatibility path runs local/cloud provider
discovery and emits `src/app.ts`.

## Channels and triggers

The schema accepts Telegram, Slack, Discord, webhook, email, and voice channel
declarations plus cron and webhook triggers. They are preserved in AgentSpec,
compiled into AgentSlot transport metadata where a slot representation exists,
and their environment-variable names are emitted into `.env.example`.

**Telegram channels are live.** `makakoo agent start` hosts them in the
supervisor: the adapter long-polls `getUpdates`, refuses any sender outside the
allowlist, and calls the runtime's authenticated `/v1/run` with a stable
session id, so a conversation keeps its context across restarts. That id
separates transports, chats, forum topics, and — in a group — senders, so two
people in one group do not share the agent's memory. Replies go back with `sendMessage`, split into
Telegram-sized messages; a runtime failure is reported into the chat rather
than swallowed. The bridge only ever replies to an inbound message — nothing in
it can send unsolicited output.

Two properties are worth knowing before you configure one:

- **An empty allowlist is deny-all**, and a transport whose allowlist is empty
  is not started at all — it could do nothing but log refusals. Set
  `allowed_users` (or `config.allowed_chat_ids` in the slot TOML).
- **The bot token never lives in the spec or the slot TOML.** It is read from
  the env var named by `token_env`, or from `makakoo secret`. Under launchd the
  service sources `~/.env`, so a token written there reaches the supervisor.

`agent start` verifies the token before handing off to the service manager: a
token Telegram rejects fails the command, while an unreachable API is only a
warning (the poll loop retries). A transport that cannot start never blocks the
runtime API — `makakoo agent prompt` keeps working either way.

### Cron triggers

A `cron` trigger is scheduled by the supervisor alongside the transports. On
each tick it sends `prompt` to the runtime under the stable session id
`cron:<trigger_id>`, so the schedule keeps its own conversation history —
separate from every chat, and preserved across restarts.

`schedule` is standard 5-field cron (`min hour dom mon dow`) with **Sunday as
0** (7 also means Sunday), matching crontab. `timezone` is any IANA name and is
validated against the real database — a typo fails `agent create` rather than
silently running in UTC.

Delivery goes to the transport's configured `allowed_chat_ids` — real chat ids
— falling back to the sender allowlist only for one-to-one bots, where Telegram
makes a user id and a chat id the same number. `deliver_to` narrows this to named
transports **by id** (`telegram-0`, ... — see the slot TOML), not by kind. An
id that matches nothing is reported at start and the trigger still runs,
delivering to whatever resolved: a stale entry must not cancel the agent's
work. An agent with **no** channels is a valid headless
shape — the answer is logged and the agent's real output is its side effects.

Ticks are skipped, never queued. A run that overruns its own period drops the
ticks it covered instead of building a backlog, and a tick missed because the
machine slept is reported and dropped rather than replayed — a 08:00 brief
delivered at 16:00 is worse than no brief.

**The other channel kinds and webhook triggers remain declaration-only.** They
are preserved in the spec and compiled into slot metadata, but nothing starts a
listener for them.

Supported declaration shapes remain:

- `telegram`: `token_env`, optional `allowed_users`
- `slack`: `token_env`, `app_token_env`, `team_id_env`, optional `allowed_users`
- `discord`: `token_env`, optional `allowed_users`
- `webhook`: `path`, `secret_env`
- `email`: `smtp_host`, `imap_host`, `secret_env`
- `voice`: `twilio_account_sid_env`, `secret_env`
- cron trigger: five-field `schedule`, optional IANA `timezone`
- webhook trigger: `path`, `secret_env`

The operator-selected Flue compatibility renderer still generates its legacy
channel and trigger modules. That does not make them part of the default DSH
runtime or Makakoo supervisor contract.

## Validation rules

- `name` must match `^[a-z0-9][a-z0-9-]{0,62}$`
- `description` and `model` must be non-empty after trim
- Each `tools[]` entry must match `^[a-z][a-z0-9_]*$` (allows `mcp__harvey__*`)
- Each `channels[].token_env` / `secret_env` / `app_token_env` / `team_id_env`
  must be an uppercase env var name (`^[A-Z][A-Z0-9_]*$`)
- Each `channels[].webhook.path` / `channels[].webhook.secret_env` /
  `triggers[].webhook.path` / `triggers[].webhook.secret_env` follows the
  same rules
- `triggers[].cron.schedule` must be a valid 5-field cron expression with
  each field in its proper range
- `triggers[].cron.timezone`, if non-empty, must look like an IANA
  timezone (`UTC`, `Europe/Berlin`, etc.)
- `scope.allowed_paths` and `scope.forbidden_paths` entries must be
  non-empty; exact-string overlap between the two lists is rejected

## V1 limitations

1. **Channel and trigger execution is not connected to DSH yet.** Declarations
   survive compilation, but no listener or scheduler calls `/v1/run`.
2. **Filesystem scope overlap detection is exact-string only.** Proper glob
   overlap still requires `globset`.
3. **The harness packages are a release candidate.** Direct dependencies are
   exact-pinned and covered by the Makakoo smoke suite, but the engine is not a
   stable upstream release.
4. **The runtime API is local-only.** It intentionally binds to `127.0.0.1`;
   remote ingress belongs behind a Makakoo-authenticated adapter, not an exposed
   bearer endpoint.
5. **Flue is compatibility-only.** It is scaffolded only when
   `MAKAKOO_AGENT_ENGINE=flue` is explicit and remains a manually operated path.

## Examples

See `examples/agents/`:
- `local-researcher.yaml` — prompt-driven DSH agent with no channel/trigger declarations
- `weather-bot.yaml` — cron + telegram + tools + scope
- `scheduled-reporter.yaml` — cron only, no channels
- `webhook-worker.yaml` — webhook channel + webhook trigger

## See also

- `spec-migration.md` — "I had a slot TOML, now what?" migration guide
- `walkthroughs/dsh-agent-runtime.md` — end-to-end supervised runtime walkthrough
- `walkthroughs/flue-telegram-bot.md` — legacy Flue compatibility path
- **`makakoo agent init-spec <PATH>`** — interactive starter; asks the right questions, discovers providers, writes a correct spec. Use `--minimal` for a 10-line "hello world" spec.
- **`makakoo agent provider-set <provider> [model]`** / **`makakoo agent provider-get`** — project-level LLM default (stored at `$MAKAKOO_HOME/config/llm-default`). `init-spec` uses it as the default model choice.
