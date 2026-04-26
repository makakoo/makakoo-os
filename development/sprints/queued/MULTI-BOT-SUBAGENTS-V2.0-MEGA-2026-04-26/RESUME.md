# RESUME — v2-MEGA continuation

**For a fresh Claude context picking this up.** Read this top-to-bottom
before touching code.

---

## TL;DR

- **Sprint dir:** `development/sprints/queued/MULTI-BOT-SUBAGENTS-V2.0-MEGA-2026-04-26/`
- **HEAD:** `f3c4ee7` (re-tag pending)
- **Sprint doc:** `SPRINT.md` (in this dir) — 47KB, locked Q1–Q15
- **Phases done:** 0, 1, 2, 3, 4, 5, 6, 12 (partial)
- **Phases pending:** 7, 8, 9, 10, 11, 12 (rest), 13
- **Test count:** ~245 new tests, all green
- **Workspace:** `/Users/sebastian/makakoo-os/` (Rust + Python plugin)
- **Memory:** `~/.claude/projects/-Users-sebastian-MAKAKOO/memory/project_multi_bot_subagents_v2_mega.md`

---

## What you must read FIRST

In this exact order:

1. `SPRINT.md` (this dir) — the locked architecture. Q0–Q15.
2. `phase0-negotiate-r4.log` — final round-4 lope ensemble PASS
3. The "Per-phase exit criteria" section in SPRINT.md
4. `RESUME.md` (this file) — what you're reading now
5. `~/.claude/projects/-Users-sebastian-MAKAKOO/memory/project_multi_bot_subagents_v2_mega.md`
   — which subsystems exist + their commit hashes

Don't read random source files until you've digested those four. The
spec drives the code, not the other way around.

---

## Workspace state

```
makakoo-os/
├── makakoo-core/src/agents/
│   ├── audit.rs                  ✅ Phase 12 (10 tests)
│   ├── destroy.rs                ✅ Phase 2  (15 tests)
│   ├── identity.rs               (Phase 3 v1 — pre-existing)
│   ├── launchd.rs                ✅ Phase 1  (13 tests)
│   ├── lifecycle.rs              (legacy plugin lifecycle — pre-existing)
│   ├── llm_override.rs           ✅ Phase 4  (13 tests)
│   ├── migrate/                  (v1 harveychat migration)
│   ├── mod.rs                    ✅ wires all the above
│   ├── rate_limit.rs             ✅ Phase 12 (8 tests)
│   ├── registry.rs               (v1)
│   ├── scaffold.rs               (legacy)
│   ├── scope.rs                  (v1)
│   ├── slot.rs                   ✅ extended w/ llm field
│   ├── status.rs                 (v1)
│   ├── supervisor.rs             ✅ Phase 1  (11 tests)
│   ├── supervisor_runtime.rs     ✅ Phase 1  (11 tests)
│   └── systemd.rs                ✅ Phase 1  (5 tests, Linux-gated)
├── makakoo-mcp/src/
│   ├── slack_events.rs           ✅ Phase 5b (13 tests)
│   ├── webhook_router.rs         ✅ Phase 5a (11 tests)
│   └── ... (existing /rpc Ed25519 path untouched)
├── makakoo/src/
│   ├── cli.rs                    ✅ AgentCmd grew Restart, Supervisor, Destroy
│   ├── commands/
│   │   ├── agent.rs              ✅ slot-aware routing
│   │   ├── agent_destroy.rs      ✅ Phase 2 (12 tests)
│   │   ├── agent_lifecycle.rs    ✅ Phase 1 (6 tests)
│   │   └── agent_slot.rs         ✅ extended `show` for LLM attribution
│   ├── context.rs                ✅ added for_home test ctor
│   └── main.rs                   (default-banner pattern intact)
├── plugins-core/agent-harveychat/python/
│   ├── __init__.py               (__version__ = "2.0.0")
│   ├── bridge.py                 ✅ Phase 3 (8 tests)
│   ├── brain_sync.py             ✅ Phase 3 (6 tests)
│   ├── conftest.py               (sys.path + module aliasing for pytest)
│   ├── file_enforcement.py       ✅ Phase 3 (9 tests)
│   ├── gateway.py                ✅ Phase 3 (11 tests)
│   ├── llm_config.py             ✅ Phase 4 (3 tests)
│   ├── tool_dispatcher.py        ✅ Phase 3 (6 tests)
│   ├── pytest.ini                (asyncio_mode = auto)
│   └── tests/
│       ├── fixtures/sample_inbound.json   ⚓ Rust↔Python contract anchor
│       └── test_*.py
└── docs/specs/
    └── ipc-contract-v2.md        ✅ Phase 3 — locked wire shape
```

