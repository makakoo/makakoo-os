# Vision — N Telegram bots = N Makakoo subagents

## The user's expressed intent

Sebastian wants to be able to say (paraphrased from chat):

> *"For my freelance office, I want a Telegram bot that acts as the secretary.
> For my career stuff, another bot. For arbitrage, another. Each one is a
> subagent in Makakoo with its own scope, tools, files, and personality.
> I should define each easily and they should Just Work."*

The current Olibia is the proof-of-concept that ONE bot works. The redesign
opens it to N.

## Mental model

```
                         Sebastian's Telegram
                                  │
              ┌───────────────────┼───────────────────┐
              ↓                   ↓                   ↓
        @OlibiaBot          @SecretaryBot        @CareerBot
            (token A)         (token B)            (token C)
              │                   │                   │
              ↓                   ↓                   ↓
        agent-olibia       agent-secretary      agent-career
        scope:             scope:                scope:
          ~/MAKAKOO/         ~/MAKAKOO/            ~/CV/
          /tmp/                data/secretary/      ~/MAKAKOO/data/career/
        tools:             tools:                tools:
          (general)          email, calendar,      linkedin, gmail-search,
                             banking-skill         contracts-folder
              │                   │                   │
              └───────────────────┴───────────────────┘
                                  ↓
                         Shared infrastructure
                  (Brain, MCP server, grant store,
                   plugin registry, capability sandbox)
```

Each subagent:
- Has its own bot token (1:1 with a real Telegram bot via @BotFather).
- Has its own scope: which paths it can read/write, which tools it can call.
- Has its own persona and system prompt (or inherits a base + adds delta).
- Shares the Brain (so cross-agent context is preserved — Career sees what
  Secretary logged about a client meeting, etc.).
- Shares the MCP server registry but with per-agent allowed-tool filter.
- Shares the grant store but grants now carry `bound_to_agent` so a grant
  for Olibia doesn't elevate Career.

## What "easy define" looks like

```sh
makakoo agent create secretary \
  --telegram-token "8342…:AAH…" \
  --persona "Sharp, professional secretary for Sebastian's freelance office" \
  --paths ~/MAKAKOO/data/secretary/ \
  --tools email,calendar,run_command,write_file \
  --allowed-users "@schkudlara"
```

That single command:
1. Creates `~/MAKAKOO/config/agents/secretary.toml` with the config above.
2. Generates `plugins-core/agent-secretary/` from a template.
3. Drops a LaunchAgent plist (or systemd unit on Linux).
4. Starts the agent process polling its bot token.
5. Verifies the bot is reachable (`getMe` against Telegram API).
6. Prints "live — message @SecretaryBot to test".

After that, Sebastian opens Telegram, finds @SecretaryBot, sends a
message — the agent answers as the secretary persona with the secretary
tool surface.

## What does NOT change

- The Brain (single source of truth, every agent reads/writes the same
  Logseq vault).
- The Makakoo bootstrap (`~/MAKAKOO/bootstrap/global.md`) — every agent
  loads it as the BASE persona. Per-agent overrides are an additional
  layer on top.
- The capability sandbox model (three layers: baseline grants + plugin
  manifest + runtime user grants).
- The MCP server (`makakoo-mcp`) is one process serving all agents.
- The infect system for AI CLIs (Claude/Gemini/Codex/etc.) is unchanged
  — that's about CLI hosts, not Telegram-driven agents.

## What's new

- A **subagent registry** at `~/MAKAKOO/config/agents/` with one TOML
  per subagent.
- A **per-agent persona layer**: small system-prompt addendum that
  rides on top of the canonical bootstrap.
- A **per-agent scope**: enforceable allowed-tools, allowed-paths,
  allowed-users.
- A **`makakoo agent create` wizard** that turns a `--telegram-token`
  + a few flags into a live, polling subagent.
- A **per-agent LaunchAgent / systemd unit** so agents survive reboots
  independently (one crashing doesn't kill the others).
- An **identity injection at session start**: when the bot polls
  Telegram, the gateway tells the LLM "you are agent-secretary, your
  slot id is `secretary`, your scope is …". The LLM no longer has to
  guess what "give Olibia access" means.

## Acceptance criteria — sprint is "done" when

1. `makakoo agent create <name>` works end-to-end in <30s on a clean
   machine (assuming Telegram bot already created via @BotFather).
2. Three subagents simultaneously running on Sebastian's machine, each
   responding only on its own bot, with isolated personas + tools.
3. Per-agent grant scoping enforced: `agent-career` cannot write to
   `~/MAKAKOO/data/secretary/` even if a grant exists, because the
   grant is bound to a different agent slot.
4. Each agent logs to today's Brain journal (with agent-id prefix) so
   cross-agent visibility is preserved.
5. `makakoo agent list` shows live status (running/stopped/crashed +
   last-message timestamp + bot username) for every configured slot.
6. CLI smoke test: send a message to every bot, get a personalised
   reply within 5s.
7. The original Olibia bot continues to work without reconfiguration
   (migrated transparently to the new model with slot id `harveychat`
   for backward compat).

## Non-goals (out of scope for this sprint)

- WhatsApp / Slack adapters (gateway.py mentions them as future work
  — keep the architecture friendly to those without building them).
- Multi-user-per-agent ACLs beyond a simple username allowlist.
- Voice mode, video, advanced media — keep current functionality
  unchanged, just multiply.
- Cross-agent delegation ("@Olibia, ask @CareerBot if there are any
  openings") — interesting but adds an interaction layer best deferred.
- A Telegram-side user-facing "manage my bots" admin bot — could come
  later as `@MakakooMgrBot`, but not part of this sprint.

## Risk register

- **Token sprawl**: storing N tokens in N config files vs OS keyring.
  Decision needed in SPRINT.md.
- **Process blow-up**: 5 subagents = 5 Python processes ≈ 500MB RAM
  baseline. Acceptable for a workstation but may need a multiplexed
  gateway later.
- **Persona-prompt drift**: every agent inheriting the canonical
  bootstrap means a 31KB prompt per call. Cost / latency.
- **BotFather rate limits**: Telegram limits bot creation to ~20 per
  account per day. If a user wants more, they need a Telegram Premium
  / multiple Telegram accounts.
- **User confusion about who is who**: bot avatars + usernames matter.
  The wizard should print clear "this bot will be at @X — go talk to it"
  guidance.
