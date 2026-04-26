# SPRINT-MULTI-BOT-SUBAGENTS

**Status:** draft — round 2, in validator review
**Owner:** Sebastian
**Date:** 2026-04-26
**Related sprints:** v0.6 agentic-plug (shipped 2026-04-21)
**Phase numbering rationale:** Phase 0 = negotiation lock (pre-implementation
validator review); Phase 1–4 = implementation. This shifts the original
Phase 1–5 down by one. Rationale: locking decisions before writing code
prevents the Q1–Q10 rework loops that sank Round 1.

---

## Origin

Sebastian wants Makakoo OS to support multiple independently scoped subagents,
each reachable through one or more chat transports (Telegram, Slack, and
follow-on adapters). Round 1 assumed Telegram-only and failed the updated
scope. Round 2 revises every decision through the multi-transport lens using
`SPRINT.md`, `AUDIT.md`, `VISION.md`, and `OPENCLAW-REFERENCE.md`.

OpenClaw (third-party, `/Users/sebastian/projects/makakoo/agents/sample_apps/openclaw`)
is the reference pattern: a transport plugin exposes gateway, outbound, config,
secrets, and routing adapters; inbound messages carry
`{transport, account, thread, sender}` metadata to a common agent dispatcher;
outbound replies use the same context.

---

## Locked decisions

### Q1 — One process per agent vs multiplexed gateway?

**Decision: one supervised process pair per agent slot.**

One Rust transport runtime plus one Python chat gateway per slot.
A single slot may multiplex multiple transports internally (one Rust process,
multiple poller/webhook tasks). A crashing LLM call in one slot does not affect
other slots. RAM cost (~80–100 MB per slot) is acceptable for a workstation
targeting 3–10 agents.

Evidence: AUDIT.md § "What's hard-coded" documents the current single-gateway
reads one config. VISION.md § Mental model diagrams one process per subagent.
Per-slot isolation is required by VISION.md § cross-subsystem awareness.

---

### Q2 — Per-bot vs per-conversation scoping?

**Decision: per-agent slot (one slot = one persona, one tool scope, one path
scope, zero or more transports).**

Evidence: AUDIT.md § "What Olibia means today" establishes that today one bot =
one persona. Sebastian's intent (VISION.md § "user's expressed intent") describes
secretaries and career-managers as separate bots. Per-conversation scoping adds
a routing layer without a stated requirement.

---

### Q3 — How does an agent know its own slot id at runtime?

**Decision: `MAKAKOO_AGENT_SLOT` environment variable injected by the
LaunchAgent plist; `--slot <slot_id>` CLI flag as override for ad-hoc testing.**

`gateway.py` reads it and passes it into `render_system_prompt()`. The existing
`_agent_id` ContextVar in `structured_logger.py:50` (`contextvars.ContextVar("harvey_agent_id")`)
carries the same value for logging layers — it is set from `MAKAKOO_AGENT_SLOT`
at gateway startup. All phases use `MAKAKOO_AGENT_SLOT` exclusively (not
`AGENT_SLOT_ID`); no alias exists.

Evidence: AUDIT.md § "HARVEY_SYSTEM_PROMPT is a single string constant" shows
no current injection mechanism. VISION.md § "identity injection at session
start" requires the gateway to tell the LLM its slot.

---

### Q4 — Where do agent definitions live?

**Decision: canonical registry is `~/MAKAKOO/config/agents/<slot_id>.toml`.
Generated runtime shims live under the Makakoo plugin and agent directories.**

The existing `makakoo-core/src/agents/scaffold.rs` (`AgentScaffold::list()`)
reads `plugins-core/agent-*/plugin.toml` for plugin enumeration, NOT for agent
slots. The new canonical path is `~/MAKAKOO/config/agents/<slot_id>.toml`.
Existing `agent-*` plugins are migrated into the TOML registry (see Q8).

`~/MAKAKOO/config/agents/` is created on first use. No second registry index.

Evidence: VISION.md § "transport-agnostic abstraction" requires TOML-first.
The existing scaffold path (`~/MAKAKOO/agents/<name>/agent.toml`) is retired;
its role is subsumed by the new registry.

---

### Q5 — Does `makakoo agent list` enumerate unprovisioned slots?

**Decision: yes, but only slots that have a `<slot_id>.toml` in the registry.**

A slot without any enabled transport (all `[[transport]]` blocks have
`enabled = false` or are absent) is still a registered intent and appears
as `UNCONFIGURED`. TOML-first — no TOML, no enumeration.

Evidence: VISION.md § "easy define" uses `makakoo agent list` as the discovery
surface. Slots without a TOML are not enumerated.

---

### Q6 — Per-agent allowed-tools whitelist + forbidden-paths blacklist?

**Decision: yes to both, layered on top of the existing three-layer capability
sandbox. Default is least privilege — new agents ship with zero tools unless
explicitly granted.**