---

## What to build next (Phase 7 onward)

### Phase 6 — DONE 2026-04-26 (commit `f3c4ee7`)

Shipped:
- `makakoo-core/src/channel_ops/{mod,directory,approval,messaging,threading,telegram,slack}.rs`
- `makakoo-mcp/src/handlers/tier_b/channel_ops.rs`
- `ChannelOpsRegistry` with per-(slot,transport_id) maps for all 4
  trait families and slot-isolated lookup.
- 10 MCP tools: `channel_directory_*`, `channel_messaging_*`,
  `channel_threading_*`, `channel_approval_request`.
- 49 new tests (41 channel_ops + 8 MCP). All green.

Note for callers: tool names use underscores not dots — the MCP
naming convention enforced by
`handler_contract_tests::tool_names_follow_naming_convention`
rejects dots.

### Phase 7 — Discord (serenity)

Add `serenity = "0.12"` to workspace Cargo.toml. Build
`makakoo-core/src/transport/discord.rs` mirroring the shape of the
existing `transport/telegram.rs` and `transport/slack.rs`. Implement
the 4 channel-ops traits from Phase 6 for Discord as well.

### Phase 8 — WhatsApp Cloud API

Use the WebhookRouter from Phase 5a. Reuse the
SlackEventsHandler pattern: HMAC verify before parse + url
verification challenge handshake.

### Phase 9 — Email IMAP IDLE + SMTP

Add `imap`, `lettre`, `mailparse` to workspace deps. IMAP IDLE
reconnect cap at 25 min + heartbeat NOOP every 5 min (locked Q8).

### Phase 10 — Voice Twilio

TwiML state machine locked in SPRINT.md Q9. Recording-callback URL
must embed CallSid for correlation. STT via SwitchAILocal whisper-1.

### Phase 11 — Web chat WS

HMAC-SHA256 cookies with key persisted to
`$MAKAKOO_HOME/keys/web-chat-hmac` (mode 0600). Origin allowlist
required in production (locked Q10 round-2 fix).

### Phase 12 (rest) — fault injection + rlimits + audit CLI

- `makakoo-core/src/agents/fault_inject.rs` — 8 scenarios from
  SPRINT.md Q11
- `makakoo-core/src/agents/rlimits.rs` — opt-in setrlimit wrapper
- `makakoo/src/commands/agent_audit.rs` — CLI binding for the
  already-shipped `agents::audit::tail_events`
- `makakoo/src/commands/agent_test_faults.rs` — gated behind
  `MAKAKOO_DEV_FAULTS=1`

### Phase 13 — HTE wizard + docs

- `plugins-core/skill-agent-wizard/SKILL.md` — interactive
  prompt → slot.toml flow (TTY detect + non-TTY fallback)
- `makakoo/src/commands/agent_wizard.rs`
- Per-transport walkthroughs:
  - `docs/walkthroughs/discord-bot.md`
  - `docs/walkthroughs/whatsapp-business.md`
  - `docs/walkthroughs/email-secretary.md`
  - `docs/walkthroughs/voice-quickstart.md`
  - `docs/walkthroughs/web-chat-demo.html`
