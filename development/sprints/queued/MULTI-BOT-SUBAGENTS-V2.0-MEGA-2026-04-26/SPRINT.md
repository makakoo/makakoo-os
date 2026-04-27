# SPRINT-MULTI-BOT-SUBAGENTS-V2.0-MEGA

**Status:** Phase 0 round-4 — codex Q15 trait async-safety + pi AC item 2 transport count fixed
**Owner:** Sebastian
**Domain:** engineering
**Date:** 2026-04-26
**Prior sprint:** `sprint-multi-bot-subagents-v1` (HEAD `9b6298b`) shipped
the Rust transport substrate, IPC, registry, CLI, identity, scope, and
grants attribution. v2.0 closes the entire Phase 5 backlog plus 5 new
transports plus production-grade hardening, in one autonomous run.

---

## Origin

Sebastian green-lit the MEGA sprint after v1 shipped: *"do MEGA!!! you are
mega Harvey! so do mega! and implement future and quality! production
ready and grandma secure system harvey!"*

v1 shipped the **substrate**. v2 makes the substrate **live, complete,
and grandma-secure** — every Makakoo user can `makakoo agent create
<slot>`, attach Telegram/Slack/Discord/WhatsApp/Email/Voice/Web, and have
the bot answer in production with audited tool/path scope and
attribution.

---

## Goals

1. Make slots **actually run** end-to-end (supervisor + Python gateway).
2. Cover the **5 high-value transports** beyond Telegram/Slack:
   Discord, WhatsApp Cloud API, Email (IMAP+SMTP), Voice (Twilio),
   Web chat (WS).
3. Ship the **4 deferred OpenClaw-parity adapters** (Directory /
   Approval / Messaging / Threading) so power users can extend.
4. Unlock **per-agent LLM model override** — secretary can run on a
   flagship model while career runs on a cheap one.
5. Deliver **grandma-secure** hardening: fault injection, per-slot
   resource limits, secret hygiene, rate limiting, audit log review.
6. Land an **HTE wizard** so `makakoo agent wizard` walks any user
   through slot creation without TOML hand-editing.

---

## Locked decisions (Phase 0 round-2)

### Q1 — Supervisor process model **[locked]**

A single Rust supervisor binary (`makakoo agentd --slot <slot>`)
hosts the transport runtime in-process and spawns ONE Python gateway
child via `tokio::process::Command`. The supervisor owns
`TransportStatusHandle` clones in-process; status writer flushes
`~/MAKAKOO/run/agents/<slot>/status.json` every 5s.

Restart budget: 5 child crashes per minute → exponential backoff
500ms→30s; circuit-break to `state=crashed` after 60s of consecutive
failures (status reflects this).

**Slot name sanitization:** `slot_id` is validated at create-time
to `^[a-z][a-z0-9-]{0,31}$` (Phase 2 enforces). LaunchAgent labels
and systemd unit filenames embed the validated slot_id directly
without further escaping; reverse-DNS prefix `com.makakoo.agent.`
prevents collision with system bundles.

`makakoo agent start <slot>`:
1. Generates LaunchAgent plist (macOS) at
   `~/Library/LaunchAgents/com.makakoo.agent.<slot>.plist` and
   `launchctl bootstrap`s it. `launchctl bootstrap` returns specific
   error code 5 = "operation not permitted" → user must grant
   Files & Folders consent under System Settings → Privacy & Security.
   We **detect this** and print copy-paste remediation.
2. Linux: writes systemd user unit at
   `~/.config/systemd/user/makakoo-agent-<slot>.service` and
   `systemctl --user daemon-reload && start`.
3. **No nohup fallback.** Without launchd/systemd, the supervisor has
   no auto-restart on its own crash, which contradicts grandma-secure.
   Instead, if both fail, command exits non-zero with a clear hint.
   Headless containers must use `MAKAKOO_AGENT_SUPERVISOR=foreground`
   to run the supervisor directly without daemon registration.

`makakoo agent status <slot>` reads `status.json` only — no IPC
roundtrip, so a hung supervisor still shows `gateway: dead` based on
the stale write.

### Q2 — Python gateway boundary **[locked]**

`plugins-core/agent-harveychat/python/` ships the reference Python
gateway (in-repo path, not a cross-repo touch). It connects to the
Rust supervisor's IPC socket as a client (matching v1 Phase 1
contract). Inbound frames arrive as newline-JSON; the gateway
prefixes the Phase 3 identity block, dispatches to the LLM,
**preflight-checks** `tools` whitelist + `allowed_paths` /
`forbidden_paths`, returns outbound newline-JSON.

**Authoritative scope enforcement is the Rust MCP/grant layer.** The
Python preflight is defense-in-depth and a UX optimization (the LLM
sees a friendlier error than a 403 from the MCP tool). All scope
violations write an audit log entry from the Rust side (the source of
truth), regardless of whether the preflight caught them.

**Brain attribution:** the gateway prefixes Brain journal lines with
`[agent:<slot_id>]`. Supervisor pre-issues a write grant for
`~/MAKAKOO/data/Brain/journals/` to the gateway process with
`bound_to_agent = Some(slot_id)` so attribution is enforced even if
the gateway forgets the prefix.

**IPC contract** is materialized as
`docs/specs/ipc-contract-v2.md` before Phase 3 starts so alternate
gateways and the contract test (R1) have a concrete spec.

### Q3 — Destroy semantics **[locked]**

`makakoo agent destroy <slot>`:
1. `makakoo agent stop <slot>` (supervisor SIGTERM)
2. Move TOML to
   `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/<slot>.toml`
3. Move data dir to
   `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/data/`
4. Scan TOML for **direct** `secret_ref = "..."` literals. List them.
   **Note explicitly that secrets nested in `[transport.config]`
   sub-tables or referenced via env-var interpolation are NOT
   detected.** (`docs/troubleshooting/agents.md` explains.)
5. Prompt: "Revoke these N detected secrets too? [y/N]"  → default N
6. Print restore one-liner

**Default behavior:** secrets are PRESERVED unless `--revoke-secrets`
is explicit. `--yes` only auto-confirms the destroy itself, not
secret revocation. `--keep-secrets` is a no-op (already the default)
but accepted for explicit clarity.

