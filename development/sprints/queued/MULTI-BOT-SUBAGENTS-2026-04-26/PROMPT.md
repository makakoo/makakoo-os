# Fresh-context handoff prompt

Paste this verbatim into a fresh Claude Code / Codex / Gemini session
to resume the multi-bot-subagents sprint without re-discovering anything.

---

```
You are Harvey, Sebastian's autonomous extension on Makakoo OS. You're
resuming a sprint that was scoped (but not yet executed) on 2026-04-26.

## What I want you to do

The sprint folder is at:

    /Users/sebastian/makakoo-os/development/sprints/queued/MULTI-BOT-SUBAGENTS-2026-04-26/

It contains four files. Read them in this order:

1. AUDIT.md — current state of Makakoo's Telegram + agent architecture.
   Don't skim. The audit lists every file that touches Telegram today
   and every place where the design assumes ONE bot.

2. VISION.md — the target: N Telegram bots = N Makakoo subagents,
   each with its own scope, persona, tools, and filesystem grants.
   `makakoo agent create <name>` deploys a new subagent in <30s.

3. SPRINT.md — six phases (0-F) with checkboxes. Phase 0 is a lope
   negotiation that LOCKS the answers to seven open architectural
   questions before any code is written.

4. PROMPT.md — this file (resume pointer). You can skip it; you're
   already reading it.

## What to do first

Run lope negotiate against the sprint to lock the open questions:

    cd /Users/sebastian/makakoo-os/development/sprints/queued/MULTI-BOT-SUBAGENTS-2026-04-26/
    PYTHONPATH=~/.lope python3 -m lope negotiate \
      "Multi-bot subagents over Telegram — see SPRINT.md for context, \
       AUDIT.md for current state, VISION.md for the target. Lock the \
       seven open questions in AUDIT.md § Open questions. Output a \
       refined SPRINT.md with the answers inline." \
      --out NEGOTIATED-SPRINT.md \
      --max-rounds 3 \
      --domain engineering

Use the pi+gemini+codex ensemble (avoid qwen — its drafter style
fights linters per the lope_wedge memory).

After lope reaches PASS, the agreed answers go into SPRINT.md (replace
the "open questions" section with "locked decisions"). Then proceed to
Phase A.

## What NOT to do

- Don't start writing Rust before Phase 0 is locked. The agent-config
  schema is load-bearing for every subsequent phase; if it's wrong,
  rework cascades.
- Don't break the existing Olibia bot during the migration. Phase A
  must keep her live as slot_id="harveychat".
- Don't store bot tokens in plain config files unless the lope ensemble
  explicitly verdicts that's OK. Default-favor the OS keyring (we have
  `makakoo secret` for that).

## Background you need

Three memories that matter (read in full if anything is unclear):

- agent_swarm_sprint — TIER 3 swarm sprint plan from earlier; this
  multi-bot work is a SCOPED PRECURSOR to that.
- telegram_group_setup — the 5 fixes that made Olibia work in groups.
  Do not regress those during the redesign.
- v06_agentic_plug — shipped 2026-04-21, gives the bring-your-own-agent
  primitives this sprint builds on.

## Why this exists

Sebastian's Olibia bot in Telegram answers "give Olibia access to
shared folders" with confused output: she treats "Olibia" as a third
party, can only grant herself, and punts to the terminal for setup
tasks. The gap is structural — there is one persona, one bot, one
config — and the LLM can't reason about itself as "agent-olibia" out
of N because N=1 today.

The user wants to grow this to:
  @SecretaryBot for the freelance office
  @CareerBot for job-hunting
  @ArbitrageBot for trading
  @OlibiaBot remains as the general-purpose default
  …etc.

Each one a real Makakoo subagent with isolated scope.

## CRITICAL framing — this is core, not a plugin

Sebastian explicitly stated (2026-04-26): "this should be a core
functionality in makakoo-os." That changes the bar:

- `makakoo agent` is a first-class verb at the same depth as `plugin`,
  `perms`, `daemon`. Not a sub-feature.
- The subagent abstraction subsumes the existing 13 `agent-*` plugins
  (arbitrage, career-manager, browser-harness, harveychat, etc.) under
  a unified model — they only differ in which transport(s) they attach
  to (Telegram messenger / SANCHO schedule / tool-only).
- The schema is transport-agnostic: Telegram is `transport.kind =
  "telegram"`, but the model must accommodate WhatsApp / Slack / email
  / voice without rework.
- Every other subsystem learns about agent-id: grants gain
  `bound_to_agent`, Brain entries get `agent_id` prefix when written
  by an agent, MCP carries an originating-agent header.
- Onboarding promotion: README quickstart + `makakoo setup` wizard
  section + `docs/getting-started.md` step.

The Genie metaphor (per `harvey_genie` memory) makes this natural:
each subagent is a specialised Genie. `makakoo agent create` is
"summon a new Genie".

## When you're done with Phase 0

Update SPRINT.md with the negotiated answers, commit:

    cd /Users/sebastian/makakoo-os
    git add development/sprints/queued/MULTI-BOT-SUBAGENTS-2026-04-26/
    git commit -m "sprint(multi-bot-subagents): Phase 0 negotiation locked"

Then ask Sebastian for go-ahead on Phase A. Don't start Phase A
unilaterally — the audit is fresh enough that a 5-min review prevents
wasted work.
```

---

## Why a separate handoff prompt

This sprint is large enough that the *current* context window has
accumulated noise (perms-bug fixes, pointer-pattern refactor, install
docs, etc.). Starting fresh keeps the next agent focused on the
multi-bot work without re-litigating earlier decisions.

The prompt above is **self-contained** — every path it references is
absolute, every command is copy-paste runnable, every external memory
is named. A fresh agent loading only the magic-URL skill plus this
prompt should be able to execute Phase 0 to completion.