- Update existing `docs/walkthroughs/multi-transport-subagents.md`
  to cover all 7 transport adapters
- Update `docs/troubleshooting/agents.md` with new failure modes
- Refresh `docs/user-manual/agent.md` with `destroy`, `wizard`,
  `audit`, `test-faults` subcommands
- New `docs/specs/http-server-security.md` — route-isolation contract

---

## Critical context that's NOT in source files

Read these so you don't re-derive them:

### Locked schema decisions (Phase 0 round-4)

The following are **non-negotiable** without re-running Phase 0:

| Q | Decision |
|---|---|
| Q1 | One Rust supervisor per slot, spawns ONE Python gateway child via tokio::process. macOS launchd / Linux systemd-user; foreground via `MAKAKOO_AGENT_SUPERVISOR=foreground`. |
| Q2 | Rust MCP/grant layer is authoritative scope enforcer. Python is preflight + UX layer only. |
| Q3 | Destroy archives to `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/<slot>.toml + data/`. `--yes` does NOT auto-revoke secrets — `--revoke-secrets` is explicit. |
| Q4 | LLM precedence: per-call > slot.toml `[llm.override]` > makakoo system defaults. |
| Q5 | Slack Events API at `/transport/<slot_uuid>/<transport_uuid>/events` (UUIDs as opaque 36-char hex strings). HMAC verify BEFORE JSON parse. 5-min replay window. |
| Q6 | Discord uses serenity. MESSAGE_CONTENT default OFF (privileged). `guild_ids` allowlist optional. |
| Q7 | WhatsApp Cloud API only. `verify_token_ref` for handshake. Inbound media → polite drop reply. |
| Q8 | Email account_id = full mailbox address. mailparse for parsing. OAuth2 for Gmail (mandatory), app-passwords for others (documented as weaker). Plain IMAP/SMTP rejected by validate. |
| Q9 | Twilio Voice + TwiML push-to-talk. Recording-callback URL embeds CallSid. STT/TTS via SwitchAILocal. Realtime streaming → v2.1. |
| Q10 | Web cookies use HMAC-SHA256 (not Ed25519). Key persists to `$MAKAKOO_HOME/keys/web-chat-hmac` mode 0600. Origin allowlist REQUIRED in production. |
| Q11 | 8 fault-injection scenarios, all mock-only behind `MAKAKOO_DEV_FAULTS=1`. |
| Q12 | rlimits OPT-IN via `[agents] enforce_rlimits = true`. RSS monitoring always on (warn-only). Slot-count cap 32 (always on). |
| Q13 | Per `(slot, transport, sender)` token bucket 60/5min + per-slot global 600/5min. Webhook verification probes bypass. |
| Q14 | Audit log JSONL at `$MAKAKOO_HOME/data/audit/agents.jsonl`, 100MB rotation, 1GB total cap, mode 0600. Secret/token/body redacted; actor/target identifiers logged. |
| Q15 | `WebhookHandler` uses `#[async_trait]` (not bare async fn — object-safety). Body pre-buffered as Bytes so verify-before-parse works. WS upgrade via separate `WsUpgradeHandler` trait. |

### Known compromises taken

- **No `uuid` crate dependency** — webhook_router validates 36-char
  hex-with-dashes shape directly. If a future subsystem needs proper
  UUID parsing, add `uuid = "1"` then refactor.
- **gateway.py uses flat sibling imports** (`from bridge import ...`)
  because hyphenated dirs can't be Python module names. Supervisor
  cd's into `plugins-core/agent-harveychat/python/` before launch.
- **conftest.py** maintains a separate package alias chain so pytest
  can use `from plugins_core.agent_harveychat.python.X import Y` in
  tests while production uses flat imports. Module identity is
  preserved via `sys.modules` aliasing.
