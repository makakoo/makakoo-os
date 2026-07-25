# Agent Spec — declarative agent definition

The agent spec is the source of truth for every agent on Makakoo OS. A
spec is a YAML (preferred) or TOML file that declares **what** an agent
is — its cognitive core, communications interfaces, and trigger sources.
Makakoo's scaffolder then renders the spec into a runnable Flue (TypeScript)
project + slot TOML.

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
model: anthropic/claude-sonnet-4-6   # required, non-empty

# Cognitive core
instructions: |                      # required, multi-line, markdown allowed
  You are a weather monitoring agent. Check forecasts every 6 hours,
  alert on severe conditions via the configured channels.
tools:                               # required, list of mcp__harvey__* names
  - brain_search
  - write_file
  - web_search

# Communications interfaces (zero or many)
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

  # V1: deferred — no first-party @flue/* adapter. See "Limitations" below.
  - kind: email
    smtp_host: smtp.example.com
    imap_host: imap.example.com
    secret_env: EMAIL_CREDS

  # V1: deferred — no first-party @flue/* adapter. See "Limitations" below.
  - kind: voice
    twilio_account_sid_env: TWILIO_ACCOUNT_SID
    secret_env: TWILIO_AUTH_TOKEN

# Trigger sources (zero or many)
triggers:
  - kind: cron                       # standard 5-field cron expression
    schedule: "0 */6 * * *"          # required
    timezone: "UTC"                  # optional, default "UTC"

  - kind: webhook
    path: /triggers/manual
    secret_env: MANUAL_TRIGGER_SECRET

# Filesystem read/write boundaries
scope:
  allowed_paths:                    # globs, must resolve under $MAKAKOO_HOME
    - "~/MAKAKOO/data/weather/**"
  forbidden_paths:                  # globs, always denied
    - "~/.ssh/**"
    - "~/.aws/**"
```

## Field reference

### `name` (required, string)
Agent identity. Regex: `^[a-z0-9][a-z0-9-]{0,62}$`. Matches Flue project
naming. Also becomes the `slot_id` in the slot TOML and the Flue
project directory name.

### `description` (required, string)
Human-facing one-liner. Shown in `makakoo agent list`. Non-empty after
trim.

### `model` (required, string)
Model identifier passed to the Flue runtime (e.g.
`anthropic/claude-sonnet-4-6`). Non-empty after trim.

### `instructions` (required, string)
System prompt / persona. Multi-line string, markdown allowed. No length
limit but be concise.

### `tools` (required, list of strings)
Whitelist of `mcp__harvey__*` tool names this agent may invoke.
Validated at create time against the registered MCP tool set. Each
name must match `^[a-z][a-z0-9_]*$` (allows `mcp__harvey__*`).

### `channels` (optional, list)
Communications interfaces. Zero or many. See "Channel kinds" below.

### `triggers` (optional, list)
Trigger sources. Zero or many. See "Trigger kinds" below.

### `scope` (required, object)
Filesystem read/write boundaries. Two fields:
- `allowed_paths` — globs, must resolve under `$MAKAKOO_HOME`
- `forbidden_paths` — globs, always denied (overrides `allowed_paths`)

V1: overlap detection is exact-string match. Proper glob overlap
detection requires the `globset` crate and is deferred to V2.

## LLM provider resolution

`makakoo agent create --specs <PATH>` detects available LLM providers
at scaffold time and emits the right `registerProvider` call in
`src/app.ts`. You don't need to hand-edit `app.ts` after scaffolding.

### Provider detection

At create time, the CLI probes (in order, concurrently, 2s timeout each):

1. `http://localhost:18080/v1/models` — **switchailocal** (local OpenAI-compat gateway)
2. `http://localhost:11434/api/tags` — **Ollama** (local). Prefers `:cloud` chat-capable models; falls back to any non-embedding model.
3. `ANTHROPIC_API_KEY` env var — **Anthropic** (cloud)
4. `OPENAI_API_KEY` env var — **OpenAI** (cloud)

Each detected provider has a `default_model` (the first available model).
The CLI sorts local-first, then cloud.

### Spec → provider selection

The spec's `model` field drives which provider is used:

- Spec's `model` starts with a known provider ID (`switchailocal/`,
  `ollama/`, `anthropic/`, `openai/`) → that provider is preferred, even
  if it sorts lower in the local-first order.
- Spec's `model` names a specific model (`ollama/gemma4:12b`) → kept as-is,
  not overwritten by the discovery's `default_model`.