Schema: `tools = ["email", "calendar"]` (whitelist; absent or empty = NO tools
unless `inherit_baseline = true`). `forbidden_paths = ["~/CV/"]` (blacklist;
additive to `allowed_paths`). Tool dispatcher checks whitelist first; out-of-
scope calls return structured `tool not in scope`.

Evidence: AUDIT.md § "Tool surface is generic, not role-specific" documents
that Olibia has the same 12 tools regardless of context. VISION.md §
"per-agent scope" explicitly requires "enforceable allowed-tools,
allowed-paths".

---

### Q7 — External username vs slot id — must they match?

**Decision: no. Slot id is internal; transport username is external. Credential
verification resolves external identity.**

Evidence: AUDIT.md § "Telegram username vs slot id" has no existing constraint.
VISION.md § "easy define" uses `--telegram-token` as the link, not username
matching.

**Transport-specific identity edge cases:**

- **Telegram**: `chat_id` (integer, string-encoded in TOML `allowed_users`)
  is the canonical sender identifier. `username` (display name) is optional
  and mutable and NOT used for access control. Group topics use
  `message_thread_id` (integer).
- **Slack**: `sender_id` (e.g. `U0123ABC`) is the canonical Slack user ID.
  `username` (display name) is mutable and NOT used for access control.
  `channel_id` (e.g. `C0123DEF`) identifies a channel conversation; DMs use
  `im_id` (e.g. `D0123DEF`) — the two are NOT interchangeable and must be
  validated separately. Threads use `thread_ts` (float string, e.g. `"1700000000.001"`).
  Bot-token-only in v1; user-token OAuth is a follow-on adapter.
  `allowed_users` matches by Slack `sender_id`.

**`allowed_users` composition rule:** the slot-level `allowed_users` is the
SUPERSET that all transports inherit by default. The per-transport
`[transport.config].allowed_users` NARROWS that set for that transport
specifically (set intersection). If the per-transport list is absent, the
slot-level list applies verbatim. If both are absent, the slot rejects all
inbound messages until an allowlist is configured (least-privilege default).
Values are matched against the transport's canonical `sender_id` type
(Telegram `chat_id` string, Slack `U…` ID), so the slot-level list MUST
contain values from the union of the canonical types of all attached
transports.

---

### Q8 — Existing `agent-*` plugins: subsume or parallel?

**Decision: subsume. All existing `agent-*` plugins become subagent slots.**

Evidence: AUDIT.md § "plugin registry" lists 13 `agent-*` plugins with
`kind=agent`. VISION.md § "subsumes the existing agent-* plugin pattern"
explicitly states the intent.

Migration: Phase 2 runs a one-time migration that reads each existing
`agent-*/plugin.toml`, creates a corresponding `<slot_id>.toml` with
`inherit_baseline = true`, and archives the legacy config. Migration is
idempotent — re-running is a no-op on already-migrated slots.

---

### Q9 — Transport-agnostic schema: singleton `[transport]` vs `[[transport]]` array?

**Decision: `[[transport]]` array with `kind` discriminator and transport-
specific `[transport.config]`.**

Evidence: VISION.md § "transport-agnostic abstraction" requires the schema to
accommodate WhatsApp, Slack, email, and voice without rework. Singleton
`[transport]` would require a second array field for multi-transport slots.

**Field semantics:**

- **`transport.id`** (required): a slot-unique string like `"telegram-main"`.
  This is the ROUTING KEY — the IPC `transport_id` field in every frame matches
  this. Two transports with the same `kind` MUST have different `id`. The
  router uses `(slot_id, transport.id)`, NOT `(kind, account_id)`, for demux.
  `account_id` is auxiliary diagnostic data (resolved from the transport's
  identity API) and never the primary key.
- **`support_thread`** (optional bool, default `false`): when `true`, the
  inbound frame's `thread_id` field is populated for messages in threads
  (Telegram forum topics → `message_thread_id`; Slack thread replies →
  `thread_ts`); when `false`, threaded messages are flattened into the parent
  conversation and `thread_id = None` in the frame. Outbound reply behavior:
  if `support_thread = true` and the inbound frame carried a `thread_id`,
  the reply is sent into the same thread; otherwise the reply is sent into
  the parent conversation.
- **Slack DM vs channel:** DM events arrive with `channel = D…` (im_id);
  channel events arrive with `channel = C…`. The Slack adapter normalizes
  both into the inbound frame's `conversation_id` field. The TOML's
  `dm_only = true` (default) limits the adapter to DMs only;
  `dm_only = false` enables channel events filtered by the optional
  `channels = ["C…"]` whitelist.

**Concrete TOML example — one slot with Telegram + Slack:**