- **Audit log NOT yet wired** into the supervisor / webhook handler
  / scope checker call sites. The module ships + tests, but call
  sites still need `audit::append_event(...)` calls. Phase 12-rest.
- **Rate limiter NOT yet wired** into the inbound frame flow. Same
  deal as audit. Phase 12-rest.
- **Pre-existing flakes** in `capability::grants::tests::shipped_core_plugins_resolve_cleanly`
  + `plugin::registry::tests::shipped_core_plugins_all_parse` —
  unrelated to v2-mega; v1 sprint memory documents them.

### Linter caveat

A linter on this branch sometimes touches files mid-edit. Watch for
`<system-reminder>` blocks announcing modifications and re-read
files before editing if a reminder fires.

### MAKAKOO_HOME caveat

Recall: `~/MAKAKOO` symlinks to `~/HARVEY` (legacy). The platform is
**Makakoo OS**; the persona is **Harvey**. Don't confuse them.

---

## How to verify state from a fresh context

```bash
cd /Users/sebastian/makakoo-os

# 1. Confirm tag + commit
git log --oneline sprint-multi-bot-subagents-v2.0-partial -1

# 2. Confirm tests pass
cargo test -p makakoo-core --lib agents::supervisor agents::supervisor_runtime
cargo test -p makakoo-core --lib agents::launchd agents::destroy
cargo test -p makakoo-core --lib agents::llm_override
cargo test -p makakoo-core --lib agents::audit agents::rate_limit
cargo test -p makakoo-core --lib transport::frame::
cargo test -p makakoo-mcp --bin makakoo-mcp webhook_router
cargo test -p makakoo-mcp --bin makakoo-mcp slack_events
cargo test -p makakoo --bin makakoo agent_lifecycle agent_destroy

# 3. Python tests
cd plugins-core/agent-harveychat/python && python3.11 -m pytest tests/

# 4. Cross-language contract anchor
cargo test -p makakoo-core --lib transport::frame::tests::rust_emits_exact_bytes_of_python_contract_fixture
```

All should pass.

---

## How to start Phase 6

```bash
# 1. Read the current state
cat /Users/sebastian/makakoo-os/development/sprints/queued/MULTI-BOT-SUBAGENTS-V2.0-MEGA-2026-04-26/SPRINT.md | grep -A 30 "Phase 6 —"

# 2. Read the existing transport contract for reference
cat /Users/sebastian/makakoo-os/makakoo-core/src/transport/router.rs

# 3. Build channel_ops/ skeleton
mkdir -p /Users/sebastian/makakoo-os/makakoo-core/src/channel_ops

# 4. Write the 4 traits (directory/approval/messaging/threading)
# 5. Per-transport impls for Telegram (transport/telegram.rs grows
#    a `impl ChannelDirectoryAdapter for TelegramAdapter` block etc.)
# 6. Per-transport impls for Slack (same pattern)
# 7. MCP tool surface in handlers/tier_b/channel_ops.rs
# 8. Tests
# 9. lope review
# 10. Commit
```

---

## What Sebastian must do (live-dogfood gate)

Before v2.0 ships fully, Sebastian needs to:

1. Provide real Telegram + Slack tokens and run
   `makakoo agent migrate-harveychat` then
   `makakoo agent start harveychat` to flush the Phase 1+3 supervisor
   + Python gateway path end-to-end.
2. File any breakages — the supervisor + Python gateway have unit
   tests + integration tests with mocks, but no real-world dogfood
   yet. There WILL be small bugs.
3. After dogfood passes, run `cargo install makakoo` to land the v2
   tag publicly.

This dogfood pass is explicitly listed as a non-goal of the
autonomous sprint (live tokens require Sebastian at the keyboard).

---

## Memory file

The memory at
`~/.claude/projects/-Users-sebastian-MAKAKOO/memory/project_multi_bot_subagents_v2_mega.md`
is the canonical state record. Update it when you ship Phase 6.