Refuses to destroy `harveychat` slot without `--really-destroy-harveychat`
flag (legacy data preservation).

**Re-create after destroy:** `makakoo agent create <name>` always
creates fresh — never restores from archive. To restore, copy from
archive manually (documented in walkthrough).

### Q4 — LLM override precedence **[locked]**

Two-section schema with clear intent:

```toml
[llm.inherit]   # Optional. Documents which fields will inherit from
                # system defaults. No field values here — comment-only.
                # Present for self-documentation; loader ignores it.

[llm.override]  # Required if you want non-default behavior.
                # Any field set here overrides system defaults for this slot.
model            = "claude-opus-4-7"
max_tokens       = 8192
temperature      = 0.7
reasoning_effort = "medium"
```

Resolution order: per-call args > slot.toml `[llm.override]` > makakoo
system defaults (from `~/MAKAKOO/config/makakoo.toml`).

Validation: `agent validate` and `agent create` both call
`SwitchAILocal::list_models()` and reject unknown model ids with the
list of available models. Network-failure during validate is a
warning, not an error (offline workflow).

`makakoo agent show <slot>` displays the **effective** LLM config
with per-field source attribution:
```
llm:
  model:            claude-opus-4-7    [override]
  max_tokens:       8192               [override]
  temperature:      0.7                [override]
  reasoning_effort: medium             [override]
  top_p:            1.0                [system default]
```

### Q5 — Slack Events API webhook adapter shape **[locked]**

`[transport.config] mode = "events_api"` switches the SlackAdapter to
HTTP-webhook mode. Listens on `makakoo-mcp --http`'s axum server
under `/transport/<slot_uuid>/<transport_uuid>/events`.

**Path uses opaque v4 UUIDs for BOTH slot and transport.** Slot
creation generates a `slot_uuid` and stores it alongside `slot_id` in
slot.toml; each `[[transport]]` entry generates a `transport_uuid`
on first save. SHA-256-truncation rejected (only 64-bit collision
space). UUID v4 = 122-bit entropy. The webhook router stores the
mapping `(slot_uuid, transport_uuid) → handler` so adapters do not
need to know the path shape.

**HMAC verification:** raw body + `X-Slack-Signature` +
`X-Slack-Request-Timestamp`, replay window 5 minutes. **Verify before
JSON parse** so malformed bodies still fail with audit log entry.

**URL verification challenge** handled automatically (responds with
`{"challenge": "..."}` on `type=url_verification`).

**Outbound path** identical to Socket Mode (chat.postMessage). Both
modes share `SlackTransportShared` — refactor SlackAdapter into thin
shell.

**Blast radius accepted, documented:** A makakoo-mcp HTTP crash takes
all webhook transports (Slack Events, WhatsApp, Voice, Web) offline
simultaneously. Mitigated by: (a) makakoo-mcp runs under
launchd/systemd with auto-restart; (b) `/health` endpoint exposed for
external monitoring; (c) explicit risk-register entry; (d) per-slot
Socket-Mode fallback documented for users who need transport
isolation.

**Route isolation:** `/rpc` (Ed25519 signed) and `/transport/*`
(HMAC per-transport) live on the same axum server but are
distinct route trees with distinct auth middleware. No cross-route
authority delegation. Documented in
`docs/specs/http-server-security.md`.

### Q6 — Discord adapter **[locked]**

Library: **serenity** (mature, good DM ergonomics, used by largest
Discord bots).

Intents: `GUILDS | GUILD_MESSAGES | DIRECT_MESSAGES`.
`MESSAGE_CONTENT` is **privileged** (Discord requires verification for
bots in 100+ guilds and may reject newer applications). Default mode:
**no MESSAGE_CONTENT** — bot only sees mentions, replies, slash
commands, and DMs (where content is always available).

Optional `[transport.config] message_content = true` enables the
intent. Validate command tries to fetch a channel message at
startup; if it returns empty content despite the intent claim,
warns and falls back to mention-only mode.

`account_id = bot.user.id`. DM-vs-guild via `channel.kind == DM`.
Threads native via `thread.parent_id`.

**`guild_ids` allowlist** in `[transport.config]`: empty list = any
guild bot is invited to (default permissive); non-empty = restrict
to listed guild IDs (security-paranoid). Documented in
`docs/walkthroughs/discord-bot.md`.

### Q7 — WhatsApp Cloud API adapter **[locked]**

Meta WhatsApp Cloud API only. Bearer token in `access_token_ref`,
phone_number_id in TOML. **Webhook verify_token** required in TOML
(`verify_token_ref` → opaque random string Meta sends back during
webhook setup handshake; we echo `hub.challenge` if it matches).

Webhook lands on the same axum server under
`/transport/<slot_uuid>/<transport_uuid>/webhook`. Verifies
`X-Hub-Signature-256` over raw body. account_id = phone_number_id,
sender_id = contact's wa_id (E.164 sans +).

