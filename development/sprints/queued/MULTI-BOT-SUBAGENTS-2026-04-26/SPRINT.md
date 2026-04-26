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

**Per-transport validation rules (Phase 2 enforces during `agent create`):**

| Field | Telegram (req) | Telegram (opt) | Slack (req) | Slack (opt) |
|---|---|---|---|---|
| `id` | yes (slot-unique) | — | yes (slot-unique) | — |
| `kind` | `"telegram"` | — | `"slack"` | — |
| `enabled` | — | bool, default `true` | — | bool, default `true` |
| `token` | yes (secret_ref / env / inline) | — | — | — |
| `app_token` | — | — | yes (secret_ref / env / inline) | — |
| `bot_token` | — | — | yes (secret_ref / env / inline) | — |
| `team_id` | — | — | yes | — |
| `allowed_users` | — | list[string]; intersect rule | — | list[string]; intersect rule |
| `support_thread` | — | bool, default `false` | — | bool, default `false` |
| `dm_only` | — | — | — | bool, default `true` |
| `channels` | — | — | — | list[string], required if `dm_only = false` |
| `poll_interval_ms` | — | int, default `1000` | — | — (Socket Mode WebSocket) |

`makakoo agent create` MUST run the per-transport credential verifier
BEFORE writing any files: `getMe` for Telegram, `auth.test` for Slack
(bot token), Socket Mode probe for Slack (app token). For Slack, the
`team_id` returned by `auth.test` MUST match the TOML `team_id` (reject
on mismatch).

**Same-kind multi-transport validation:** two `[[transport]]` blocks of
the same `kind` are permitted iff their resolved credential identities
differ. Phase 2 schema validation rejects:
- duplicate `transport.id` within a slot (any kind);
- two Telegram transports whose `getMe.id` resolves to the same bot;
- two Slack transports with the same `bot_token` in the same `team_id`
  (same `bot_token` in DIFFERENT `team_id` is permitted — it's a
  different bot identity in a different workspace).

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
   same slot, else reject as cross-transport per the Phase 1 IPC spec
   "Cross-transport reply (locked v1, FORBIDDEN)" rule). The
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
- **Inbound IPC frame (`MakakooInboundFrame`):**
  ```
  slot_id: String                 # set by the transport task from spawn ctx
  transport_id: String             # PRIMARY routing key, e.g. "telegram-main"
  transport_kind: String           # "telegram" | "slack" — type dispatch
  account_id: String               # auxiliary diagnostic; resolved bot identity
                                   #   Telegram: getMe.id; Slack: auth.test.bot_id
  tenant_id: Option<String>        # Slack: team_id; Telegram: None
  conversation_id: String          # WHERE to reply
                                   #   Telegram: chat_id (string-encoded int)
                                   #   Slack DM: im_id (D…); Slack channel: C…
  thread_id: Option<String>        # transport-native thread token
  thread_kind: Option<String>      # discriminator: "telegram_topic" |
                                   #   "slack_thread_ts" | None (flat / DM)
  sender_id: String                # canonical sender used for ACL
                                   #   Telegram: chat_id; Slack: user_id (U…)
  message_id: String               # transport-native id of THIS message
                                   #   Telegram: message_id (int as string)
                                   #   Slack: event_ts / ts (float-string)
  text: String
  received_at: DateTime<Utc>       # Makakoo local-receive clock (NOT transport
                                   # server clock) — ensures consistent ordering
                                   # across multi-transport slots
  raw_metadata: Map<String, Value> # transport-native extras for debugging;
                                   # carries the transport server timestamp if
                                   # available (e.g. raw_metadata["server_ts"])
  ```
  `sender_username` is intentionally NOT in the v1 frame schema. Directory-
  adapter lookup (`ChannelDirectoryAdapter.self()`) is post-v1 (see Q12
  deferred adapters). Until then, downstream code uses `sender_id` for both
  ACL and display.
