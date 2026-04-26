# SPRINT-MULTI-BOT-SUBAGENTS

**Status:** draft — round 3, addressing validator REQUIRED_FIX list
**Owner:** Sebastian
**Date:** 2026-04-26
**Phase numbering rationale:** Phase 0 = negotiation lock (pre-implementation
validator review); Phase 1–4 = implementation. Locking decisions before
writing code prevents the Q1–Q10 rework loops that sank Round 1.

---

## Origin

Sebastian wants Makakoo OS to support multiple independently scoped subagents,
each reachable through one or more chat transports. Round 1 assumed
Telegram-only and failed the updated scope. Round 2 revises every decision
through the multi-transport lens using `SPRINT.md`, `AUDIT.md`, `VISION.md`,
and `OPENCLAW-REFERENCE.md`.

OpenClaw (third-party, `/Users/sebastian/projects/makakoo/agents/sample_apps/openclaw`)
is the reference pattern: a transport plugin exposes gateway, outbound, config,
secrets, and routing adapters; inbound messages carry transport/account/thread/sender
metadata to a common dispatcher; outbound replies use the same context.
Evidence: `OPENCLAW-REFERENCE.md:14-20`, `OPENCLAW-REFERENCE.md:47-72`,
`OPENCLAW-REFERENCE.md:73-91`.

---

## Locked decisions

### Q1 — Process model

**Decision: one supervised process pair per agent slot.**

One Rust transport runtime plus one Python chat gateway. A single slot may
multiplex multiple transports internally through concurrent async transport
tasks feeding one ordered per-slot gateway queue.

Evidence: current gateway is one process with one config at `AUDIT.md:131-139`;
OpenClaw supports one process with N concurrent transport listeners at
`OPENCLAW-REFERENCE.md:14-20`.

---

### Q2 — Scope unit

**Decision: agent slot, not conversation.**

One slot has one persona, one tool scope, one path scope, and zero or more
transports.

Evidence: current system has one persona/tool surface at `AUDIT.md:77-79`;
target requires per-agent scope at `VISION.md:81-89`.

---

### Q3 — Runtime identity

**Decision: `MAKAKOO_AGENT_SLOT` only.**

LaunchAgent/systemd injects it; `--slot <slot_id>` overrides for tests. Do not
use `AGENT_SLOT_ID`. Structured logging already has an agent context slot at
`plugins-core/lib-harvey-core/src/core/observability/structured_logger.py:49-74`.

Evidence: slot identity is currently absent from gateway config at
`plugins-core/lib-harvey-core/src/core/chat/config.py:43-55`.

---

### Q4 — Registry location

**Decision: canonical source is `~/MAKAKOO/config/agents/<slot_id>.toml`.**

Existing `~/MAKAKOO/agents/<name>/agent.toml` scaffolds are legacy and must not
become a second registry.

Evidence: current Rust scaffold still targets
`{MAKAKOO_HOME}/agents/<name>/agent.toml` at `makakoo-core/src/agents/scaffold.rs:1-19`;
target registry is `~/MAKAKOO/config/agents/` at `VISION.md:129-136`.

---

### Q5 — Listing behavior

**Decision: `makakoo agent list` enumerates every TOML slot, including slots
with no enabled transport.**

Evidence: current CLI only supports start, stop, status, and health at
`makakoo/src/cli.rs:640-664`.

---

### Q6 — Tool and path scoping

**Decision: per-agent allowed-tools whitelist plus allowed-paths and
forbidden-paths, enforced before tool execution. New agents default to least
privilege.**

Evidence: generic tool surface problem at `AUDIT.md:108-113`; target per-agent
scope at `VISION.md:131-134`.

---

### Q7 — External username

**Decision: transport username does not need to match slot id. Access control
uses stable transport IDs: Telegram `chat_id` or `user_id`; Slack `user_id`,
`channel_id`, and optional `thread_ts`. Usernames are mutable display metadata
only.**

**`allowed_users` composition rule (simplified per validator fix):**
per-transport only. No slot-level superset. Each `[[transport]]` block carries
its own `[transport.config].allowed_users` list. Values are matched by the
transport's canonical sender_id type (Telegram `chat_id` string, Slack `U…`
ID). If `allowed_users` is absent or empty for a transport, that transport
rejects all inbound messages (least-privilege default). Slot-level
`allowed_users` is removed from the schema entirely.

---

### Q8 — Existing agents migration (revised per validator fix)

**Decision (v1): migrate HarveyChat/Olibia only. Other `agent-*` plugins receive
inventory/compat status in Phase 2 and are deferred to a follow-up phase.**

Rationale: Phase 1–4 already include registry, Slack, IPC, grants, Brain,
CLI, and dogfood — adding 12 plugin migrations would blow scope.

Evidence: existing plugin agent pattern at `AUDIT.md:115-129`; target
subsumption at `VISION.md:29-35` for the long-term model. Phase 2 adds a
`makakoo agent inventory` command that lists all existing `agent-*` plugins
with their current status (active/migrated/pending) without migrating them.

---

### Q9 — Transport-agnostic schema

**Decision: `[[transport]]` array with `kind` discriminator and transport-specific
`[transport.config]`. Secrets at the `[[transport]]` level (flat, not nested).**