```toml
slot_id = "secretary"

name = "Secretary"
persona = "Sharp, professional secretary for Sebastian's freelance office"
inherit_baseline = false

# Per-slot path and tool scope
allowed_paths = ["~/MAKAKOO/data/secretary/"]
forbidden_paths = ["~/CV/", "~/MAKAKOO/data/career/"]
tools = ["email", "calendar", "write_file", "run_command"]

# Slot-level allowed_users is the SUPERSET that all transports inherit
# unless overridden per-transport (intersection rule — see Q7).
# Values must be canonical sender_id types of the attached transports
# (Telegram chat_id strings, Slack U… IDs).
allowed_users = ["746496145", "U0123ABCD"]

# ── Transport 1: Telegram ──────────────────────────────────────
[[transport]]
id = "telegram-main"
kind = "telegram"
enabled = true

[transport.config]
# Credentials via makakoo secret store (preferred)
token.secret_ref = "secret:telegram/secretary-bot-token"
# Fallback for development only (writes WARNING to log on load):
# token = "123456:ABCdefGHIjklMNOpqrsTUVwxyz"

# Per-transport allowlist NARROWS the slot-level list (intersection)
allowed_users = ["746496145"]   # Telegram chat_id (string-encoded integer)

# Optional: enable forum-topic threading
support_thread = true

# ── Transport 2: Slack (Socket Mode) ───────────────────────────
[[transport]]
id = "slack-main"
kind = "slack"
enabled = true

[transport.config]
# Slack Socket Mode requires TWO tokens:
#   app_token (xapp-…) for the WebSocket connection
#   bot_token (xoxb-…) for chat.postMessage / auth.test API calls
app_token.secret_ref = "secret:slack/secretary-app-token"
bot_token.secret_ref = "secret:slack/secretary-bot-token"

# Slack workspace tenant
team_id = "T0123ABCD"

# DM-only by default; set false to enable channel events
dm_only = true
# channels = ["C0123DEFG"]    # only used if dm_only = false

# Optional: enable Slack thread_ts threading
support_thread = true

# Per-transport allowlist NARROWS the slot-level list
allowed_users = ["U0123ABCD"]   # Slack user IDs (U…)
```

---

### Q10 — How does agent-id propagate through the rest of Makakoo?

**Decision: Three propagation rules.**

1. **Grants**: `bound_to_agent` field is mandatory on grants issued by agents;
   grants without `bound_to_agent` are machine-global (backward compat).
2. **Brain journals**: every line written by an agent is prefixed
   `[agent:<slot_id>]`. Non-agent sources (CLIs, manual edits) are unlabeled.
3. **MCP calls**: `X-Makakoo-Agent-ID` HTTP header on every inbound MCP request
   originating from a subagent process; for stdio/local MCP calls, the agent-id
   is passed via the existing `harvey_agent_id` ContextVar.

Evidence: AUDIT.md § "grant_write_access doesn't carry for which agent"
confirms the gap. VISION.md § "cross-subsystem awareness" requires agent-id in
grants, Brain journals, MCP calls, and audit logs. `structured_logger.py:50`
(`contextvars.ContextVar("harvey_agent_id")`) is the existing hook.

---

### Q11 — Which transports ship in v1? What is the multi-transport TOML schema?

**Decision: v1 ships Telegram and Slack adapters. Discord and WhatsApp are
explicit follow-on adapters, not v1 blockers.**

Multi-transport schema: see Q9 (`[[transport]]` array). Transport-specific
`[transport.config]` block holds credentials and routing fields. Secrets
precedence (highest → lowest):

1. `makakoo secret` store: `token.secret_ref = "secret:<namespace>/<key>"`
2. Environment variable: `token.env = "SECRETARY_SLACK_TOKEN"`
3. TOML inline fallback (dev-only only; writes WARNING log on load):
   `token = "<value>"`

**Env var naming for per-slot/per-transport collisions:** format is
`<MAKAKOO_AGENT_SLOT>_<TRANSPORT_ID_UPPER>_<FIELD_UPPER>`. Example:
`SECRETARY_TELEGRAM_MAIN_TOKEN` for the `secretary` slot's `telegram-main`
transport's token field. All-caps to match shell convention.

Evidence: OPENCLAW-REFERENCE.md § Secrets layering uses the same precedence
ladder. `ChannelSecretsAdapter.secretTargetRegistryEntries`
(`openclaw/src/channels/plugins/types.adapters.ts`) is the reference interface.

---

### Q12 — Adopt OpenClaw SDK or rebuild in Rust?

**Decision: rebuild the channel abstraction in Rust, using OpenClaw's interface
shape as the contract. Do not add Node.js as a Makakoo core dependency.**

No Node.js runtime dependency. OpenClaw's TypeScript contract (especially
`ChannelGatewayAdapter` with ~15 optional adapters) is translated into Rust
trait modules. **Intentionally deferred in v1:** approval-native runtime,
exec-native delivery, OAuth user-token flows, conversation bindings
(ACP-spawn), streaming adapters, and `ChannelDirectoryAdapter` (sender
display-name resolution). These require deeper OpenClaw interop and are
flagged in the risk register.

Rust trait modules to implement:

| OpenClaw interface | Rust trait module | v1 status |
|---|---|---|
| `ChannelConfigAdapter` | `makakoo-core/src/transport/config.rs` | ✓ |
| `ChannelSecretsAdapter` | `makakoo-core/src/transport/secrets.rs` | ✓ |
| `ChannelGatewayAdapter` | `makakoo-core/src/transport/gateway.rs` | ✓ |
| `ChannelOutboundAdapter` | `makakoo-core/src/transport/outbound.rs` | ✓ |
| `ChannelStatusAdapter` | `makakoo-core/src/transport/status.rs` | ✓ |
| `ChannelPairingAdapter` | `makakoo-core/src/transport/pairing.rs` | ✓ |
| `ChannelDirectoryAdapter` | `makakoo-core/src/transport/directory.rs` | deferred |
| `ChannelApprovalAdapter` | `makakoo-core/src/transport/approval.rs` | deferred |
| `ChannelMessagingAdapter` | `makakoo-core/src/transport/messaging.rs` | deferred |
| `ChannelThreadingAdapter` | `makakoo-core/src/transport/threading.rs` | deferred |

Evidence: OPENCLAW-REFERENCE.md § "Adopt the ChannelPlugin shape as the
contract." The Rust rebuild avoids Node.js dependency while preserving the
abstraction. Risk: the full OpenClaw surface (~15 adapters) requires
translating TypeScript optional-chaining patterns into Rust trait bounds,
which may surface API gaps. Mitigation: v1 ships 5 of ~15 adapters; deferred
adapters are listed explicitly above.

---

## Multi-transport concurrency model

A single slot may have N transports attached (e.g. Telegram + Slack in the
example above). When two transports for the same slot deliver messages
concurrently:

1. Each transport's Rust task (Telegram `getUpdates` poller / Slack Socket
   Mode WebSocket listener) receives its inbound message and constructs a
   `MakakooInboundFrame` per the Phase 1 schema. Each frame carries
   `transport_id` (PRIMARY routing key), `transport_kind` (type dispatch),
   `account_id` (auxiliary diagnostic), `tenant_id` (Slack only),
   `conversation_id`, `thread_id` + `thread_kind`, `sender_id`, and
   `message_id`.
2. Both frames carry the same `slot_id` (each transport task knows its own
   slot from its spawn context — no per-message lookup) but different
   `transport_id` and sender metadata.
3. Both frames are written newline-delimited (`\n`) to the slot's
   Unix-domain IPC socket. tokio's per-stream write mutex serialises
   concurrent writers so frames never interleave on the wire.
4. The Python gateway reads frames into a SINGLE asyncio queue per slot
   (NOT per transport — one queue per slot ensures all transports
   serialise through the same LLM dispatcher). Dequeued frames dispatch
   to the LLM sequentially. No parallelism within a slot. This keeps the
   "one process pair per slot" rule intact and avoids thread-safety
   issues in the Python gateway.
5. Replies are sent back over the IPC socket as `MakakooOutboundFrame`
   objects. The Rust router demultiplexes by `outbound.transport_id`
   (PRIMARY key — must match an inbound frame's `transport_id` from the
   same slot, else reject as cross-transport per Q1 forbid rule). The
   selected adapter coerces `reply_to_message_id` to its native type
   (Telegram int / Slack thread_ts) and forwards the payload.
6. **Python gateway crash recovery:** if the Python gateway process dies,
   the Rust transport tasks detect a broken pipe on the next write,
   structured-log the failure, drop the in-flight frame (at-most-once
   per Phase 1 IPC delivery semantics), and enter exponential-backoff
   reconnect (initial 500 ms, cap 30 s, jittered). Inbound messages
   received during the gateway's downtime are dropped — no buffering,
   no replay. When the gateway restarts, the next inbound is delivered
   normally.
7. **Single-transport failure isolation:** if the Slack Socket Mode
   WebSocket drops, the Telegram task continues uninterrupted. The
   Python gateway, receiving no Slack frames, processes Telegram frames
   normally. Slack reconnect happens inside the Slack adapter's loop
   without restarting the slot's process pair.

This model is explicitly designed so that replacing `transport.kind = "telegram"`
with `transport.kind = "slack"` does not change the concurrency semantics.

---

## Olibia migration (explicit)

The legacy Olibia bot (single Telegram bot at `~/MAKAKOO/data/chat/config.json`)
migrates to a new slot `harveychat` with the following rules:

- Token: preserved from `data/chat/config.json`; moved to
  `~/MAKAKOO/config/agents/harveychat.toml` under `[[transport]]`.
- Allowlist: `allowed_users` in TOML, same numeric chat_ids.
- Persona: `persona = null` → inherits `HARVEY_SYSTEM_PROMPT` fallback
  (HARVEY_SYSTEM_PROMPT is NOT removed; it is the fallback for `persona = null`).
- Conversation DB: `data/chat/conversations.db` →
  `data/agents/harveychat/conversations.db`.
- LaunchAgent plist: regenerated from template with `MAKAKOO_AGENT_SLOT=harveychat`.
- The bot responds without manual reconfiguration after migration.

Migration is tested in Phase 2 as a round-trip: JSON → TOML → read-back.

---

## Phases

### Phase 0: Finalize transport-agnostic design

**Goal:** Lock Q1–Q12 and make the sprint document validator-ready.

