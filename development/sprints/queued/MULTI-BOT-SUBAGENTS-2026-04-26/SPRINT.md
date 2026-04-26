# SPRINT — Multi-bot subagents over Telegram

**Status:** queued for negotiation. Ready to feed into `lope negotiate`.
**Owner:** Sebastian.
**Date:** 2026-04-26.
**Related sprints:** v0.6 agentic-plug (shipped 2026-04-21), telegram_group_setup (memory).

## One-line vision

Make every Telegram bot Sebastian creates an independently-scoped
Makakoo subagent. `makakoo agent create <name>` deploys a new bot with
its own persona, tools, and filesystem scope in <30 seconds.

## Scope flag — this is core, not a plugin

Sebastian explicitly framed this (2026-04-26) as **core OS
functionality**, not a peripheral plugin feature. Bar is raised:

- `makakoo agent` becomes a first-class verb at the same depth as
  `plugin`, `perms`, `daemon`. Same JSON envelopes, same audit
  hooks, same getting-started prominence.
- The agent-id propagates through every subsystem: grants gain
  `bound_to_agent`, Brain entries gain `agent_id` prefix, MCP calls
  carry an originating-agent header.
- The schema is transport-agnostic. Telegram is `transport.kind="telegram"`,
  but the model must accommodate WhatsApp / Slack / email / voice
  without rework. Phase 0 negotiation locks the schema general enough
  to grow.
- The redesign SUBSUMES the existing `agent-*` plugin pattern. After
  this sprint, today's `agent-arbitrage`, `agent-career-manager`,
  `agent-browser-harness`, `agent-harveychat`, etc. are all the same
  flavor of thing — they only differ in which transport(s) they
  attach to (Telegram messenger, SANCHO schedule, tool-only, etc.).

(Read [VISION.md](VISION.md) for the full picture and
[AUDIT.md](AUDIT.md) for what exists today and what's blocking this.)

## Phases

### Phase 0 — discovery (before writing code)

- [ ] Run `lope negotiate` on this sprint with `pi + gemini + codex`
      ensemble. Hard-fail if validators don't reach consensus on the
      open questions in AUDIT.md.
- [ ] Lock the agent-config schema (TOML) — every later phase depends on
      it. Sample sketch in VISION.md; needs validator review.
- [ ] Decide: per-agent process vs multiplexed gateway. Both are
      defensible; pick one + document tradeoff.
- [ ] Decide: where bot tokens live (per-agent `config.json`, OS
      keyring, env vars). Cross-reference with the existing
      `makakoo secret` keyring tooling.

### Phase A — agent registry + config schema

- [ ] Define `~/MAKAKOO/config/agents/<slot>.toml` with fields:
      `name`, `slot_id`, `persona`, `paths`, `tools`, `bot_token_ref`,
      `allowed_users`, `process_mode` (own/multiplexed).
- [ ] Add `makakoo agent list` to enumerate configured slots (running
      + stopped + crashed). Backed by the registry, not by `pgrep`.
- [ ] Migrate the existing `agent-harveychat` to the new schema as
      the first slot (slot_id = `harveychat`, name = "Olibia").
      Backward-compat: existing `data/chat/config.json` continues to
      work; new schema reads from it on first load.
- [ ] CLI: `makakoo agent show <slot>` prints the resolved schema
      (with token redacted).
- [ ] Tests: registry parser refuses invalid TOML, refuses duplicate
      slot ids, refuses tokens that fail `getMe` validation.

### Phase B — `makakoo agent create` wizard

- [ ] Interactive: prompts for slot name, bot token, persona snippet,
      paths, tools, allowed users.
- [ ] Non-interactive: every prompt is also a flag for scripting.
- [ ] Validates the bot token via `getMe` BEFORE writing anything
      to disk.
- [ ] Generates `plugins-core/agent-<slot>/plugin.toml` from a template.
- [ ] Generates `~/MAKAKOO/config/agents/<slot>.toml`.
- [ ] Generates the LaunchAgent plist / systemd unit.
- [ ] Calls `makakoo agent start <slot>` at the end.
- [ ] Verifies polling is live by sending a `getUpdates` test.

### Phase C — per-agent persona + tool scoping

- [ ] Replace the singular `HARVEY_SYSTEM_PROMPT` with a renderer
      that:
      1. Loads the canonical `~/MAKAKOO/bootstrap/global.md`.
      2. Loads the per-agent `persona` snippet from the agent's config.
      3. Injects an "identity block": *"You are `<slot_id>`. Your
         scope is `<paths>`. Your tools are `<tools>`."*
      4. Returns a single concatenated prompt.
- [ ] Tool dispatcher honors the per-agent allowed-tools whitelist.
      Calls to a non-allowed tool return `tool not in scope` to the
      LLM (which lets the LLM tell the user, not crash).
- [ ] `write_file` and `markdown_to_pdf` honor the per-agent
      allowed-paths in addition to the existing baseline + grants.
- [ ] `grant_write_access` gains a `bound_to_agent` field. A grant
      issued by `agent-olibia` is invisible to `agent-career`.

### Phase D — multi-process / shared infra

- [ ] One process per agent (Phase 0 verdict TBD). Each process
      runs `core.chat start` with `--slot <slot_id>` so the gateway
      reads only the matching `config/agents/<slot>.toml`.
- [ ] LaunchAgent plists generated per slot:
      `com.makakoo.agent.<slot>.plist`. Same shape, slot-id-suffixed.
- [ ] Shared writes to `~/MAKAKOO/data/Brain/journals/<today>.md`
      use a file lock so two agents writing simultaneously don't
      collide.
- [ ] Shared `conversations.db` becomes per-agent: each slot keeps
      its own SQLite at `~/MAKAKOO/data/agents/<slot>/conversations.db`.

### Phase E — observability + teardown

- [ ] `makakoo agent status <slot>` reports running/stopped/crashed,
      last-message timestamp, polling latency, allowed-users count,
      error rate (last hour).
- [ ] `makakoo agent logs <slot> [--tail N]` tails the per-agent log.
- [ ] `makakoo agent restart <slot>` for in-place reload after a
      config edit.
- [ ] `makakoo agent destroy <slot>` interactive teardown:
      stop process, remove plist, archive config + db to
      `~/.makakoo/archive/agents/<slot>-<timestamp>/`, optionally
      revoke the bot token via Telegram API.

### Phase F — docs + test sweep + core promotion

- [ ] `docs/agents/multi-bot-subagents.md` covers the full end-to-end
      flow with concrete examples (secretary, career-manager, custom).
- [ ] Update `docs/install-wizard-flows.md` § Per-CLI infect with a
      cross-link to multi-bot agent creation.
- [ ] Smoke test: clean machine + 3 bot tokens → 3 subagents live in
      <90 seconds.
- [ ] Update bootstrap-base.md to document `<slot_id>` injection so
      the LLM knows it's a specific subagent (the audit-discovered
      "Olibia thinks Olibia is third party" bug is fixed).