Evidence: transport-agnostic requirement at `VISION.md:24-28`; OpenClaw
recommends per-agent transport blocks at `OPENCLAW-REFERENCE.md:113-120`.

**Field semantics:**

- **`transport.id`** (required): slot-unique string, e.g. `"telegram-main"`.
  This is the ROUTING KEY. Two transports with the same `kind` MUST have
  different `id`. Router uses `(slot_id, transport.id)`, NOT `(kind, account_id)`.
- **`support_thread`** (optional bool, default `false`): when `true`,
  `thread_id` field is populated for threaded messages; outbound replies
  honor the thread if the inbound frame carried one.
- **`secret_env`**: environment variable name for this transport's primary
  credential. Naming format: `<MAKAKOO_AGENT_SLOT>_<TRANSPORT_ID_UPPER>_<FIELD_UPPER>`.
  Example: `SECRETARY_TELEGRAM_MAIN_TOKEN` for the `secretary` slot's
  `telegram-main` transport's token.
- **`app_token_env`** and **`bot_token_env`**: Slack-specific; both required
  for Socket Mode.
- **`inline_secret_dev`**: TOML inline value, dev-only. Loads only if env var
  and `makakoo secret` both resolve to nothing. Logs a WARNING on load in
  all modes.

**Concrete TOML example — one slot with Telegram + Slack:**

```toml
slot_id = "secretary"
name = "Secretary"
persona = "Sharp professional secretary for Sebastian's freelance office"
inherit_baseline = false

allowed_paths = ["~/MAKAKOO/data/secretary/"]
forbidden_paths = ["~/CV/", "~/MAKAKOO/data/career/"]
tools = ["brain_search", "write_file", "gmail", "google-calendar"]

process_mode = "supervised_pair"

[[transport]]
id = "telegram-main"
kind = "telegram"
enabled = true
account_id = "@SecretaryBot"
secret_ref = "agent/secretary/telegram-main/bot_token"
secret_env = "SECRETARY_TELEGRAM_MAIN_TOKEN"
inline_secret_dev = ""
allowed_users = ["746496145"]

[transport.config]
polling_timeout_seconds = 30
allowed_chat_ids = ["746496145"]
allowed_group_ids = []
support_thread = true

[[transport]]
id = "slack-main"
kind = "slack"
enabled = true
account_id = "T0123TEAM:B0123BOT"
secret_ref = "agent/secretary/slack-main/bot_token"
app_token_ref = "agent/secretary/slack-main/app_token"
secret_env = "SECRETARY_SLACK_MAIN_BOT_TOKEN"
app_token_env = "SECRETARY_SLACK_MAIN_APP_TOKEN"
inline_secret_dev = ""
allowed_users = ["U0123ABC"]

[transport.config]
mode = "socket"
dm_only = true
channels = []
support_thread = true
```

---

### Q10 — Agent propagation

**Decision: grants gain `bound_to_agent`; Brain journal lines from agents gain
`[agent:<slot_id>]`; IPC frames carry `agent_slot_id`.**

Evidence: target cross-subsystem awareness at `VISION.md:17-20`; current grants
do not enforce agent binding at `AUDIT.md:140-141`.

---

### Q11 — v1 transports and multi-transport TOML schema

**Decision: v1 ships Telegram polling and Slack Socket Mode adapters. Discord
and WhatsApp are follow-on adapters documented in `docs/roadmap/adapters.md`.**

**Secrets precedence (locked, identical for every adapter):**

1. Environment variable (`secret_env`, `app_token_env`, `bot_token_env`).
2. `makakoo secret` store (`secret_ref`, `app_token_ref`, `bot_token_ref`).
3. TOML inline (`inline_secret_dev`). Logs WARNING. Refuses to load in
   non-dev mode (enforced at config load, not at adapter level).

Evidence: OpenClaw env-first precedence at `OPENCLAW-REFERENCE.md:73-91`.

**Per-transport validation rules (Phase 1 enforces during `agent create`):**

| Field | Telegram | Slack |
|---|---|---|
| `id` | required (slot-unique) | required (slot-unique) |
| `kind` | `"telegram"` | `"slack"` |
| `enabled` | bool, default `true` | bool, default `true` |
| `secret_ref` / `secret_env` | required | required (`bot_token`) |
| `app_token_ref` / `app_token_env` | not applicable | required (Socket Mode) |
| `allowed_users` | per-transport list; absent = deny all | per-transport list; absent = deny all |
| `support_thread` | bool, default `false` | bool, default `false` |
| `polling_timeout_seconds` | int, default `30` | not applicable |
| `dm_only` | not applicable | bool, default `true` |
| `channels` | not applicable | required if `dm_only = false` |
| `team_id` | not applicable | required |

`makakoo agent create` MUST run per-transport credential verifier BEFORE writing
files: `getMe` for Telegram, `auth.test` for Slack bot token, and Socket Mode
WebSocket connection probe for Slack app token. For Slack, the `team_id`
returned by `auth.test` MUST match the TOML `team_id` (reject on mismatch).

**Same-kind multi-transport guards:** Phase 1 schema validation rejects:
- duplicate `transport.id` within a slot;
- two Telegram transports whose `getMe.id` resolves to the same bot;
- two Slack transports with the same `bot_token` in the same `team_id`.