**Criteria:**
- Every Q1–Q12 decision is explicit, evidence-backed, and transport-agnostic.
- The design passes the smell test: replacing `transport.kind = "telegram"` with
  `transport.kind = "slack"` does not invalidate the schema, routing, identity,
  scoping, or concurrency model.
- v1 scope is limited to Telegram and Slack adapters. Discord and WhatsApp are
  follow-on adapters documented in `docs/roadmap/adapters.md`.
- OpenClaw is used as a reference contract, not a runtime dependency.
- No placeholder text or prose ellipsis tokens remain.
- All phase numbering uses `MAKAKOO_AGENT_SLOT` consistently (not `AGENT_SLOT_ID`).
- Phase numbering rationale is documented in the sprint header.
- Every claim cites a source document (AUDIT.md, VISION.md, OPENCLAW-REFERENCE.md,
  or a specific file:line in the codebase).

**Files:**
- `SPRINT.md`
- `AUDIT.md`
- `VISION.md`
- `OPENCLAW-REFERENCE.md`

**Tests:**
- Run `lope review "$(cat SPRINT.md)" --validators pi,gemini` against this
  sprint document.
- Address every `REQUIRED_FIX` line by line before proceeding to Phase 1.

---

### Phase 1: Rust transport abstraction and IPC core

**Goal:** Add a Rust transport layer that can receive messages from Telegram
and Slack and forward normalized frames to the Python gateway over a
Unix-domain socket.

**Criteria:**
- `Transport` trait hierarchy at `makakoo-core/src/transport/` mirrors OpenClaw's
  gateway, outbound, config, secrets, status, and pairing responsibilities (see
  Q12 trait table). Telegram adapter and Slack adapter both implement the trait.
- Telegram adapter: credential verification via Bot API `getMe`; inbound via
  `getUpdates` long polling.
- Slack adapter: credential verification via `auth.test`; inbound via Socket Mode
  (app-level token, not user-token OAuth — suitable for local workstation
  dogfood without a public webhook endpoint). Events API `message.im` and
  `message.channel` events are handled; `app_mention` events deferred to Phase 3.
- **Inbound IPC frame (`MakokooInboundFrame`):**
  ```
  slot_id: String             # resolved by router from (transport_id, account_id)
  transport_kind: String     # "telegram" | "slack"
  transport_id: String       # e.g. "telegram-main" — the TOML transport.id
  account_id: String         # bot token account identifier
  conversation_id: String    # transport-native conversation identifier
  thread_id: Option<String>  # transport-native thread/topic ID (None if flattened)
  sender_id: String          # transport-native sender ID (canonical; numeric string for Telegram)
  sender_username: Option<String>  # display name; OPTIONAL; derived asynchronously
                                   # via ChannelDirectoryAdapter.self() after the
                                   # raw inbound frame is dispatched; NOT in raw frame
  text: String
  timestamp: DateTime<Utc>   # Makakoo local-receive timestamp (clock at time of
                              # deserialization in the Rust transport process, not
                              # the transport server's reported timestamp — this
                              # ensures consistent ordering across multi-transport slots
                              # where the same event may be reported by two platforms
                              # with different server clocks)
  raw_metadata: Map<String, Value>  # transport-native extras for debugging
  ```
  Note: `sender_username` is derived asynchronously via the directory adapter.
  The raw inbound frame does NOT include it. It is added as a derived field
  after the `ChannelDirectoryAdapter.self()` lookup completes and the Python
  gateway has access to the resolved display name for the system prompt.
  Note: `timestamp` uses Makakoo's local-receive clock for multi-transport
  ordering consistency. If the transport provides a server timestamp, it
  appears in `raw_metadata` alongside the local timestamp.
- **Outbound IPC frame (`MakokooOutboundFrame`):**
  ```
  transport_kind: String
  transport_id: String    # the TOML transport.id (primary routing key)
  to: String             # recipient identifier (transport-native)
  thread_id: Option<String>
  text: String
  reply_to_id: Option<String>
  ```
- Secrets precedence: `makakoo secret` store (`token.secret_ref`), then
  `token.env = "ENV_VAR_NAME"`, then TOML inline `token = "..."` (dev-only,
  logs WARNING). See Q11 TOML example and Q11 env var naming convention.
- Router: `(transport_id) → slot_id` resolution using the registry index.
  Thread-id is NOT part of the primary routing key — it is carried in the
  frame. Thread-aware delivery uses `thread_id` from the inbound frame to
  select the correct outbound `thread_id` (see `support_thread` in Q9).
- IPC: Unix-domain socket (`tokio::net::UnixStream`) for Rust-to-Python frame
  exchange. Frames are JSON-serialized `MakokooFrame` enum (inbound + outbound
  variants). The socket path is
  `~/MAKAKOO/run/agents/<slot_id>/ipc.sock` (created on first use).
- Multi-transport slot: the Rust router sends all frames for a given `slot_id`
  to the same Unix socket; Python gateway dispatches sequentially via `asyncio`
  queue (see concurrency model above).