- Spec's `model` is just a provider prefix (`ollama/`) → filled in with
  the discovery's `default_model`.
- Spec's `model` is the default (`anthropic/claude-sonnet-4-6`) → uses that.

If multiple providers are available and stdin is a TTY, the CLI prompts
the user to pick. Otherwise it auto-selects local-first and prints a
clear `warn:` line.

### What the scaffolder writes

`src/app.ts` is always emitted (Phase 6 of SPRINT-FLUE-DEFAULT-AGENT-SPECS).
The `registerProvider` call depends on the provider type:

**Local (switchailocal, ollama)** — needs `api` + `baseUrl` + the lope
team fix:

```ts
registerProvider('switchailocal', {
  api: 'openai-completions',
  baseUrl: process.env.SWITCHAI_BASE_URL ?? 'http://127.0.0.1:18080/v1',
  apiKey: process.env.AIL_API_KEY ?? 'sk-test-123',
  contextWindow: 128_000,
  maxTokens: 8_192,
});
```

Ollama is keyless — the scaffolder uses `'ollama'` as a placeholder
key because Flue v1.0.0-beta.9 rejects empty `apiKey`.

**Cloud catalog (anthropic, openai)** — only `apiKey` is needed
(catalog provides `contextWindow` + `maxTokens`):

```ts
registerProvider('anthropic', {
  apiKey: process.env.ANTHROPIC_API_KEY,
});
```

### Lope team fix: `contextWindow: 128_000` + `maxTokens: 8_192`

Flue v1.0.0-beta.9 silently defaults `contextWindow: 0` and
`maxTokens: 0` for non-catalog providers, which limits the LLM
output to **one token** and looks like a "hang". The scaffolder
**always sets both to safe values** (`128_000` and `8_192`) for local
providers. This is the single most important thing to get right —
without it, the LLM calls complete but produce nothing.

## Channel kinds

### `telegram` ✓ V1
```yaml
- kind: telegram
  token_env: TELEGRAM_BOT_TOKEN        # required, uppercase env var name
  allowed_users: ["123456"]            # optional, list of Telegram user IDs
```

The Flue channel verifies inbound webhooks via
`TELEGRAM_WEBHOOK_SECRET_TOKEN` (auto-added to `.env.example`). The
`allowed_users` list is enforced **inside the webhook handler** before
`dispatch()` (not on the channel config — `@flue/telegram` has no
`allowedUsers` option).

### `slack` ✓ V1 (inbound only — V1 outbound requires operator tool)
```yaml
- kind: slack
  token_env: SLACK_BOT_TOKEN          # required
  app_token_env: SLACK_APP_TOKEN      # required
  team_id_env: SLACK_TEAM_ID          # required
  allowed_users: ["U0123"]            # optional
```

The Flue channel verifies inbound request signatures using
`SLACK_SIGNING_SECRET` (auto-added to `.env.example`). It does NOT
take `botToken` / `appToken` / `teamId` — the channel only signs
inbound. For **outbound** messaging, the generated `post_slack_message`
tool calls the Slack Web API directly with `SLACK_BOT_TOKEN`.

### `discord` ✓ V1 (interactions only)
```yaml
- kind: discord
  token_env: DISCORD_BOT_TOKEN        # required (used by outbound tool)
  allowed_users: ["987654"]            # optional
```

The Flue channel verifies inbound interactions using
`DISCORD_PUBLIC_KEY` (auto-added to `.env.example`). It receives
**interactions** (slash commands, component clicks, modals) — NOT
free-form message events. Handlers are `interactions(input)`, not
`messageCreate`. To post back, operators wire their own `defineTool`
that calls the Discord REST API with `DISCORD_BOT_TOKEN`.

### `webhook` ✓ V1
```yaml
- kind: webhook
  path: /hooks/weather               # required, must start with /
  secret_env: WEATHER_WEBHOOK_SECRET  # required
```

Inbound-only HTTP endpoint with HMAC-SHA256 signature verification
(X-Signature header). No outbound tool — this channel only receives.

### `email` ⏳ V1 deferred
```yaml
- kind: email
  smtp_host: smtp.example.com        # required
  imap_host: imap.example.com        # required
  secret_env: EMAIL_CREDS            # required (JSON: {"user":"…","pass":"…"})
```

**V1 limitation**: there is no first-party `@flue/email` package on
npm. `makakoo agent create --specs` with an `email` channel writes the
slot TOML but errors at scaffold time:

```
spec declares an `email` channel, but the @flue/* adapter is not
available in V1. Use a `webhook` channel + a custom `defineTool` for
SMTP/IMAP, or remove the channel from the spec. Tracked for V2.
```

**Workaround**: use a `webhook` channel + a `defineTool` that calls
`nodemailer` for SMTP send and `imapflow` for IMAP receive.

### `voice` ⏳ V1 deferred
```yaml
- kind: voice
  twilio_account_sid_env: TWILIO_ACCOUNT_SID  # required
  secret_env: TWILIO_AUTH_TOKEN               # required
```

**V1 limitation**: same as email — no first-party `@flue/voice`
package. Workaround: `webhook` channel + `defineTool` that calls
Twilio's REST API.

## Trigger kinds

### `cron` ✓ V1
```yaml
- kind: cron
  schedule: "0 */6 * * *"            # required, 5-field cron
  timezone: "UTC"                    # optional, default "UTC"
```

V1 implementation uses `node-cron` directly. Standard 5-field cron
expressions. Each field range is validated at spec-parse time:

| Field | Range | Example |
|-------|-------|---------|
| minute | 0-59 | `0`, `*/15`, `0,30` |
| hour | 0-23 | `*/6`, `9-17` |
| day of month | 1-31 | `1`, `15` |
| month | 1-12 | `*`, `6,12` |
| day of week | 0-6 | `0` (Sunday) |

### `webhook` ✓ V1
```yaml
- kind: webhook
  path: /triggers/manual            # required, must start with /
  secret_env: MANUAL_TRIGGER_SECRET  # required
```

Standalone Hono server on port 8809 (convention — MCP proxy is on
8808) with HMAC-SHA256 signature verification. Override port with
`MAKAKOO_TRIGGER_PORT` env var.

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

## V1 limitations (tracked for V2)

1. **Email & voice channels deferred** — no first-party `@flue/*`
   adapter. Workaround: `webhook` + custom `defineTool`. Tracked for V2.
2. **Slack/Discord outbound is operator-supplied** — the Flue channel
   only handles inbound. Outbound requires a `defineTool` that calls
   the platform's Web/REST API directly. The generated template
   includes a `post_slack_message` starter tool.
3. **Telegram `allowedUsers` is handler-enforced** — not a channel
   config option. The webhook handler checks the sender ID before
   `dispatch()`.
4. **Cron uses `node-cron` directly** — `@flue/runtime` has no
   `defineTrigger` export. Standard 5-field cron only.
5. **Webhook trigger is a standalone Hono server** — separate port
   from the Flue app. Triggers are loaded as side-effecting imports
   from `assistant.ts`.
6. **Scope overlap detection is exact-string only** — proper glob
   overlap requires `globset`. V2.
7. **Flue v1.0.0-beta.9 LLM dispatch bug for some cloud models** —
   `flue dev` accepts the webhook, starts the agent session, but the
   background worker never fires the LLM call for some providers
   (observed: all Ollama `:cloud` models hang at the dispatch step;
   switchailocal works fine). The LLM itself is fine (direct
   `curl` to the provider works in <1s). The bug is in the Flue
   runtime's background worker, not the LLM. Track at
   <https://github.com/withastro/flue/issues>.
8. **Provider detection is best-effort** — the 2s probe timeout
   means a slow LLM gateway (cold start, network latency) might be
   missed. The spec's `model` field is the source of truth; if the
   auto-detection misses your provider, set `AGENT_MODEL` in `.env`
   or set the spec's `model` explicitly.

## Examples

See `examples/agents/`:
- `weather-bot.yaml` — cron + telegram + tools + scope
- `scheduled-reporter.yaml` — cron only, no channels
- `webhook-worker.yaml` — webhook channel + webhook trigger

## See also

- `spec-migration.md` — "I had a slot TOML, now what?" migration guide
- `walkthroughs/create-agent-from-spec.md` — end-to-end walkthrough
- `walkthroughs/flue-telegram-bot.md` — note: spec-driven is the new default
- **`makakoo agent init-spec <PATH>`** — interactive starter; asks the right questions, discovers providers, writes a correct spec. Use `--minimal` for a 10-line "hello world" spec.
- **`makakoo provider set <provider> [model]`** / **`makakoo provider get`** — project-level LLM default (stored at `$MAKAKOO_HOME/config/llm-default`). `init-spec` uses it as the default model choice.