---

### Q12 — OpenClaw SDK choice

**Decision: rebuild the channel abstraction in Rust using OpenClaw's interface
shape as the contract. Do not add Node.js as a Makakoo core dependency.**

Implement gateway, outbound, config, secrets, routing, and status traits in v1.
**Defer until a second transport requires them:** pairing (use allowlist field
directly), groups, commands, approval, rich formatting, directory (sender
display-name), messaging, threading. **Acceptance is contract tests across
Telegram and Slack adapters, not TypeScript SDK reuse.**

Evidence: `OPENCLAW-REFERENCE.md:47-72` for the full adapter surface.

---

## Multi-transport concurrency model

A single slot may have N transports attached. When two transports for the same
slot deliver messages concurrently:

1. Each transport's Rust task receives its inbound message and constructs a
   `MakakooInboundFrame` (see Phase 1 IPC spec).
2. Both frames carry the same `agent_slot_id` (each transport task knows its
   own slot from spawn context) but different `transport_id` and sender metadata.
3. Both frames are written newline-delimited (`\n`) to the slot's Unix-domain
   IPC socket. tokio's per-stream write mutex serialises concurrent writers.
4. The Python gateway reads frames into a SINGLE asyncio queue per slot.
   Dequeued frames dispatch to the LLM sequentially. No parallelism within a
   slot.
5. Replies are sent back as `MakakooOutboundFrame` objects. The Rust router
   demultiplexes by `outbound.transport_id` (must match an inbound frame's
   `transport_id` from the same slot; cross-transport reply is FORBIDDEN in v1).
6. **Python gateway crash:** Rust transport detects broken pipe, logs structured
   error, drops in-flight frame, enters exponential-backoff reconnect
   (initial 500 ms, cap 30 s, jittered). No buffering, no replay.
7. **Single-transport failure isolation:** if the Slack WebSocket drops, the
   Telegram task continues uninterrupted.

**Bounded queue overflow policy:** the per-slot asyncio queue has a fixed
capacity of 100 frames. On overflow, the newest frame is dropped and a structured
warning is logged (`{"event": "queue.overflow", "transport_id": ..., "action":
"drop_newest"}`). This is intentional: v1 does not implement backpressure
on the transport task; transport tasks continue independently. Queue depth is
reported in `makakoo agent status <slot>`.

This model is designed so that replacing `kind = "telegram"` with
`kind = "slack"` does not change the concurrency semantics.

---

## Olibia migration (explicit)

The legacy Olibia bot migrates to a new slot **`harveychat`** (locked per
validator fix — never `olibia` as a slot id).

Migration rules:
- Token: preserved from `data/chat/config.json`; moved to `harveychat.toml`.
- Allowlist: `allowed_users` in TOML, same numeric chat_ids.
- Persona: `persona = null` inherits `HARVEY_SYSTEM_PROMPT` fallback.
  `HARVEY_SYSTEM_PROMPT` is NOT removed; it is the fallback for null persona.
- Conversation DB: archived at `data/agents/harveychat/conversations.db.bak`;
  new per-agent DB starts at `data/agents/harveychat/conversations.db`.
  The old shared DB is NOT migrated to per-agent schema (archived, not merged).
- LaunchAgent plist: regenerated from template with `MAKAKOO_AGENT_SLOT=harveychat`.
- The bot responds without manual reconfiguration after migration.

Grants issued by `harveychat` use `bound_to_agent = "harveychat"`. Journal
lines from `harveychat` carry `[agent:harveychat]`.

---

## IPC envelope schema (locked)

**Inbound: `MakakooInboundFrame`**

```
agent_slot_id: String           # set by transport task from spawn context
transport_id: String           # PRIMARY routing key, e.g. "telegram-main"
transport_kind: String          # "telegram" | "slack"
account_id: String               # auxiliary diagnostic; Telegram getMe.id;
                                  # Slack auth.test.bot_id + team_id
conversation_id: String         # Telegram: chat_id; Slack DM: im_id (D…);
                                  # Slack channel: C…
sender_id: String                # Telegram: chat_id; Slack: user_id (U…)
thread_id: Option<String>        # Telegram: message_thread_id; Slack: thread_ts
thread_kind: Option<String>      # "telegram_forum" | "slack_thread" | None
message_id: String               # Telegram: message_id int as string;
                                  # Slack: ts float-string
text: String
transport_timestamp: Option<String>  # original provider server timestamp;
                                      # present when transport supplies it
received_at: DateTime<Utc>       # Makakoo local receive clock; used for
                                  # routing and ordering across transports
raw_metadata: Map<String, Value> # transport-native extras for debugging
```

`sender_username` is intentionally NOT in the v1 frame. `ChannelDirectoryAdapter`
is deferred (Q12). Downstream code uses `sender_id` for both ACL and display.

**Outbound: `MakakooOutboundFrame`**

```
transport_id: String             # MUST match inbound frame's transport_id
                                  # for the same slot; cross-transport forbidden
transport_kind: String           # type dispatch for adapter
conversation_id: String          # channel/im id, NOT user id
thread_id: Option<String>        # honored only when support_thread = true
thread_kind: Option<String>      # MUST match inbound thread_kind; mismatch logs
                                  # WARN and drops thread (sends to conversation)
text: String
reply_to_message_id: Option<String>  # transport-native reply target; format
                                      # mismatch → drop reply_to, log WARN, send
                                      # to conversation without thread anchor
```