**Files:**
- `makakoo-core/src/transport/mod.rs` — trait hierarchy + frame types
- `makakoo-core/src/transport/telegram.rs` — Telegram adapter
- `makakoo-core/src/transport/slack.rs` — Slack adapter (Socket Mode)
- `makakoo-core/src/transport/router.rs` — transport→slot resolution
- `makakoo-core/src/transport/secrets.rs` — secrets precedence adapter
- `makakoo-core/src/transport/config.rs` — TOML config loading
- `makakoo-core/src/transport/outbound.rs` — outbound frame adapter
- `makakoo-core/src/transport/status.rs` — transport status adapter
- `makakoo-core/src/transport/pairing.rs` — allowlist adapter
- `makakoo-core/src/ipc/mod.rs` — IPC frame types (`MakokooFrame` enum)
- `makakoo-core/src/ipc/unix_socket.rs` — Unix-domain socket bridge
- `makakoo-core/Cargo.toml`

**Tests:**
- Unit: Telegram credential verification with mocked `getMe` HTTP response.
- Unit: Slack credential verification with mocked `auth.test` response.
- Unit: Slack Socket Mode event deserialization (`message.im` and `message.channel`).
- Unit: router resolution from `transport_id` to `slot_id` (uses registry index).
- Unit: multi-transport slot routes Telegram DM and Slack DM to same `slot_id`
  with distinct `transport_kind` and `transport_id` in frame.
- Unit: `sender_username` is absent from raw inbound frame (directory lookup
  is a separate step in Phase 3).
- Integration: Rust-to-Python IPC frame parsing over Unix-domain socket.
- `cargo test --workspace`

---

### Phase 2: Agent registry, schema, CLI lifecycle, and migration

**Goal:** Make subagents first-class Makakoo OS objects with TOML registry,
CLI commands, and harveychat migration.

**Criteria:**
- `~/MAKAKOO/config/agents/` created on first use.
- One TOML file per slot (schema: see Q9 TOML example with Telegram + Slack).
- `makakoo agent list` shows configured slots, status, and transport kinds.
  Status is UNCONFIGURED if no `[[transport]]` block is enabled.
- `makakoo agent show <slot>` redacts secrets (`token`, `token.secret_ref`,
  `app_token`, `bot_token`, `app_token.secret_ref`, `bot_token.secret_ref`).
- `makakoo agent create <slot>` supports flags for persona, paths, tools,
  allowed users, and multiple `[[transport]]` blocks.
- `makakoo agent create` validates credentials via the Rust transport adapter
  (calls `getMe` for Telegram, `auth.test` for Slack) BEFORE writing any files.
- harveychat migration: reads `~/MAKAKOO/data/chat/config.json`, writes
  `~/MAKAKOO/config/agents/harveychat.toml`, archives old config, moves
  `conversations.db`. Idempotent on re-run.
- Slack Socket Mode for dogfood: Phase 2 uses Socket Mode app-level tokens
  (no public webhook required). Production path (webhook + cloudflare tunnel)
  is documented in `docs/roadmap/adapters.md` as a Phase 4 follow-on.
- Duplicate slot rejection: TOML filename must match `slot_id` field; rejects
  if file already exists.

**Files:**
- `makakoo-core/src/agent/schema.rs` — `AgentSlot`, `TransportEntry`,
  `TransportKind`, `TelegramTransport`, `SlackTransport` types
- `makakoo-core/src/agent/registry.rs` — TOML load/save, validation, slot index
- `makakoo-core/src/agent/migrate/harveychat.rs` — JSON→TOML migration
- `makakoo-core/src/cli/agent.rs` — `list`, `show` subcommands
- `makakoo-core/src/cli/agent_create.rs` — `create` wizard
- `makakoo-core/templates/agent-plist.plist` — LaunchAgent template with
  `MAKAKOO_AGENT_SLOT` env var injected
- `makakoo-core/templates/agent-systemd.service` — systemd template (Linux;
  not actively tested unless Linux CI is added)

**Tests:**
- Unit: TOML parse and validation (valid/invalid schema, duplicate slot).
- Unit: secret redaction in `agent show` (token absent from output).
- Unit: harveychat JSON→TOML round-trip (write → read → compare fields).
- Unit: Slack credential verification with mocked `auth.test` response (mirrors
  Phase 1 Telegram test for completeness; the Slack adapter ships in Phase 1,
  Phase 2 ships the test coverage).
- Unit: `agent create` refuses invalid Telegram token (mocked `getMe` returns
  error) before writing files.
- Unit: `agent create` refuses invalid Slack token (mocked `auth.test` returns
  error) before writing files.
- Integration: `agent create harveychat` from existing `data/chat/config.json`
  produces valid `harveychat.toml`; migrated Olibia responds without manual
  reconfiguration.
- `cargo test --workspace`

---

### Phase 3: Per-agent identity, scoping, and shared resources

**Goal:** Each running subagent knows its slot id, renders the correct persona,
and is restricted to its declared tools and paths.