- **Outbound IPC frame (`MakakooOutboundFrame`):**
  ```
  transport_id: String             # MUST match the inbound frame's transport_id
                                   # for the same slot — cross-transport reply
                                   # is forbidden in v1 (rejected at router)
  transport_kind: String           # type dispatch for adapter selection
  conversation_id: String          # WHERE to send (channel/im id, NOT user id)
  thread_id: Option<String>        # only honored when support_thread = true
                                   # for the matching transport
  thread_kind: Option<String>      # MUST match the inbound thread_kind if both
                                   # are set; mismatch → adapter logs WARN and
                                   # drops thread (still sends to conversation)
  text: String
  reply_to_message_id: Option<String> # transport-native reply target;
                                      # the demux layer COERCES this to the
                                      # transport's expected type (Telegram int
                                      # parsed from numeric string; Slack
                                      # thread_ts as float-string).
                                      # Format mismatch → drop reply_to (still
                                      # send the message, no thread anchor) +
                                      # WARN log
  ```
- Secrets precedence (locked, identical for every adapter):
  1. `makakoo secret` store: `<field>.secret_ref = "secret:<ns>/<key>"`
  2. Environment variable: `<field>.env = "ENV_VAR_NAME"`
  3. TOML inline (dev-only, adapter logs WARNING): `<field> = "<value>"`
- **Router: `(slot_id, transport_id) → adapter` is the PRIMARY mapping**
  used for outbound demux. Inbound resolution: each transport poller knows
  its own `(slot_id, transport_id)` from the spawn context (no per-message
  registry lookup). The triple `(transport_kind, account_id, tenant_id)` is
  used only for cross-checking — e.g., rejecting a Slack event whose
  `team_id` doesn't match the transport's TOML.
- **IPC framing protocol (locked):** newline-delimited JSON
  (`\n`-terminated). Each side reads/writes one complete JSON object per
  line. Outer envelope: `{ "kind": "inbound" | "outbound", "frame": {…} }`.
  Length-prefix is rejected as overkill for v1 (frames are bounded by
  Telegram/Slack message limits). tokio's per-stream write mutex serialises
  concurrent writers so frames never interleave on the wire.
- **IPC delivery semantics (locked v1):**
  - **At-most-once.** No frame ack, no IPC-layer retry.
  - If the Python gateway socket is unavailable when the Rust transport
    receives an inbound message, the transport logs a STRUCTURED ERROR
    (`{"event": "ipc.gateway_unavailable", "transport_id": …, "drop": true}`)
    and drops the frame, then enters exponential-backoff reconnect
    (initial 500 ms, cap 30 s, jittered) for the next inbound. No buffering,
    no replay.
  - Outbound transport API (Telegram/Slack) failures: Rust adapter logs
    the failure and returns the error to Python via a synchronous response
    on the same Unix socket. No transport-side retry.