**IPC delivery semantics:** at-most-once. No frame ack, no IPC-layer retry.
Inbound frames dropped during gateway downtime are logged and NOT replayed.

---

## Phases

### Phase 0: Lock transport-agnostic design

**Goal:** Finalize Q1–Q12 and make the sprint document validator-ready.

**Criteria:**
- Q1 through Q12 are explicit, evidence-backed, and checked against
  `SPRINT.md`, `AUDIT.md`, `VISION.md`, `OPENCLAW-REFERENCE.md`, and current
  code paths.
- Stale Telegram-only language is removed: no "N Telegram bots" framing,
  no "10 open questions," no singleton `[transport]`, and no Slack-as-non-goal.
- TOML example includes one slot with Telegram and Slack using `[[transport]]`
  with flat secrets at the `[[transport]]` level (not nested under
  `[transport.config]`).
- `MAKAKOO_AGENT_SLOT` is the only runtime slot env var.
- Phase 0 numbering is documented as negotiation lock, not implementation work.
- v1 scope is Telegram polling plus Slack Socket Mode; Discord and WhatsApp
  are non-blocking follow-ups documented in `docs/roadmap/adapters.md`.
- OpenClaw is treated as interface evidence and contract shape, not a
  Node.js dependency.
- Every `REQUIRED_FIX` from the validator is mapped to a concrete decision
  or phase criterion in this document.

**Artifacts/Files/Deliverables:**
- Revised `SPRINT.md` with all 12 decisions locked
- Revised `VISION.md` if multi-transport scope changes acceptance criteria
- Confirmation of no remaining placeholder tokens or prose ellipsis

**Checks/Tests/Success Metrics:**
- Validator review returns PASS or only non-blocking notes
- Manual smell test: replacing `kind = "telegram"` with `kind = "slack"` does
  not invalidate schema, routing, identity, scoping, or process model
- Lint confirms no prose ellipsis tokens and no placeholder language

---

### Phase 1: Rust transport abstraction, adapters, and IPC core

**Goal:** Build the Rust transport layer, implement Telegram and Slack Socket Mode
adapters, and establish the Unix-domain IPC socket bridge between Rust and Python.

**Criteria:**
- Rust transport modules implement gateway, outbound, config, secrets, routing,
  and status traits matching the OpenClaw contract subset (Q12 trait table).
- Telegram adapter verifies credentials via Bot API `getMe`, receives inbound
  via `getUpdates` long polling, supports DMs, groups, and forum topic
  `message_thread_id`.
- Slack adapter verifies bot token via `auth.test` and app token via Socket Mode
  WebSocket connection probe. Events handled: `message.im` (DM) and `message.channel`
  (channel, only if `dm_only = false`). **Self-loop filtering:** ignore events
  where `event.user` matches the bot's own `auth.test.bot_id`; ignore
  `app_mention` events (deferred); ignore `message_changed`, `message_deleted`,
  `message_replied`, and unsupported subtype events. **Invalid app token
  handling:** Socket Mode WebSocket connect failure with `xapp-…` token returns
  a clear "invalid Socket Mode app token" error before the transport task starts.
  **Reconnect:** on WebSocket disconnect that is not an intentional close, the
  Slack adapter enters exponential-backoff reconnect (initial 1 s, cap 60 s,
  jittered) and emits a `status.reconnecting` event.
  **Duplicate/retry envelope handling:** Slack sends each event exactly once
  under normal operation. The adapter tracks the last processed `event.ts` per
  `event.channel` and ignores duplicates within a 5-minute sliding window.
  Events received out of order (lower `event.ts` than last processed) are
  logged as `{"event": "slack.out_of_order", "ts": ...}` and dropped.
- Inbound `MakakooInboundFrame` includes all fields from the IPC envelope spec
  above. `sender_username` is absent (directory lookup deferred).
- `transport_timestamp` is populated when the transport provider supplies a
  server-side timestamp. `received_at` is always populated from Makakoo local
  UTC clock.
- Outbound `MakakooOutboundFrame` matches the spec above. Cross-transport
  reply (outbound `transport_id` not matching any inbound `transport_id` for the
  same slot) is rejected at the router with a structured error log.
- Router demux uses `(agent_slot_id, transport_id)` as the primary mapping.
  Inbound transport tasks know their own `(agent_slot_id, transport_id)` from
  spawn context (no per-message registry lookup).
- Same-slot multi-transport concurrency: per-transport async tasks feed one
  bounded queue per slot (capacity 100; overflow drops newest, logs warning).
  One Python gateway consumes sequential frames and replies to the originating
  transport.
- Secrets resolution: env var wins over `makakoo secret`, which wins over
  `inline_secret_dev`. Dev-mode inline warnings logged.
- IPC socket path: `~/MAKAKOO/run/agents/<slot_id>/ipc.sock` (parent dir `0700`).