- [ ] **Core-promotion items** (because subagents are now first-class):
  - [ ] README — promote `makakoo agent create` to the Quickstart
        block alongside `makakoo install`, `makakoo setup`, and
        `makakoo infect`.
  - [ ] `docs/getting-started.md` — add "Step 5: Create your first
        subagent bot" after the existing infect step.
  - [ ] `docs/user-manual/index.md` — add `makakoo-agent.md` page
        as a peer to `makakoo-plugin.md`, `makakoo-perms.md`, etc.
  - [ ] `makakoo setup` wizard — new `agent` section between
        `model-provider` and `infect`. Optional (skippable for
        users who don't want any bots), but documented.
  - [ ] `docs/concepts/architecture.md` (or equivalent) — the
        agent abstraction is documented as one of the four core
        primitives (plugin, agent, daemon, perms).

## Acceptance criteria

See [VISION.md § Acceptance criteria](VISION.md#acceptance-criteria--sprint-is-done-when).

## Open questions to lock in Phase 0

(Listed in AUDIT.md § Open questions. Reproduced here for the
negotiation prompt.)

1. One process per agent vs multiplexed gateway?
2. Per-bot vs per-conversation scoping?
3. How does an agent know its own slot id at runtime?
4. Where do agent definitions live (config/agents/ vs plugins-core/)?
5. Does `makakoo agent list` enumerate unprovisioned slots?
6. Per-agent allowed-tools whitelist + forbidden-paths blacklist?
7. Telegram username vs slot id — must they match?
8. **NEW (core-elevation question)**: subsume the existing 13
   `agent-*` plugins under the unified subagent model, or keep two
   parallel concepts (legacy `agent-*` plugins vs new transport-
   attached subagents)? Lope ensemble must converge on one.
9. **NEW (transport-agnostic question)**: schema field for messenger
   attachment — `[transport]` table with `kind = "telegram" | "whatsapp"
   | "slack" | "email" | "voice"` + transport-specific subfields, vs
   per-transport top-level keys (`[telegram]`, `[whatsapp]`, ...).
   Whichever is locked must keep the schema forward-compatible for
   transports we haven't added yet.
10. **NEW (cross-subsystem propagation)**: how agent-id rides through
    the rest of Makakoo. `bound_to_agent` on grants is clear; less
    clear: does every Brain journal line get `[agent:<id>]` prefix,
    or only when written by an agent? Does the MCP server expose an
    originating-agent header for tool invocations? Lope must lock
    the propagation contract.

## Estimated cost

- Phase 0 (negotiate): 1-2 hours of validator round-tripping.
- Phase A (registry): 1 day, ~600 LOC Rust + tests.
- Phase B (wizard): 1.5 days, ~800 LOC Rust + integration tests.
- Phase C (scoping): 2 days, touches gateway.py + bridge.py + the
  grant-store schema.
- Phase D (multi-process): 1 day, mostly LaunchAgent template work.
- Phase E (observability): 1 day.
- Phase F (docs + tests): 1 day.

**Total: ~7-8 working days of focused work, parallelizable across
Rust + Python sides.**

## Resume pointer

If you're a fresh AI session resuming this sprint:

1. Read [PROMPT.md](PROMPT.md) — the handoff prompt.
2. Read [AUDIT.md](AUDIT.md) — current architecture.
3. Read [VISION.md](VISION.md) — the target.
4. Run `lope negotiate "$(cat SPRINT.md)" --domain engineering` to
   start Phase 0.