- **Cross-transport reply (locked v1, FORBIDDEN):** outbound `transport_id`
  MUST equal the `transport_id` of an inbound frame for the same `slot_id`.
  The router rejects mismatches at the demux layer before invoking the
  adapter. (Cross-transport messaging — e.g. "send results via email
  instead" — is a follow-on capability.)
- IPC socket path: `~/MAKAKOO/run/agents/<slot_id>/ipc.sock` (created on
  first use; world-readable for the local user only — `0700` on the
  parent dir).
- Multi-transport slot: all transports for a slot write to the SAME Unix
  socket. Python gateway reads newline-delimited frames into a SINGLE
  asyncio queue per slot (see concurrency model section).

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
- `makakoo-core/src/ipc/mod.rs` — IPC frame types (`MakakooFrame` enum)
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
- Unit: TOML parse and validation (valid/invalid schema, duplicate slot
  filename).
- Unit: secret redaction in `agent show` (token absent from output).
- Unit: harveychat JSON→TOML round-trip (write → read → compare fields).
- Unit: Slack credential verification with mocked `auth.test` response
  (mirrors Phase 1 Telegram test for completeness; the Slack adapter ships
  in Phase 1, Phase 2 ships the test coverage).
- Unit: Slack `team_id` mismatch between TOML and `auth.test.team_id` is
  rejected during `agent create`.
- Unit: `agent create` refuses invalid Telegram token (mocked `getMe`
  returns error) before writing files.
- Unit: `agent create` refuses invalid Slack token (mocked `auth.test`
  returns error) before writing files.
- Unit: same-kind multi-transport guard — TOML with two `[[transport]]`
  blocks of `kind = "telegram"` and DUPLICATE `transport.id` is rejected
  with "duplicate transport.id" error.
- Unit: same-kind multi-transport guard — TOML with two `kind = "telegram"`
  transports having distinct IDs but IDENTICAL bot tokens (resolved
  `getMe.id` matches) is rejected with "duplicate bot identity" error.
- Unit: same-kind multi-transport guard — TOML with two `kind = "slack"`
  transports in the same `team_id` with the same `bot_token` is rejected;
  same `bot_token` in DIFFERENT `team_id` is permitted.
- Unit: same-kind multi-transport HAPPY path — two distinct Telegram bots
  (different tokens, different IDs) on one slot are accepted; both
  registered in router.
- Integration: `agent create harveychat` from existing
  `data/chat/config.json` produces valid `harveychat.toml`; migrated
  harveychat (display "Olibia") responds without manual reconfiguration.
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
- Unit: `bound_to_agent=career` grant is invisible to `harveychat` slot.
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

**Naming convention (locked):** the slot id of the migrated legacy bot is
`harveychat`; its display name (the TOML `name` field) is `Olibia`.
Throughout Phase 4 docs, tests, criteria, and live dogfood the slot id is
`harveychat` — never `olibia`. "Olibia" appears only as the display name
in user-facing surfaces (Telegram avatar, `makakoo agent list` Name column).

**Criteria:**
- Three agent slots run simultaneously: `harveychat` (Telegram-only, migrated
  from `data/chat/config.json`; display name "Olibia"), `secretary`
  (Telegram + Slack, created new), `career` (Telegram-only, created new).
- `secretary` is reachable via both Telegram and Slack with distinct
  `transport_id` in each frame; replies go back to the ORIGINATING
  `transport_id` (cross-transport reply forbidden — see Phase 1 IPC spec).
- Each agent replies with its configured persona and scoped tool surface.
- Crash of one agent process pair does not affect other agents.
- `makakoo agent status <slot>` reports PER `transport.id`, not just per
  kind:
  ```
  secretary
    gateway:   alive   pid=12345  last_frame=2s ago
    transport telegram-main:  connected  last_inbound=8s ago  errors_1h=0
    transport slack-main:     connected  last_inbound=3m ago  errors_1h=1
  ```
  Status fields per transport: `connected | reconnecting | failed`,
  `last_inbound` timestamp, `errors_1h` count (frames dropped or send
  failures in the last hour).
- `makakoo agent validate <slot>` (NEW): runs the per-transport credential
  verifier (per Q11 validation rules) WITHOUT starting the agent. Useful
  before `start` to surface bad credentials early.
- Existing harveychat (display: "Olibia") bot continues working after
  migration without manual reconfiguration.
- `docs/roadmap/adapters.md` documents: Slack Events API production path
  (webhook + cloudflare tunnel), Discord and WhatsApp follow-on adapters,
  and the deferred OpenClaw-parity adapters from Q12.

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
- Live dogfood: `harveychat`, `secretary`, `career` slots running
  simultaneously.
- Live dogfood: `secretary` receives Telegram DM and Slack DM, replies to
  each within 5 seconds, with distinct `transport_id` logged per reply.
- Smoke test: concurrent Telegram + Slack messages to `secretary` —
  conversation history shows both turns interleaved in slot order with
  `[telegram]` / `[slack]` channel prefixes visible to the LLM.
- Smoke test: `secretary` Slack thread reply with `support_thread = true`
  goes into the originating `thread_ts`, not into the parent channel.
- Fault test: SIGTERM one agent pair (e.g. `secretary`), verify the other
  two (`harveychat`, `career`) continue responding without restart.
- Fault test: kill the Python gateway for `harveychat` — Rust transports
  reconnect with backoff; first inbound after gateway restart is delivered.
- Fault test: `makakoo agent validate secretary` with a corrupted Slack
  bot token reports the failure WITHOUT touching the running agent.
- Regression test: migrated `harveychat` responds without manual
  reconfiguration; `data/chat/config.json` is preserved (not deleted) for
  rollback safety in v1.
- `cargo test --workspace`

---

## Acceptance criteria

The sprint is done when:

1. `makakoo agent create <name>` works end-to-end in <30 s (Telegram bot token
   already created via @BotFather; Slack app already created in workspace).
2. Three subagents simultaneously running, each responding on its own bot,
   with isolated personas + tools.
3. Per-agent grant scoping enforced: `agent-career` cannot exercise a grant
   bound to `agent-harveychat`.
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