**Artifacts/Files/Deliverables:**
- `makakoo-core/src/transport/mod.rs` — trait hierarchy and frame types
- `makakoo-core/src/transport/telegram.rs` — Telegram adapter with credential
  verification, polling, self-loop filtering, and forum topic support
- `makakoo-core/src/transport/slack.rs` — Slack Socket Mode adapter with bot/app
  token verification, WebSocket lifecycle, self-loop filtering, reconnect,
  and duplicate detection
- `makakoo-core/src/transport/router.rs` — `(slot_id, transport_id)` demux and
  cross-transport reply rejection
- `makakoo-core/src/transport/secrets.rs` — env/secret/inline precedence
- `makakoo-core/src/transport/config.rs` — TOML config loading and per-transport
  schema validation including same-kind multi-transport guards
- `makakoo-core/src/transport/outbound.rs` — outbound frame adapter
- `makakoo-core/src/transport/status.rs` — transport status adapter with
  `connected | reconnecting | failed` states
- `makakoo-core/src/transport/pairing.rs` — allowlist adapter (direct, no
  ChannelPairingAdapter in v1)
- `makakoo-core/src/ipc/mod.rs` — `MakakooFrame` enum with `MakakooInboundFrame`
  and `MakakooOutboundFrame` variants
- `makakoo-core/src/ipc/unix_socket.rs` — Unix-domain socket bridge with
  newline-delimited JSON framing
- `makakoo-core/Cargo.toml`

**Checks/Tests/Success Metrics:**
- Unit: Telegram credential verification with mocked `getMe` HTTP response
- Unit: Telegram inbound update normalizes `chat_id`, `user_id`, and
  `message_thread_id`
- Unit: Telegram self-loop: bot's own message is not emitted as inbound frame
- Unit: Slack bot token verification with mocked `auth.test` response
- Unit: Slack Socket Mode event deserialization for `message.im` and
  `message.channel`
- Unit: Slack self-loop: event where `user` matches bot ID is dropped
- Unit: Slack invalid `xapp-…` token returns clear error before transport starts
- Unit: Slack WebSocket disconnect triggers reconnect with backoff
- Unit: Slack duplicate event (same `ts` within 5-minute window) is dropped
- Unit: Slack out-of-order event (lower `ts` than last processed) is dropped
- Unit: router resolution from `transport_id` to `agent_slot_id`
- Unit: cross-transport outbound reply is rejected with structured error
- Unit: env secret wins over `makakoo secret`, which wins over `inline_secret_dev`
- Unit: queue overflow drops newest and logs structured warning
- Integration: Rust-to-Python IPC frame round-trip over Unix-domain socket
- `cargo test --workspace`

---

### Phase 2: Agent registry, schema, CLI lifecycle, and migration

**Goal:** Make subagent slots first-class Makakoo objects with TOML registry,
CLI commands, and harveychat migration. This phase does NOT build or test
transport adapters — those are Phase 1.

**Criteria:**
- `~/MAKAKOO/config/agents/` is created on first use.
- One TOML file defines one slot; no duplicate registry under `~/MAKAKOO/agents/`.
- Schema validates `slot_id`, `name`, `persona`, `allowed_paths`,
  `forbidden_paths`, `tools`, `process_mode`, and `[[transport]]`.
  `allowed_users` is per-transport only (no slot-level field).
- `makakoo agent list` shows all configured slots, status, and transport kinds.
  Status is `UNCONFIGURED` if no `[[transport]]` block is enabled.
- `makakoo agent show <slot>` redacts all secret fields: `secret_ref`,
  `secret_env`, `app_token_ref`, `app_token_env`, `bot_token_ref`,
  `bot_token_env`, and `inline_secret_dev`.
- `makakoo agent create <slot>` supports flags for persona, paths, tools,
  allowed users (per-transport), and multiple `[[transport]]` blocks.
  Validates credentials via Rust transport adapter BEFORE writing files.
- `makakoo agent inventory` lists all existing `agent-*` plugins with their
  current status (active/migrated/pending). Does NOT migrate them.
- `makakoo agent validate <slot>` runs per-transport credential verifier
  WITHOUT starting the agent. Useful before `start` to surface bad credentials.
- Duplicate slot rejection: TOML filename must match `slot_id` field;
  rejects if file already exists.
- harveychat migration: reads `~/MAKAKOO/data/chat/config.json`, writes
  `~/MAKAKOO/config/agents/harveychat.toml`, archives old config, moves
  `conversations.db` to `data/agents/harveychat/conversations.db.bak`.
  New per-agent DB starts fresh. Migration is idempotent (re-running is a
  no-op on already-migrated slots). The old shared `conversations.db` is
  preserved at its archive path for rollback.
- Unknown `MAKAKOO_AGENT_SLOT` at gateway startup: if the TOML file does not
  exist, the gateway exits immediately with exit code 64 and the message
  "Agent slot '<slot_id>' not found at ~/MAKAKOO/config/agents/<slot_id>.toml.
  Run 'makakoo agent create <slot_id>' to create it." There is no fallback to
  `data/chat/config.json` for new slots — that fallback only applies to the
  harveychat migration, which is explicitly invoked.

**Artifacts/Files/Deliverables:**
- `makakoo-core/src/agents/mod.rs` — agent module entry point
- `makakoo-core/src/agents/schema.rs` — `AgentSlot`, `TransportEntry`,
  `TransportKind`, `TelegramTransport`, `SlackTransport`, `InboundFrame`,
  `OutboundFrame` types