**Media handling:** v2.0 ships **explicit-drop** behavior — inbound
media messages return a polite reply ("I can only handle text
messages right now") to the sender. The frame still goes to the
gateway with `text = "[media: <type>]"` so the LLM knows the user
sent something. Full media support deferred to v2.1.

### Q8 — Email adapter shape **[locked]**

IMAP IDLE for inbound (push, not poll), SMTP for outbound. Per-mailbox
slot. Thread = `Message-ID` chain via In-Reply-To/References headers.

`account_id = full normalized mailbox address` (e.g.
`secretary@office.example`) — local-part alone collides across domains.

`conversation_id = root Message-ID`. `sender_id = From email address`.
Body extraction strips quoted replies via Rust-native parser
(crate: `mailparse` + custom quote-line heuristic — no Python deps).
**Raw body preserved** in `raw_metadata.body_raw` so audit and
reply-context have full text.

OAuth2 for Gmail (refresh_token_ref + client_id_ref +
client_secret_ref) — REQUIRED for Gmail. App-passwords for
non-Gmail providers via `password_ref` — **explicitly documented as
less secure** with prominent warning in walkthrough.
**Plain SMTP/IMAP without TLS rejected by validate.** STARTTLS or
implicit-TLS only.

IDLE reconnect: 25-min cap (RFC says 29min ceiling) + heartbeat
NOOP every 5min.

### Q9 — Voice adapter shape **[locked]**

Twilio Voice + TwiML, push-to-talk semantics. Locked call state
machine:

```
Inbound call → Twilio webhook POST /transport/<uuid>/<uuid>/voice
            → TwiML response: <Say>greeting</Say>
                              <Record action="/transport/<uuid>/<uuid>/voice/recording-{CallSid}"
                                      maxLength="60" />

Recording done → Twilio webhook POST /transport/<uuid>/<uuid>/voice/recording-{CallSid}
              → adapter fetches RecordingUrl with Twilio basic-auth
                (account_sid + auth_token from secrets)
              → STT via SwitchAILocal whisper-1 model
              → text frame to gateway
              → gateway returns reply text
              → TTS via SwitchAILocal openai/tts-1 (PRIMARY) →
                fallback to elevenlabs/eleven_turbo_v2 (if available)
              → upload audio to local /transport/<uuid>/audio/<sha>
              → TwiML response: <Play>https://.../audio/<sha></Play>
                              <Record ... />   (loop for next turn)

Caller hangs up → Twilio webhook POST .../voice/status
                → adapter cleans up call state
```

The recording-callback URL **embeds CallSid** in the path so
correlation is unambiguous. Initial TwiML response is dynamically
generated (not static).

**STT/TTS routing through SwitchAILocal:** confirmed both `whisper-1`
and `openai/tts-1` are in the SwitchAILocal model catalog.
Fallback chain: openai/tts-1 → elevenlabs/eleven_turbo_v2 → drop call
with `<Say>Sorry, I can't reply right now.</Say>`.

**Audio fetch authentication:** Twilio recording URLs require
`account_sid:auth_token` basic auth. Both stored in secrets;
adapter passes them via reqwest.

**Webhook signature verification:** ALL Twilio webhooks must verify
`X-Twilio-Signature` HMAC-SHA1 over the URL + sorted POST params
(per Twilio spec) before processing. Missing/invalid signature →
401 + audit log entry (`webhook.invalid_signature`). Without this,
voice webhooks are spoofable from anyone with knowledge of the URL.

`account_id = twilio account_sid`, `sender_id = caller E.164`,
`conversation_id = twilio call_sid`, `thread_kind = none`.

Realtime streaming voice deferred to v2.1.

### Q10 — Web chat adapter shape **[locked]**

WS endpoint at `/transport/<slot_uuid>/<transport_uuid>/ws` on
`makakoo-mcp --http`. Visitor session = HMAC-signed cookie (NOT
Ed25519 — overkill for short-lived session id; HMAC-SHA256 with the
makakoo HTTP server's secret).

Cookie key persisted to `$MAKAKOO_HOME/keys/web-chat-hmac` (mode 0600,
created on first start). Rotation procedure: delete the file +
restart makakoo-mcp; all active web sessions invalidate naturally.

Cookie payload = random 128-bit visitor_id, 24h TTL.
**Production:** `Secure + HttpOnly + SameSite=Strict`.
**Localhost dev:** `Secure` flag dropped if request comes from
`127.0.0.1` or `localhost` (axum extractor checks).

**Origin allowlist (CSRF defense):** `[transport.config]
allowed_origins = ["https://example.com"]` REQUIRED in production.
WS upgrade requests with missing or non-allowlisted `Origin` header
are rejected with 403 + audit log entry. Empty allowlist =
localhost-only (dev mode); production deployments MUST list explicit
origins. HMAC cookies alone do not defend against cross-site
WebSocket abuse.

WS upgrade handler **explicitly reads the `Cookie` header**
from the HTTP upgrade request (not the WS frames). Documented
in adapter code with a comment because failure is silent.

Message envelope:
- Inbound:  `{type: "msg", text: "..."}`
- Outbound: `{type: "msg", text: "...", ts: <unix_ms>}`
- Both:     `{type: "typing"}` indicator (optional, ignored by gateway)

Minimal HTML demo at `docs/walkthroughs/web-chat-demo.html` —
single static page, no build step. CSP locked.

### Q11 — Fault-injection harness **[locked]**

`makakoo agent test-faults <slot>` (gated behind
`MAKAKOO_DEV_FAULTS=1` env so prod can't trigger). All scenarios
**mock-only — no real transport credentials, no network calls**.

Scenarios:
- `gateway-sigterm` — kill Python gateway, expect supervisor restart < 5s
- `gateway-oom` — handle BOTH subcases:
  (a) gateway exits with code 137 (OOM killer SIGKILL),
  (b) gateway exits with code 1 + stderr "MemoryError" (RLIMIT_AS hit, Python catches)
  → For retry-budget-exhaustion assertion, the harness simulates 6
  consecutive crashes within 60s window (budget is 5/min) — single
  OOM event alone does not exhaust budget
- `transport-ws-drop` — close Slack/Discord WS, expect reconnect <30s
- `transport-token-revoke` — adapter receives 401, expect status=failed + slot survives
- `ipc-socket-unlink` — `rm` the IPC socket, expect adapter records `gateway_unavailable`
- `tool-scope-violation` — gateway tries `run_command` with `tools=[brain_search]`, expect ScopeError + audit log entry
- `path-scope-violation` — gateway writes `/etc/passwd`, expect ScopeError + audit log entry
- `rate-limit-burst` — 200 inbound frames in 1s, expect queue.overflow + drop_newest

Each scenario is a Rust test using `MockAdapter`/`MockGateway` and
asserts the locked observable behavior.

### Q12 — Per-slot resource limits **[locked]**

**Two distinct mechanisms, separately controlled:**

**1. RSS monitoring (always on, observation only).** Supervisor
reads child RSS every 5s, writes to status.json. CLI `agent status`
shows `rss=NNN MB`. Crossing `[resource_limits] memory_warn_mb`
(default 512MB) writes one warn to audit log per breach
(deduplicated within 5min window). RSS monitoring NEVER kills the
process — it only observes and warns.

**2. Hard rlimits (opt-in via `[agents] enforce_rlimits = true`).**
When and only when this flag is true does the supervisor call
`setrlimit` in the child's `pre_exec` closure:
- `RLIMIT_AS = [resource_limits] memory_mb` (default 1024MB)
- `RLIMIT_NOFILE = 256`
- `RLIMIT_NPROC = 64` (macOS does not enforce; documented)

When `enforce_rlimits = false` (DEFAULT), `setrlimit` is NOT called.
The OS imposes only its own ambient limits. Users measure baseline
via RSS monitoring before opting in.

**Supervisor self-limit:** ALSO opt-in via
`[agents] enforce_supervisor_rlimit = true`. Default false to avoid
self-kill under dependency spikes. When true, supervisor calls
`setrlimit(RLIMIT_AS, [agents] supervisor_memory_mb)` (default
512MB) on its own startup.

**Slot-count cap:** 32 slots per machine, configurable via
`[agents] max_slots = N`. Hard error from `agent create` if exceeded.
This is enforced at the registry level — orthogonal to rlimits and
always on.

### Q13 — Rate limiting **[locked]**

Token bucket per `(slot_id, transport_id, sender_key)` where
`sender_key` is transport-specific:
- Telegram: `user_id`
- Slack:    `user_id` (U…)
- Discord:  `user_id`
- Web:      `visitor_id` (cookie)
- WhatsApp: `wa_id`
- Email:    `From-address` (note: trivially spoofable — augmented by
  per-slot global limit)
- Voice:    `caller E.164` (single-caller floods are physically
  rate-limited by phone connection)

Default: 60 messages per 5 minutes per `sender_key`.
Configurable per-slot in `[rate_limit]` section.

**Second-tier global per-slot limit:** 600 messages per 5 minutes
total (across all senders), defends against multi-account spam on
Email/WhatsApp where sender identity is weaker.

**Webhook-verification bypass:** Slack URL verification challenges,
WhatsApp `hub.challenge`, Twilio status callbacks, etc. bypass the
rate limiter entirely (would otherwise trigger from provider's own
verification probes).

**Reply protocol on rate-limit hit:** Rust router emits frame
`{type: "system_message", text: "rate_limited", display_message:
"<slot's [rate_limit].custom_message OR default>"}`. Python gateway
relays `display_message` to the user verbatim — no LLM call.

### Q14 — Audit log **[locked]**

JSONL at `~/MAKAKOO/data/audit/agents.jsonl`, rotated at 100MB,
**total cap 1GB** across rotated files (oldest evicted on
overflow). On reaching cap:
- Write warn to stderr
- Write warn to `~/MAKAKOO/data/audit/agents.alerts.log`
  (separate file, never rotated, stays as evidence of audit pressure)
- Refuse to drop new audit lines (block briefly, retry oldest evict)

**Schema:** `{ts, slot_id, transport_id, kind, actor, target,
outcome, detail}`. **All fields typed**.

**Redaction policy (clarified):**
- **Never logged:** secret values, OAuth tokens, bearer tokens,
  message bodies (only first 200 chars truncated preview at most),
  HMAC signing keys, cookie payloads.
- **Logged as identifiers:** `actor` and `target` may contain
  user-identifying values (email address, phone number, Slack U…
  id, WhatsApp wa_id, Discord user id). These are required for
  audit forensics; treated as sensitive but not redacted.
- `secret.resolve` records only the ref name and outcome
  (`success | not_found | denied`), never the resolved value.

**alerts.log file** is bounded by separate cap (10MB total, oldest
truncated on overflow) since it's the audit-pressure escape hatch
and must remain writeable when the main audit log is itself blocked.

Expanded `kind` enum:
- `scope.tool` — tool whitelist violation
- `scope.path` — path scope violation
- `secret.resolve` — secret lookup attempt
- `grant.issue` / `grant.revoke` — UserGrant lifecycle
- `slot.create` / `slot.start` / `slot.stop` / `slot.destroy` — lifecycle
- `transport.verify` — credential verification result
- `rate.limit` — rate limit hit
- `fault.test` — fault-injection scenario triggered
- `gateway.crash` — child process crash
- `webhook.invalid_signature` — HMAC verification failure

File permissions: `0600` enforced on creation + after rotation.

`makakoo agent audit <slot> [--last 50] [--kind scope.path]
[--since 2026-04-26]` tails with filtering.
**Default CLI output redacts** `detail` fields beyond a 200-char
preview; `--full` shows full detail.

### Q15 — HTTP server scaffolding **[locked]** (new in round 2)

Phase 5 absorbs the HTTP server scaffolding work BEFORE the Slack
Events implementation. Order:

**Phase 5a:** Webhook router primitive in `makakoo-mcp/src/webhook_router.rs`:

```rust
/// All webhook bodies are pre-buffered before handler dispatch so
/// signature verification has access to raw bytes WITHOUT
/// consuming the body for the handler.
pub struct WebhookRequest {
    pub headers: HeaderMap,
    pub uri:     Uri,
    pub method:  Method,
    pub raw_body: Bytes,        // already read once, owned
    pub extensions: Extensions,
}

/// Object-safe via `#[async_trait::async_trait]`. The crate is
/// already a workspace dep (used by adapter::peer). Concrete
/// macro expansion: `handle` becomes
/// `fn handle<'a>(...) -> Pin<Box<dyn Future<Output=Response> + Send + 'a>>`,
/// which is dyn-compatible.
#[async_trait::async_trait]
pub trait WebhookHandler: Send + Sync {
    /// Called BEFORE handle(). Must verify signature/HMAC against
    /// raw_body without parsing. Returning Err short-circuits to 401
    /// + audit log entry. The trait shape forces verify-first.
    fn verify(&self, req: &WebhookRequest) -> Result<(), VerifyError>;

    /// Called only after verify() passes. Free to parse raw_body.
    async fn handle(&self, req: WebhookRequest) -> Response;
}

#[async_trait::async_trait]
pub trait WsUpgradeHandler: Send + Sync {
    /// WS upgrade requests bypass the body-buffering trait above.
    /// They get the raw upgrade request and consume it themselves.
    /// verify_upgrade() runs against the upgrade request before
    /// accepting (cookie + Origin allowlist for Web; HMAC for
    /// transports that ws-upgrade after webhook handshake).
    fn verify_upgrade(&self, req: &Request<Body>) -> Result<(), VerifyError>;

    async fn on_upgrade(&self, ws: WebSocketUpgrade) -> Response;
}
```

- `WebhookRouter::register_webhook(slot_uuid, transport_uuid, kind, Box<dyn WebhookHandler>)`
- `WebhookRouter::register_ws(slot_uuid, transport_uuid, kind, Box<dyn WsUpgradeHandler>)`
- Mount on `/transport/<slot_uuid>/<transport_uuid>/<kind>/...`
- Per-handler audit + rate-limit middleware (verification bypass per Q13)
- `/health` endpoint exposed at root
- **Graceful shutdown:** on SIGTERM, router stops accepting new
  requests, drains in-flight HTTP requests with 30s timeout, drops
  in-flight WS / Twilio recording sessions with explicit warn log
  (the alternative — waiting for live calls to end — could block
  shutdown indefinitely). Documented in
  `docs/specs/http-server-security.md`.

**Phase 5b:** SlackEventsAdapter built on top of WebhookRouter.

This unblocks Phases 7 (WhatsApp), 9 (Voice), 10 (Web) which all use
the same router. Risk register entry R11 added.

---

## Phases

Each phase is one /lope-execute round with all 3 lope validators
(pi + codex + opencode). Each phase completes with `cargo test`
green and lope ensemble PASS before the next begins.

### Phase 0 — Architecture lock via lope review

**Goal:** Get pi+codex+opencode consensus on Q1–Q15 above before
writing any code.

**Status:** round 1 complete (`phase0-negotiate.log`); round 2
(this revision) addresses all NEEDS_REVISION items; awaiting final
validator pass.

**Exit criteria:**
- `phase0-negotiate-r2.log` shows all 3 validators at PASS
- Final SPRINT.md committed with `[locked]` marker on each Q

### Phase 1 — Per-slot supervisor + lifecycle

**Goal:** `makakoo agent start/stop/restart/status <slot>` actually
spawns + holds + tears down the gateway, with status.json reflecting
live state.

**New code:**
- `makakoo-core/src/agents/supervisor.rs` — gateway spawn + status
  writer + restart budget + circuit break
- `makakoo-core/src/agents/lifecycle.rs` — start/stop/restart state
  machine
- `makakoo-core/src/agents/launchd.rs` (macOS) — plist generation +
  bootstrap with consent-error detection
- `makakoo-core/src/agents/systemd.rs` (Linux) — user unit generation
  + daemon-reload + start
- `makakoo/src/commands/agent_lifecycle.rs` — CLI wiring with
  consent remediation hint on macOS

**Tests:**
- 8 unit tests for supervisor restart budget + status snapshot +
  circuit break
- 4 integration tests with `MockChild` exercising start/stop/crash
- 2 macOS-gated tests for plist generation
- 2 Linux-gated tests for systemd unit generation

**Exit criteria:**
- `makakoo agent start secretary` returns within 2s; supervisor PID
  in status.json; gateway PID populated within 10s
- `makakoo agent status secretary` reads from status.json and
  matches Phase 4 v1 layout
- SIGTERM to supervisor cleanly stops the child within 5s
- macOS Files & Folders consent error caught and printed with
  remediation
- Foreground mode (`MAKAKOO_AGENT_SUPERVISOR=foreground`) works
  for headless deployments

### Phase 2 — `makakoo agent destroy`

**Goal:** Interactive teardown with archive + secret offer.

**New code:**
- `makakoo-core/src/agents/destroy.rs` — archive logic with `$MAKAKOO_HOME/archive/`
- `makakoo/src/commands/agent_destroy.rs` — interactive CLI

**Tests:**
- 8 unit tests covering archive layout, secret detection (with
  documented limitations), harveychat guard, --yes / --revoke-secrets /
  --keep-secrets flags, re-create-after-destroy semantics

**Exit criteria:**
- After destroy, `makakoo agent list` no longer shows the slot
- Archive at `$MAKAKOO_HOME/archive/agents/<slot>-<ts>/` contains
  `<slot>.toml` + `data/`
- Restore one-liner printed on success works
- `--yes` does NOT auto-revoke secrets
- harveychat refuses without `--really-destroy-harveychat`

### Phase 3 — Python gateway integration

**Goal:** A real Python LLM dispatcher attached to the Rust
supervisor handles inbound frames end-to-end with identity block,
tool whitelist preflight, path enforcement preflight, and brain
attribution. Authoritative scope still in Rust MCP layer.

**Pre-work:** Materialize `docs/specs/ipc-contract-v2.md`.

**New code:** `plugins-core/agent-harveychat/python/`
- `bridge.py` — newline-JSON IPC client (matches v1 Phase 1 contract)
- `gateway.py` — LLM dispatch loop, identity-block prefixing
- `tool_dispatcher.py` — MCP whitelist preflight
- `file_enforcement.py` — allowed_paths/forbidden_paths preflight
- `brain_sync.py` — `[agent:<slot>]` journal prefix

**Supervisor change:** Pre-issues a write grant for the slot's Brain
journal directory with `bound_to_agent = Some(slot_id)` before
spawning the gateway.

**Tests:**
- 14 pytest tests for bridge framing, identity rendering, tool
  whitelist denial, path violation denial, brain prefix, IPC
  reconnect
- 4 integration tests via `pexpect` exercising the full Rust
  supervisor + Python gateway pair against `MockTelegram`
- 1 contract test validating IPC schema across Rust + Python

**Exit criteria:**
- A mock Telegram inbound frame routed through the supervisor
  reaches `gateway.py`, prefixes the identity block, calls the
  LLM (mock), returns an outbound frame to the Rust router
- `tools = ["brain_search"]` rejects an attempted `run_command`
  in BOTH Python preflight AND Rust MCP enforcement
- `allowed_paths = ["~/Office/"]` rejects a write to `/etc/passwd`
  in BOTH Python preflight AND Rust MCP enforcement
- Brain journal line written with `[agent:secretary]` prefix
- IPC contract test confirms Rust + Python agree on framing

### Phase 4 — Per-agent LLM override

**Goal:** Each slot can run a different LLM with different
parameters, validated at create-time.

**New code:**
- `makakoo-core/src/agents/slot.rs` — add `LlmInherit` (no-op,
  documentation-only) + `LlmOverride` structs
- `makakoo-core/src/agents/llm_resolution.rs` — precedence resolver
- `makakoo/src/commands/agent_show.rs` — extend to display effective
  config with per-field source attribution
- `makakoo-core/src/agents/llm_validation.rs` — calls
  `SwitchAILocal::list_models()` for unknown-model rejection
- `plugins-core/agent-harveychat/python/llm_config.py` — read env
  vars set by supervisor, fall back to system defaults
- Supervisor populates `MAKAKOO_LLM_*` env on gateway spawn

**Tests:**
- 8 Rust tests for precedence + validation (online success, offline
  warning, unknown model rejection)
- 3 Python tests for env-var resolution

**Exit criteria:**
- Slot with `[llm.override] model = "claude-haiku-4-5"` spawns
  gateway with that model in env
- `makakoo agent show <slot>` displays effective LLM config with
  source attribution per field
- `agent create` with bogus model id rejects with available-models
  list
- Offline `agent validate` issues warning, not error

### Phase 5 — HTTP webhook router + Slack Events API

**Goal:** Production-HA Slack mode + reusable webhook scaffold for
Phases 7/9/10.

**Phase 5a — Webhook router primitive:**
- `makakoo-mcp/src/webhook_router.rs` — `WebhookHandler` trait,
  `WebhookRouter::register/mount` + per-handler audit middleware
- Mount on `/transport/<slot_uuid>/<transport_uuid>/<kind>/...` (UUIDs
  prevent path collision/guessing)
- `/health` endpoint with subsystem status
- `docs/specs/http-server-security.md` — route isolation contract

**Phase 5b — SlackEventsAdapter:**
- `makakoo-core/src/transport/slack_events.rs` — HTTP-webhook variant
- `makakoo-core/src/transport/slack.rs` — refactor: SlackAdapter
  becomes thin shell over `SlackTransportShared`
- HMAC verification BEFORE JSON parse (raw body)
- Replay window 5min (timestamp validation)
- URL verification challenge auto-response

**Tests:**
- 6 unit tests for WebhookRouter (registration, route isolation,
  health endpoint, missing-handler 404)
- 8 unit tests for SlackEventsAdapter: HMAC verification (good/bad/
  expired-timestamp/malformed-body), challenge response, audit log
  on bad signature
- 4 integration tests through axum test harness

**Exit criteria:**
- TOML `[transport.config] mode = "events_api"` validates with
  `signing_secret_ref` set; `signing_secret_ref` missing → reject
- `curl` against the webhook with valid HMAC routes to gateway
- Invalid HMAC returns 401 + audit log entry
  (`webhook.invalid_signature`)
- Health endpoint reports webhook router up

### Phase 6 — 4 OpenClaw-parity adapters

**Goal:** Power users can list channels, request explicit
approvals, route DM-vs-channel, and manage threads from the LLM.

**New code:**
- `makakoo-core/src/channel_ops/directory.rs` — list_channels,
  list_users, lookup_user
- `makakoo-core/src/channel_ops/approval.rs` — request_approval
  (sends inline buttons or text-fallback yes/no, awaits with
  timeout)
- `makakoo-core/src/channel_ops/messaging.rs` — send_dm,
  send_channel, broadcast
- `makakoo-core/src/channel_ops/threading.rs` — create_thread,
  list_threads, follow_thread

Per-transport impls for Telegram + Slack (Discord lands in Phase 7).

- `makakoo-mcp/src/handlers/tier_b/channel_ops.rs` — MCP tool surface

**Tests:**
- 16 unit tests (4 ops × 2 transports × 2 happy/fail)
- 4 integration tests using mock adapters
- 2 isolation tests confirming a slot can't query another slot's transports

**Exit criteria:**
- `mcp call channel_directory.list_channels {slot_id, transport_id}`
  returns the live channel list
- `channel_approval.request` blocks until user clicks/replies
  yes|no|timeout
- All 4 ops respect the slot's transport allowlist (no
  cross-slot data leakage)

### Phase 7 — Discord adapter (with channel ops)

**Goal:** Discord support via serenity. Includes the 4 OpenClaw
ops for Discord.

**New code:**
- `makakoo-core/src/transport/discord.rs` — DiscordAdapter
- `makakoo-core/src/channel_ops/discord_*.rs` — 4 op impls
- `makakoo-core/src/agents/slot.rs` — add `DiscordConfig` to
  TransportConfig enum (with `message_content: bool` and
  `guild_ids: Vec<u64>`)

**Tests:**
- 14 unit tests (frame mapping, intent validation, DM-vs-guild,
  thread parent resolution, guild_ids allowlist enforcement,
  MESSAGE_CONTENT degraded mode)
- 2 integration tests with mock serenity context

**Exit criteria:**
- Slot TOML with `[[transport]] kind = "discord"` validates and
  starts; bot appears online in mock guild
- DM and guild channel both route to the gateway with correct
  conversation_id distinction
- `guild_ids = [...]` rejects messages from non-allowlisted guilds
- `message_content = false` (default) gracefully handles
  empty-content messages

### Phase 8 — WhatsApp Cloud API adapter

**Goal:** Meta WhatsApp Cloud API webhook + send.

**New code:**
- `makakoo-core/src/transport/whatsapp.rs` — WhatsAppAdapter
- Webhook route via WebhookRouter
- `makakoo-core/src/agents/slot.rs` — `WhatsAppConfig` with
  `verify_token_ref`, `access_token_ref`, `phone_number_id`,
  `app_secret_ref` (for X-Hub-Signature-256)

**Tests:**
- 10 unit tests (webhook signature, hub.challenge handshake, message-vs-status
  filtering, media-drop reply, malformed body)
- 2 integration tests

**Exit criteria:**
- Mock webhook POST routes to gateway with correct frame
  population
- Outbound Cloud API `messages` POST returns 200 in mock
- Validate command checks `phone_number_id` exists via
  Cloud API GET `/<phone_number_id>` (mocked)
- Inbound media triggers documented drop-reply

### Phase 9 — Email adapter (IMAP IDLE + SMTP)

**Goal:** Email-as-transport with proper threading.

**New code:**
- `makakoo-core/src/transport/email.rs` — EmailAdapter
- `imap` + `lettre` + `mailparse` crates added to workspace
- Custom quote-line stripper (no Python deps)
- `makakoo-core/src/agents/slot.rs` — `EmailConfig` with OAuth2 +
  app-password variants

**Tests:**
- 14 unit tests (Message-ID threading, IDLE reconnect, STARTTLS
  handshake mock, OAuth2 refresh mock, reply parser, plain-IMAP
  rejection by validate)
- 2 integration tests against `mock-smtp-server` + `mock-imap`

**Exit criteria:**
- IMAP IDLE inbound delivers a Message-ID-rooted thread to the
  gateway with conversation_id = root Message-ID
- Outbound SMTP send sets In-Reply-To + References correctly
- OAuth2 path validates a Gmail token (mock-tested with
  `oauth2-mock-server`)
- `account_id = full mailbox address`
- `raw_metadata.body_raw` preserves full unparsed body
- Plain (non-TLS) IMAP/SMTP rejected by validate

### Phase 10 — Voice adapter (Twilio + TwiML, push-to-talk)

**Goal:** Inbound calls become text frames; replies become
spoken TwiML.

**New code:**
- `makakoo-core/src/transport/voice_twilio.rs` — VoiceAdapter
  with locked state machine (Q9)
- TwiML XML generation
- STT integration via SwitchAILocal (`whisper-1`)
- TTS via SwitchAILocal `openai/tts-1` (with elevenlabs fallback)
- Webhook routes via WebhookRouter (with CallSid in path for
  recording correlation)
- Twilio basic-auth for recording fetches

**Tests:**
- 10 unit tests (TwiML XML generation, signature verification,
  STT/TTS mock flow, fallback chain, recording-URL auth, hangup
  cleanup)
- 1 integration test exercising mock Twilio webhook → STT mock →
  gateway mock → TTS mock → TwiML response

**Exit criteria:**
- Mock Twilio inbound webhook returns valid TwiML with dynamic
  CallSid-embedded recording URL
- Recording-completed webhook fetches audio with basic-auth (mocked),
  STT returns text, gateway responds, TTS returns audio URL,
  TwiML `<Play>` references it
- Fallback chain triggers when primary TTS fails
- Realtime streaming documented as deferred to v2.1

### Phase 11 — Web chat adapter

**Goal:** Embeddable web chat client; stable WS endpoint.

**New code:**
- `makakoo-core/src/transport/web.rs` — WebChatAdapter
- WS route via WebhookRouter (with WS upgrade special-case)
- HMAC-SHA256 signed visitor cookie (key persisted to
  `$MAKAKOO_HOME/keys/web-chat-hmac` mode 0600)
- `docs/walkthroughs/web-chat-demo.html` — minimal static client

**Tests:**
- 12 unit tests (cookie sign/verify roundtrip, expired cookie
  rejection, message envelope schema, typing indicator,
  reconnect, key persistence on restart, localhost dev exception)
- 2 integration tests through axum test harness

**Exit criteria:**
- Connect to ws://localhost:.../transport/.../ws with a fresh
  cookie; send `{type:"msg", text:"hi"}`; receive
  `{type:"msg", text:"...", ts:...}` reply
- Cookie key persists across makakoo-mcp restart
- `Secure` flag dropped only on localhost requests
- Demo HTML works in a browser against a live makakoo-mcp

### Phase 12 — Fault injection + grandma-secure hardening

**Goal:** Production-grade resilience and audit trail.

**New code:**
- `makakoo-core/src/agents/fault_inject.rs` — `MAKAKOO_DEV_FAULTS=1`
  gated test commands (Q11 scenarios, both gateway-oom subcases)
- `makakoo-core/src/agents/rlimits.rs` — setrlimit wrapper with
  warn-mode default
- `makakoo-core/src/agents/rate_limit.rs` — token bucket per
  `(slot, transport, sender_key)` + global per-slot tier
- `makakoo-core/src/agents/audit.rs` — JSONL writer with 100MB
  rotation + 1GB total cap + 0600 perms + secret redaction
- `makakoo/src/commands/agent_audit.rs` — `agent audit` CLI with
  `--last`, `--kind`, `--since`, `--full` flags
- `makakoo/src/commands/agent_test_faults.rs` — fault test runner
- Supervisor RSS monitor writing to status.json

**Tests:**
- 20 unit tests covering each Q11 scenario (incl. both gateway-oom
  subcases) + rlimit (warn vs enforce) + rate bucket (per-sender +
  global tier) + audit log rotation + 1GB cap behavior +
  secret redaction
- 4 integration tests for end-to-end fault recovery
- 2 tests for webhook-verification rate-limit bypass

**Exit criteria:**
- All 8 Q11 scenarios produce the locked observable outcome
- Per-slot RSS surfaces in status.json + `agent status`
- 200-frame burst triggers `queue.overflow` + `drop_newest` +
  `rate.limit` audit entry
- 1GB audit cap triggers stderr warn + alerts.log entry
- `makakoo agent audit secretary --last 10` returns last 10
  audit lines with redacted detail; `--full` shows raw

### Phase 13 — HTE wizard + UX polish

**Goal:** `makakoo agent wizard` walks any user through slot
creation without TOML editing. All docs match v2 surface.

**New code:**
- `plugins-core/skill-agent-wizard/SKILL.md` — interactive wizard
  (lives in plugins-core alongside other shipped skills; legacy
  harvey-os path retired)
- `makakoo/src/commands/agent_wizard.rs` — wizard CLI entry with
  TTY detection + non-TTY fallback (prints TOML template)
- Updated `docs/walkthroughs/multi-transport-subagents.md`
  covering all 7 transport adapters (Telegram, Slack, Discord, WhatsApp, Email,
Voice, Web — Slack ships in two modes Socket+Events but is one
adapter family)
- Updated `docs/troubleshooting/agents.md` with new transport
  failure modes (Discord intents/MESSAGE_CONTENT, WhatsApp
  signature/verify_token, Email IDLE/OAuth2, Voice TwiML/CallSid,
  Web cookie/key-rotation, macOS launchd consent)
- Updated `docs/user-manual/agent.md` with all new subcommands
  (`destroy`, `wizard`, `audit`, `test-faults`)
- New `docs/walkthroughs/discord-bot.md` (with guild_ids notes)
- New `docs/walkthroughs/whatsapp-business.md` (with verify_token + media)
- New `docs/walkthroughs/email-secretary.md` (with OAuth2 vs app-password)
- New `docs/walkthroughs/voice-quickstart.md` (with TwiML state machine)
- New `docs/walkthroughs/web-chat-demo.html`
- New `docs/specs/ipc-contract-v2.md` (materialized in Phase 3, refreshed here)
- New `docs/specs/http-server-security.md` (route isolation)

**Tests:**
- 8 unit tests for wizard prompt/response flow (incl. TTY-missing
  fallback)
- Doc-link CI gate: every command name in walkthroughs must
  resolve in the CLI manual

**Exit criteria:**
- Running `makakoo agent wizard` end-to-end creates a working
  slot from prompts alone (no manual TOML)
- TTY-missing detected and TOML template printed instead
- `cargo test` green across workspace
- `lope review` PASS from all 3 validators on doc accuracy

---

## Acceptance criteria (sprint-level)

1. `makakoo agent {start|stop|restart|status|destroy|wizard|audit|test-faults}`
   all live and documented.
2. 7 transport adapter families operational in code: Telegram, Slack
   (Socket Mode + Events API webhook), Discord, WhatsApp, Email,
   Voice, Web.
3. Per-slot LLM override works end-to-end with create-time validation.
4. All 4 OpenClaw-parity adapters live for Telegram + Slack + Discord.
5. Grandma-secure: rlimits (warn-default), rate limits (per-sender +
   global), audit log (100MB rotation + 1GB cap + redaction),
   fault injection (8 scenarios) all live.
6. Python reference gateway live for harveychat with documented IPC
   contract spec.
7. HTE wizard ships and onboards a brand-new user successfully.
8. All 8 fault-injection scenarios pass.
9. `cargo test` green across the workspace.
10. All 3 lope validators PASS the final round.

---

## Non-goals

- **Live dogfood with real tokens.** Sebastian must run that himself
  with real Telegram/Slack/Discord/WhatsApp/Email/Twilio/web setup.
  Code ships fully tested against mocks; v2.1 will absorb whatever
  dogfood surfaces.
- **Realtime streaming voice.** Push-to-talk only in v2.0. v2.1
  for OpenAI Realtime / Twilio Media Streams.
- **Cross-machine slot sync.** Slots stay per-machine. v3 if
  anyone asks.
- **whatsapp-web.js / Twilio WhatsApp.** Cloud API only.
- **POP3 or non-TLS email.** Modern email only.
- **Multi-tenant SaaS hosting.** Makakoo is local-first. Each user
  runs their own.
- **Full inbound media on WhatsApp.** v2.0 ships explicit-drop;
  full media in v2.1.
- **macOS without Files & Folders consent.** No silent fallback —
  user must grant or use foreground supervisor mode.

---

## Risk register

| # | Risk | Mitigation |
|---|------|------------|
| R1 | Python gateway IPC contract drift between Rust + Python | Contract test in `tests/ipc_contract.rs` running both sides on each `cargo test` + materialized spec at `docs/specs/ipc-contract-v2.md` |
| R2 | Voice adapter requires audio infrastructure tests can't fully exercise | Mock STT/TTS; flag dogfood gap explicitly in voice-quickstart.md |
| R3 | Email IDLE reconnect on long-lived idle TCP races with server-side timeout | Idle window cap at 25min + heartbeat NOOP every 5min |
| R4 | Discord bot intent verification requires bot to be in a real guild | Mock serenity Context for tests; degraded MESSAGE_CONTENT mode default; explicit dogfood note |
| R5 | WhatsApp Cloud API rate limits per-phone | Documented; rate limiter respects them |
| R6 | Twilio webhook signature scheme rotates / changes | Use official twilio-rust crate signature verifier |
| R7 | Web chat HMAC key rotation breaks active sessions | Document key rotation procedure; cookies expire naturally in 24h |
| R8 | rlimits on macOS differ from Linux (no NPROC enforcement) | Document; rely on slot-count cap as backstop; warn-mode default |
| R9 | Per-slot audit log fills disk if a misbehaving agent loops | Rotate at 100MB; **1GB total cap** with stderr + alerts.log + block-then-evict-oldest |
| R10 | HTE wizard breaks if user's terminal lacks TTY | Detect; fall back to TOML template print mode |
| R11 | Shared makakoo-mcp HTTP crash takes all webhook transports offline | Document blast radius; require launchd/systemd auto-restart for makakoo-mcp; expose `/health`; offer Socket Mode fallback for users who need transport isolation |
| R12 | macOS launchd Files & Folders consent requirement may surprise users | Detect error code 5 and print copy-paste remediation; documented in troubleshooting |
| R13 | Email app-password fallback weaker than OAuth2 | Document prominently; require TLS; consider deferring non-Gmail to v2.1 if dogfood hits friction |
| R14 | Audit log secret-name leakage if dev forgets schema | Typed `detail` field with explicit redaction in writer; lint check in CI for `format!` calls inside audit.rs |
| R15 | Discord MESSAGE_CONTENT intent rejection by Discord verification | Default to mention-only mode; explicit opt-in flag with degraded fallback |

---

## Estimated cost

- **Validator review rounds:** ~14 phases × ~1.5 rounds avg = ~21 lope ensemble runs
- **New Rust LOC:** ~7,000 (5 new transports × ~700 + supervisor + audit + rlimits + 4 OpenClaw seams + webhook router)
- **New Python LOC:** ~1,400 (gateway + 4 enforcement modules + LLM config)
- **New tests:** ~180 (Rust) + ~30 (Python)
- **New docs:** ~12 walkthroughs/troubleshooting/manual/spec pages
- **Cross-repo touches:** none planned (garagetytus already shipped Phase 3 grant binding; plugins-core is in-repo)

---

## How to resume after this sprint

After v2.0 ships, resume with `sprint-multi-bot-subagents-v2.1`:
1. **Live dogfood** — Sebastian provides real tokens, runs all 7
   transports, files what breaks
2. **Realtime streaming voice** — OpenAI Realtime API or Twilio
   Media Streams
3. **Full WhatsApp media** — inbound image/audio/video handling
4. **Cross-machine slot sync** — if anyone asks
5. **Marketplace transport plugins** — third-party transports via
   `makakoo plugin install git+...`