**Criteria:**
- Python gateway loads slot config from `MAKAKOO_AGENT_SLOT` env var (injected
  by plist) or `--slot` CLI flag; resolves `~/MAKAKOO/config/agents/<slot>.toml`.
  If unset, falls back to legacy `data/chat/config.json` for backward compat
  only.
- System prompt includes: canonical bootstrap + per-agent persona snippet +
  identity block: *"You are `<name>`. Your slot id is `<slot_id>`. This
  message arrived via `<transport_kind>`. Your allowed tools are `<tools>`.
  Your allowed paths are `<paths>`."*
- `HARVEY_SYSTEM_PROMPT` is NOT removed — it is the fallback for
  `persona = null` (existing CLIs, headless agents). Verified at
  `plugins-core/lib-harvey-core/src/core/chat/bridge.py:78`.
- Tool dispatcher enforces per-agent allowed-tools whitelist; out-of-scope
  calls return structured `tool not in scope`.
- File writes enforce `allowed_paths` first, then `forbidden_paths` overrides,
  then `bound_to_agent` grants.
- Brain journal writes from agents prepend `[agent:<slot_id>]` prefix.
- Concurrent agent journal writes use file locking (`fcntl.flock`).
- Conversations.db becomes per-agent:
  `~/MAKAKOO/data/agents/<slot_id>/conversations.db`.
- MCP HTTP calls: `X-Makakoo-Agent-ID` header forwarded through
  `makakoo-mcp/src/http_server.rs`; stdio MCP calls read `MAKAKOO_AGENT_SLOT`
  and set `harvey_agent_id` ContextVar.
- `allowed_users` enforcement: check against canonical sender_id for the
  transport (see Q7 identity edge cases). Telegram uses numeric chat_id;
  Slack uses Slack user ID.
- Cross-reference (do not re-state) Decision 6: per-agent tool and path
  scoping uses the least-privilege default established in Q6.

**Files:**
- `plugins-core/lib-harvey-core/src/core/chat/bridge.py` — persona renderer
  reads slot config; adds identity block layer; sets `harvey_agent_id`
  ContextVar from `MAKAKOO_AGENT_SLOT`
- `plugins-core/lib-harvey-core/src/core/chat/gateway.py` — reads
  `MAKAKOO_AGENT_SLOT`; sets per-agent db path; single asyncio queue per
  slot for sequential LLM dispatch (multi-transport frames merge into the
  same queue per the concurrency model)
- `plugins-core/lib-harvey-core/src/core/chat/tool_dispatcher.py` — whitelist
  enforcement
- `plugins-core/lib-harvey-core/src/core/chat/brain_sync.py` — `[agent:<id>]`
  prefix; file-lock journal writes
- `makakoo-core/src/agent/scope.rs` — `check_tool()`, `check_path()`,
  `bound_to_agent` grant filtering
- `makakoo-core/src/agent/identity.rs` — slot config loading from TOML;
  env var reading
- `makakoo-mcp/src/http_server.rs` — `X-Makakoo-Agent-ID` forward
- `makakoo-mcp/src/dispatch.rs` — `harvey_agent_id` ContextVar from header/env

**Tests:**
- Unit: out-of-scope tool rejection (tool not in whitelist → structured error).
- Unit: `forbidden_paths` overrides `allowed_paths` (e.g. `~/Shared/` in
  allowed but `~/CV/` in forbidden → reject `~/CV/file`).
- Unit: `bound_to_agent=career` grant is invisible to `olibia` slot.
- Unit: `allowed_users` rejects Slack DM from non-allowlisted user ID.
- Unit: `allowed_users` rejects Telegram message from non-allowlisted chat_id.
- Integration: two agents write to today's journal simultaneously without
  corruption (file lock + read-back verification).
- Integration: Olibia interprets "give yourself access to ~/Shared/" as
  `bound_to_agent="harveychat"` (her own slot), not as a third party.
- `cargo test --workspace`

---

### Phase 4: Multi-transport dogfood and release hardening

**Goal:** Prove the system works end-to-end with multiple agents and at least
one agent attached to two transports.

**Criteria:**
- Three agent slots run simultaneously: `olibia` (Telegram-only, migrated from
  `data/chat/config.json`), `secretary` (Telegram + Slack, created new),
  `career` (Telegram-only, created new).
- `secretary` is reachable via both Telegram and Slack with distinct transport
  metadata (`transport_kind`, `transport_id`, `conversation_id`) in each frame;
  replies go back to the originating transport/thread. Cross-reference Decision 6
  (least privilege) rather than re-stating it.
- Each agent replies with its configured persona and scoped tool surface.
- Crash of one agent process pair does not affect other agents.
- `makakoo agent status <slot>` reports: transport runtimes (per transport
  adapter status from `ChannelStatusAdapter`), Python gateway alive/dead,
  last inbound message time (from `timestamp` field in inbound frame), and
  crash state (pid file stale or exit code non-zero).
- Existing Olibia bot continues working after migration without manual
  reconfiguration.