- `makakoo-core/src/agents/registry.rs` — TOML load/save, validation, slot index
- `makakoo-core/src/agents/migrate/harveychat.rs` — JSON→TOML migration
- `makakoo-core/src/agents/status.rs` — per-transport status reporting
- `makakoo/src/cli.rs` — `agent` subcommand group
- `makakoo/src/commands/agent.rs` — `list`, `show`, `validate` subcommands
- `makakoo/src/commands/agent_create.rs` — `create` wizard
- `makakoo/src/commands/setup/cli_agent.rs` — agent setup utilities
- `makakoo-core/templates/agent-plist.plist` — LaunchAgent template with
  `MAKAKOO_AGENT_SLOT` env var injected
- `makakoo-core/templates/agent-systemd.service` — systemd template
- `plugins-core/lib-harvey-core/src/core/chat/config.py` — gateway TOML reader
- `plugins-core/lib-harvey-core/src/core/chat/store.py` — per-agent conversation
  DB path resolution (`data/agents/<slot_id>/conversations.db`)

**Checks/Tests/Success Metrics:**
- Unit: TOML parse and round-trip for a Telegram plus Slack slot
- Unit: duplicate slot rejection (filename mismatch with `slot_id` field)
- Unit: invalid `slot_id` rejection (non-alphanumeric or too long)
- Unit: secret redaction in `agent show` (all secret fields absent from output)
- Unit: harveychat JSON→TOML round-trip (write → read → compare fields)
- Unit: harveychat migration idempotency (re-running is a no-op)
- Unit: `agent create` refuses invalid Telegram token before writing files
- Unit: `agent create` refuses invalid Slack bot token before writing files
- Unit: `agent create` refuses invalid Slack app token before writing files
- Unit: Slack `team_id` mismatch between TOML and `auth.test.team_id` is rejected
- Unit: `agent validate` with corrupted token reports failure without starting
  the agent
- Unit: unknown `MAKAKOO_AGENT_SLOT` produces exit code 64 with the specified
  error message
- Unit: `agent inventory` lists existing `agent-*` plugins without migrating them
- Integration: `agent create harveychat` from existing `data/chat/config.json`
  produces valid `harveychat.toml`
- Integration: `conversations.db` is archived (not migrated) and new DB starts
  at `data/agents/harveychat/conversations.db`
- `cargo test --workspace`

---

### Phase 3: Per-agent identity, scoping, and shared resources

**Goal:** Each running subagent knows its slot id, renders the correct persona,
and is restricted to its declared tools and paths.

**Criteria:**
- Python gateway loads slot config from `MAKAKOO_AGENT_SLOT` env var (injected
  by plist) or `--slot` CLI flag. Resolves `~/MAKAKOO/config/agents/<slot>.toml`.
  If unset or file not found, exits with exit code 64 and the specified message.
- System prompt includes: canonical bootstrap plus per-agent persona snippet plus
  identity block: *"You are `<name>`. Your slot id is `<slot_id>`. This message
  arrived via `<transport_kind>`. Your allowed tools are `<tools>`. Your allowed
  paths are `<paths>`."*
- `HARVEY_SYSTEM_PROMPT` is NOT removed; it is the fallback for `persona = null`.
- Tool dispatcher enforces per-agent allowed-tools whitelist; out-of-scope calls
  return structured `ToolNotInScope` error with the tool name and the slot's
  allowed list.
- File write enforcement: `allowed_paths` checked first, then `forbidden_paths`
  overrides, then `bound_to_agent` grants. File read enforcement: same layering
  applied before returning file contents. Read denial returns structured
  `PathNotInScope` error with the denied path and the slot's allowed/forbidden
  lists.
- Brain journal writes from agents prepend `[agent:<slot_id>]` prefix.
- Concurrent agent journal writes use file locking (`fcntl.flock` on the
  journal file).
- MCP HTTP calls: `X-Makakoo-Agent-ID` header forwarded through
  `makakoo-mcp/src/http_server.rs`; stdio MCP calls read `MAKAKOO_AGENT_SLOT`
  and set `harvey_agent_id` ContextVar.
- `allowed_users` enforcement: per-transport only. Telegram uses numeric
  `chat_id` string; Slack uses `U…` user ID. Absence means deny all.
- `makakoo agent status <slot>` reports PER `transport.id`: connection state,
  `last_inbound` timestamp, `errors_1h` count, and `queue_depth`.
- v1 does NOT include per-agent LLM model override (BridgeConfig `switchai_model`,
  `max_tokens` remain shared machine-level config). This is a deferred decision
  documented in `docs/roadmap/adapters.md` alongside the Slack webhook production
  path, Discord, and WhatsApp.

**Artifacts/Files/Deliverables:**
- `plugins-core/lib-harvey-core/src/core/chat/bridge.py` — persona renderer reads
  slot config, adds identity block, sets `harvey_agent_id` ContextVar
- `plugins-core/lib-harvey-core/src/core/chat/gateway.py` — reads
  `MAKAKOO_AGENT_SLOT`, sets per-agent db path, single asyncio queue per slot,
  sequential LLM dispatch across all transports for the slot
- `plugins-core/lib-harvey-core/src/core/chat/tool_dispatcher.py` — whitelist
  enforcement with structured error responses
- `plugins-core/lib-harvey-core/src/core/chat/file_enforcement.py` — read and
  write path enforcement against allowed_paths/forbidden_paths
- `plugins-core/lib-harvey-core/src/core/chat/brain_sync.py` — `[agent:<id>]`
  prefix and file-lock journal writes
- `makakoo-core/src/agents/scope.rs` — `check_tool()`, `check_path()`,
  `bound_to_agent` grant filtering
- `makakoo-core/src/agents/identity.rs` — slot config loading from TOML,
  env var reading, unknown slot exit
- `makakoo-mcp/src/http_server.rs` — `X-Makakoo-Agent-ID` header forward
- `makakoo-mcp/src/dispatch.rs` — `harvey_agent_id` ContextVar from header/env
- `makakoo-mcp/src/handlers/tier_a/agents.rs` — per-agent agent-handler routes
- `docs/roadmap/adapters.md` — deferred decisions: per-agent LLM model config,
  Slack Events API webhook production path, Discord, WhatsApp, full
  ChannelDirectoryAdapter

**Checks/Tests/Success Metrics:**
- Unit: gateway selects slot config from `MAKAKOO_AGENT_SLOT`
- Unit: `--slot` overrides env for local tests
- Unit: unknown slot id produces exit code 64 with clear message
- Unit: out-of-scope tool rejection (whitelist miss returns `ToolNotInScope`)
- Unit: `forbidden_paths` overrides `allowed_paths` on write attempt
- Unit: `forbidden_paths` overrides `allowed_paths` on read attempt
- Unit: read denial returns `PathNotInScope` structured error
- Unit: `bound_to_agent = "career"` grant is invisible to `harveychat` slot
- Unit: `allowed_users` rejects Slack DM from non-allowlisted user ID
- Unit: `allowed_users` rejects Telegram message from non-allowlisted chat_id
- Integration: two agents write to today's Brain journal simultaneously without
  corruption (file lock plus read-back verification)
- Integration: journal lines contain `[agent:<slot_id>]`
- Integration: Olibia "give yourself access to ~/Shared/" uses
  `bound_to_agent = "harveychat"` (her own slot)
- Integration: Career slot cannot exercise a grant bound to harveychat
- `cargo test --workspace`
- Python tests for `plugins-core/lib-harvey-core` chat and capability modules

---

### Phase 4: Multi-transport dogfood and release hardening

**Goal:** Prove the system works end-to-end with multiple agents and at least
one agent attached to two transports simultaneously.

**Criteria:**
- Three agent slots run simultaneously: `harveychat` (Telegram-only, migrated;
  display name "Olibia"), `secretary` (Telegram + Slack Socket Mode, created
  new), `career` (Telegram-only, created new).
- `secretary` is reachable via both Telegram and Slack with distinct
  `transport_id` in each frame. Replies go back to the ORIGINATING
  `transport_id` only (cross-transport reply is rejected at the router).
- Each agent replies with its configured persona and scoped tool surface.
- Crash of one agent process pair does not affect the other agents.
- `makakoo agent status <slot>` reports per `transport.id` with connection
  state, `last_inbound`, `errors_1h`, and `queue_depth`.
- Existing `harveychat` (display: "Olibia") bot continues working after
  migration without manual reconfiguration. The old `data/chat/config.json`
  is preserved (not deleted) for rollback safety.
- `docs/roadmap/adapters.md` is published with all deferred decisions.

**Artifacts/Files/Deliverables:**
- `makakoo-core/src/agents/lifecycle.rs` — supervised spawn with `MAKAKOO_AGENT_SLOT`
  env var; restart on crash
- `makakoo-core/src/agents/status.rs` — status aggregation across transports
  with queue depth reporting
- `makakoo-core/src/transport/router.rs` — verified routing to same `agent_slot_id`
  from multiple `transport_id` values
- `plugins-core/lib-harvey-core/src/core/chat/gateway.py` — single asyncio
  queue per slot; sequential LLM dispatch across all transports for the slot
- `docs/user-manual/agent.md` — CLI reference for all agent subcommands
- `docs/walkthroughs/multi-transport-subagents.md` — end-to-end walkthrough
  from clean config through live dual-transport reply
- `docs/roadmap/adapters.md` — deferred decisions catalog
- `docs/troubleshooting/agents.md` — common failure modes and remediation

**Checks/Tests/Success Metrics:**
- Live dogfood: `harveychat`, `secretary`, and `career` slots running
  simultaneously
- Live dogfood: `secretary` receives Telegram DM and Slack DM, replies to
  each within 5 seconds, with distinct `transport_id` logged per reply
- Smoke: concurrent Telegram plus Slack messages to `secretary` — conversation
  history shows both turns interleaved in slot order with `[telegram]` and
  `[slack]` channel prefixes visible to the LLM
- Smoke: `secretary` Slack thread reply with `support_thread = true` goes into
  the originating `thread_ts`, not into the parent channel
- Smoke: `secretary` Slack reply WITHOUT `support_thread` goes to the DM
  conversation without a thread anchor