- `docs/roadmap/adapters.md` documents: Slack Events API production path
  (webhook + cloudflare tunnel), Discord and WhatsApp follow-on adapters.

**Files:**
- `makakoo-core/src/agent/status.rs` — status aggregation across transports
- `makakoo-core/src/agent/process_manager.rs` — supervised spawn with
  `MAKAKOO_AGENT_SLOT` env var; restart on crash
- `makakoo-core/src/transport/router.rs` — verified routing to same `slot_id`
  from multiple `transport_id` values
- `plugins-core/lib-harvey-core/src/core/chat/gateway.py` — single asyncio
  queue per slot; sequential LLM dispatch across all transports for the slot
- `docs/user-manual/agent.md` — CLI reference
- `docs/walkthroughs/multi-transport-subagents.md` — end-to-end guide
- `docs/roadmap/adapters.md` — Slack webhook production path, Discord/WhatsApp

**Tests:**
- Live dogfood: `olibia`, `secretary`, `career` slots running simultaneously.
- Live dogfood: `secretary` receives Telegram DM and Slack DM, replies to each
  within 5 seconds, with distinct `transport_kind` and `transport_id` logged.
- Smoke test: send one message per transport to `secretary`, verify
  `transport_kind` differs in logs but `slot_id` is identical.
- Fault test: SIGTERM one agent pair, verify other two continue responding.
- Regression test: migrated Olibia responds without manual reconfiguration.
- `cargo test --workspace`

---

## Acceptance criteria

The sprint is done when:

1. `makakoo agent create <name>` works end-to-end in <30 s (Telegram bot token
   already created via @BotFather; Slack app already created in workspace).
2. Three subagents simultaneously running, each responding on its own bot,
   with isolated personas + tools.
3. Per-agent grant scoping enforced: `agent-career` cannot exercise a grant
   bound to `agent-olibia`.
4. Each agent's journal lines carry `[agent:<id>]` prefix; CLI-initiated
   entries do not.
5. `makakoo agent list` shows live status for every slot in the registry.
6. `secretary` replies to both Telegram and Slack DMs; logs show distinct
   `transport_kind` per message.
7. The original Olibia / harveychat bot continues to work after migration
   without reconfiguration.
8. Agent A can see Agent B's `[agent:<id>]` journal lines when searching
   Brain (cross-agent journal visibility preserved from VISION.md).

## Non-goals

- Discord / WhatsApp adapters (explicit follow-on adapters; not v1 blockers;
  documented in `docs/roadmap/adapters.md`).
- Multi-user-per-agent ACLs beyond username/sender-id allowlist.
- Voice mode, video, advanced media.
- Cross-agent delegation ("@Olibia, ask @CareerBot …").
- Admin management bot (@MakakooMgrBot).
- Telegram token revocation via API (`deleteMyBot` does not exist; deferred).
- Linux systemd unit delivery (templates included but not actively tested
  unless Linux CI is added).
- Error-rate tracking in `makakoo agent status` (polling latency only).
- OAuth user-token flows for Slack (v1 ships bot-token only; Socket Mode).
- Full `ChannelDirectoryAdapter` implementation (sender_username resolution is
  deferred to Phase 4 follow-on; `sender_username` is absent from raw inbound
  frame until async lookup completes).

## Estimated cost

| Phase | Focus | Est. LOC | Notes |
|-------|-------|----------|-------|
| 0 | Lock Q1–Q12 | 0 | Validator review only |
| 1 | Rust transport abstraction + IPC | ~650 | Telegram + Slack adapters; frame types; IPC socket |
| 2 | Registry, schema, CLI, migration | ~500 | TOML, CLI, harveychat migration; Slack tests |
| 3 | Per-agent identity + scoping | ~450 | Tool/Path enforcement, grants, Brain, MCP |
| 4 | Multi-transport dogfood + hardening | ~200 | Live test + status + docs |

**Total: ~1 800 LOC + tests + docs; ~6 working days.**

Phase boundaries are hard cut lines. If Phase 1 blows past 2× estimated LOC,
stop and re-negotiate scope before continuing to Phase 2.

## Risk register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| OpenClaw interface surface (~15 adapters) too large for v1 | Medium | Medium | Deferred adapters table (Q12); v1 ships 5/15 adapters; `ChannelDirectoryAdapter` deferred |
| Slack Socket Mode insufficient for production | Low | Medium | Docs flag webhook + tunnel path; Phase 4 dogfood uses Socket Mode |
| `sender_username` async lookup adds latency | Low | Low | Optional field; not blocking for v1; Phase 3 derives it post-frame |
| Per-slot IPC demux complexity (asyncio sequential dispatch) | Medium | Medium | Sequential dispatch avoids Python gateway thread-safety issues; documented in concurrency model |
| Token sprawl (N slots × N transports) | Medium | Low | `makakoo secret` store is first-class; env var naming is namespaced per Q11 |
| Rust trait translation of OpenClaw TypeScript optional adapters | Medium | Medium | Deferred adapters table; v1 ships concrete adapters (Telegram, Slack); optional adapter gaps found in Phase 1 integration test |