- Fault: SIGTERM `secretary` Python gateway; `harveychat` and `career`
  continue responding without restart
- Fault: Slack Socket Mode WebSocket drops for `secretary`; Telegram transport
  continues uninterrupted; Slack reconnects with backoff; `agent status`
  reports Slack as `reconnecting` then `connected`
- Fault: `makakoo agent validate secretary` with corrupted Slack bot token
  reports the failure WITHOUT touching the running agent
- Routing: `secretary` Telegram and Slack logs share `agent_slot_id` but differ
  in `transport_id`, `conversation_id`, and `thread_id`
- Scope: `career` cannot access Secretary paths or `harveychat` grants
- Regression: migrated `harveychat` responds without manual reconfiguration
- Docs: following the walkthrough from clean config through live dual-transport
  reply completes successfully
- `cargo test --workspace`

---

## Acceptance criteria

The sprint is done when:

1. `makakoo agent create <name>` works end-to-end in less than 30 seconds
   (Telegram bot token already created via @BotFather; Slack app already
   created in workspace).
2. Three subagents simultaneously running, each responding on its own bot,
   with isolated personas and tools.
3. Per-agent grant scoping enforced: `career` cannot exercise a grant bound
   to `harveychat`.
4. Each agent's journal lines carry `[agent:<slot_id>]` prefix.
5. `makakoo agent list` shows live status for every slot in the registry.
6. `secretary` replies to both Telegram and Slack DMs; logs show distinct
   `transport_kind` per message.
7. The original Olibia / harveychat bot continues to work after migration
   without reconfiguration.
8. Agent A can see Agent B's `[agent:<id>]` journal lines when searching
   Brain (cross-agent journal visibility preserved from VISION.md).

---

## Non-goals

- Discord and WhatsApp adapters (explicit follow-on adapters; documented in
  `docs/roadmap/adapters.md`).
- Multi-user-per-agent ACLs beyond per-transport sender-id allowlist.
- Voice mode, video, advanced media.
- Cross-agent delegation ("@Olibia, ask @CareerBot …").
- Admin management bot.
- Telegram token revocation via API.
- Linux systemd unit delivery (templates included but not actively tested).
- Error-rate tracking in `makakoo agent status` (polling latency only).
- OAuth user-token flows for Slack (v1 ships bot-token only; Socket Mode).
- Full `ChannelDirectoryAdapter` (sender_username resolution deferred).
- Per-agent LLM model override (deferred to follow-up phase).
- Conversation DB schema migration (old DB archived, new DB starts fresh).
- Migration of `agent-*` plugins beyond harveychat (Phase 2 inventories them;
  migration is a follow-up phase).

---

## Estimated cost

| Phase | Focus | Est. LOC | Notes |
|-------|-------|----------|-------|
| 0 | Lock Q1–Q12 | 0 | Validator review only |
| 1 | Rust transport abstraction, Telegram adapter, Slack Socket Mode adapter, IPC bridge | 1100–1300 | Telegram polling + Slack Socket Mode (tokio-tungstenite, WebSocket lifecycle, reconnect, ack, duplicate detection, self-loop filtering) + IPC socket + frame types + router |
| 2 | Registry, TOML schema, CLI lifecycle, harveychat migration | 500 | TOML validation, CLI list/show/create/validate/inventory, JSON→TOML migration, per-transport secret redaction |
| 3 | Per-agent identity, scoping, file enforcement, Brain, MCP, grants | 450 | Tool whitelist, path enforcement (read + write), journal prefix, file locking, MCP header, unknown slot exit |
| 4 | Multi-transport dogfood, crash testing, Slack Socket Mode E2E, docs | 500 | 3-agent live, concurrent dual-transport, fault injection (SIGTERM, WebSocket drop), queue depth reporting, walkthrough + troubleshooting docs |

**Total: 2550–2800 LOC plus tests and docs; approximately 8 working days.**

Phase boundaries are hard cut lines. If any phase blows past two times
estimated LOC, stop and re-negotiate scope before continuing.

---

## Risk register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| OpenClaw interface surface (~15 adapters) too large for v1 | Medium | Medium | Deferred adapters table (Q12); v1 ships 5 of ~15; `ChannelDirectoryAdapter` explicitly deferred |
| Slack Socket Mode insufficient for production | Low | Medium | Docs flag Events API webhook plus tunnel path; Phase 4 dogfood uses Socket Mode |
| `sender_username` async lookup adds latency | Low | Low | Absent from v1 frame; downstream uses `sender_id`; Phase 3 does not derive username |
| Per-slot IPC demux complexity (asyncio sequential dispatch) | Medium | Medium | Sequential dispatch avoids Python gateway thread-safety issues; documented in concurrency model |
| Token sprawl (N slots times N transports) | Medium | Low | `makakoo secret` store is first-class; env var naming is namespaced per Q11 |
| Rust trait translation of OpenClaw TypeScript optional adapters | Medium | Medium | Deferred adapters table; v1 ships concrete adapters; optional adapter gaps found in Phase 1 integration test |
| Slack WebSocket duplicate/retry envelope handling complexity | Low | Medium | 5-minute sliding window dedup; out-of-order detection; both testable with mocked Socket Mode server |
