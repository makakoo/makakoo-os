# Audit — current Telegram + agent architecture (2026-04-26)

Snapshot of what exists today, BEFORE the multi-bot-subagents redesign.
Read this first in any new context window — saves rediscovering.

## TL;DR

There is **one Telegram bot, one persona ("Harvey"), one gateway process**.
The user (Sebastian) has been talking to "Olibia" in Telegram — but
Olibia is just a Telegram username/avatar. Internally the same
`agent-harveychat` process answers as the persona configured in
`~/MAKAKOO/config/persona.json` (currently `name: "Harvey"`).

When the user asks Olibia "give Olibia access to shared folders", Olibia
parses "Olibia" as a third party (not herself) and either punts to the
terminal or grants herself access. The model has no concept of
**"Olibia" being one configured subagent slot among N**.

The redesign sprint must change that.

## File-level architecture today

```
~/MAKAKOO/
├── config/
│   ├── persona.json                        # one persona for the whole machine
│   └── bootstrap-cache.md                   # rendered v12 bootstrap (all CLIs read this)
├── data/
│   ├── chat/
│   │   ├── config.json                      # ONE bot token + ONE allowlist + ONE switchAI URL
│   │   ├── conversations.db                 # all telegram convos in one SQLite DB
│   │   ├── harveychat.log
│   │   └── harveychat.pid
│   └── auto-memory/                         # cross-session memory (shared by every CLI)
└── plugins/
    └── agent-harveychat/                    # single agent plugin
        ├── plugin.toml                      # kind=agent, language=python
        └── src/
            ├── agent.py                     # 70 LOC bootstrap → core.chat.__main__
            ├── agent.yaml                   # tool list (12 tools)
            ├── AGENT.md                     # human docs
            └── com.harvey.harveychat.plist  # LaunchAgent
```

The actual bridge code lives at:

```
~/MAKAKOO/plugins/lib-harvey-core/src/core/chat/
├── bridge.py             # HARVEY_SYSTEM_PROMPT constant + render_system_prompt()
├── gateway.py            # Telegram polling loop + message dispatch
├── media.py              # photo/voice helpers
├── brain_sync.py         # auto-journal every user message
└── __main__.py           # entry point — reads data/chat/config.json
```

## How a Telegram message flows today

```
Telegram user (Sebastian)
  ↓ "give Olibia access to shared folders"
Telegram Bot API (one bot, token in data/chat/config.json)
  ↓ getUpdates polling (every ~30s)
gateway.py — single allowlist check (user_id in [746496145])
  ↓
bridge.py::render_system_prompt(channel="telegram")
  → HARVEY_SYSTEM_PROMPT (1.5KB embedded persona) + canonical bootstrap (29KB)
  ↓
LLM call (switchAILocal → minimax/MiniMax-M2.7) with full system prompt + tools
  ↓
Tool calls (brain_search, write_file, run_command, browse_url, ...) executed in-process
  ↓
LLM response → Telegram sendMessage
  ↓
brain_sync.py auto-journal of "@schkudlara: <user message>"
```

Every message uses **the same persona, same tool set, same allowlist,
same brain, same write-grants store**. There is no per-conversation
or per-bot scoping.

## What "Olibia" means today

- **Telegram side**: just the bot's username + avatar. Sebastian renamed
  the bot to "Olibia" via @BotFather. Token is the original Harvey-OS
  era token from 2026-04-10ish.
- **Code side**: the constant `HARVEY_SYSTEM_PROMPT` in `bridge.py` says
  "You are Harvey". There is no Olibia branding inside the prompt.
- **Persona file**: `~/MAKAKOO/config/persona.json` says `name: "Harvey"`.

So when Sebastian says "give Olibia access", the LLM reads "Olibia" in
his message and the system prompt saying "You are Harvey" and gets
confused — they're different entities by name. The LLM defaults to
treating "Olibia" as a third party (the bot avatar) rather than itself.

## Why she keeps punting to the terminal

Olibia's pattern in screenshots:
- *"You need to run `garagetytus folder bind` first in your terminal"*
- *"I have no tool to grant permissions to other agents/systems"*

Two real causes:

1. **No `run_command` policy clarity**. She HAS the `run_command` tool
   but the system prompt restricts it to "safe shell commands (ls, ps,
   crontab -l, git status)". She doesn't think of `garagetytus folder
   bind` as in-scope.

2. **Tool surface is generic, not role-specific**. Olibia has the same
   12 tools as every other use case (writing PDFs, browsing URLs,
   etc.). Nothing tells her "for setup tasks, you SHOULD run
   garagetytus / mkdir / makakoo perms commands directly". The
   bootstrap's anti-punt rule (added today) is in her prompt, but it
   conflicts with the embedded "safe shell only" guard.

## What's already pluggable

Some good news — the codebase is closer to the redesign than it looks:

- `plugin.toml::kind = "agent"` is the canonical agent slot. There's
  already a registry (`makakoo plugin list` shows 13 `agent-*`
  plugins).
- `makakoo agent {start, stop, status, health}` was added 2026-04-25
  to drive any agent's lifecycle.
- Every plugin has its own `state/` dir + `[capabilities]` grants in
  `plugin.toml` — the per-agent isolation primitives exist.
- The bot-token + allowlist concept is centralised in ONE config file
  but the *file* is per-agent: `data/chat/config.json` belongs to
  agent-harveychat. Other agents could have their own `config.json`
  if we plumbed it.

## What's hard-coded against multi-bot today

- `HARVEY_SYSTEM_PROMPT` is a single string constant in `bridge.py`.
  No persona templating beyond `{channel}` and `{allowed_paths}`.
- `gateway.py` reads ONE config: `data/chat/config.json`. No
  enumeration of multiple bot configs.
- The Telegram polling loop spawns one Python `core.chat` process.
  Two bots = two processes today (no sharing of brain access /
  conversation DB / tool registry).
- `grant_write_access` doesn't carry "for which agent" — `label`
  exists but it's a free-form tag, not an enforceable scope.
- No `makakoo agent create` wizard. New agents = manual `plugin.toml`
  + Python entrypoint + LaunchAgent plist + config.json copy.

## Smell tests — what we want the NEW system to pass

When Sebastian opens the redesigned system, these prompts in Telegram
should Just Work:

1. *"@SecretaryBot: pay this invoice — attached"* → secretary subagent
   reads PDF, drafts payment in user's banking app via skill, asks
   confirmation.
2. *"@CareerBot: any new openings matching my CV?"* → career-manager
   subagent runs LinkedIn API skill, reports.
3. *"@OlibiaBot: give yourself write access to shared folders"* →
   Olibia knows SHE is `agent-olibia` (slot id), resolves "shared
   folders" via discovery, calls `grant_write_access(label="olibia",
   bound_to_agent="agent-olibia")`, returns the verbatim grant.
4. *"@CareerBot: give yourself the same access"* → fails because
   CareerBot is scoped only to `~/CV/` and `~/MAKAKOO/data/career/`.
   Tells Sebastian "out of scope, ask Olibia or grant me explicitly".
5. CLI: `makakoo agent create secretary --telegram-token=<…>
   --tools=email,calendar,banking-skill --paths=~/MAKAKOO/data/secretary/`
   → wizard creates plugin, registers, drops a config, deploys
   LaunchAgent, starts polling. Done in <30 seconds.

If any of those don't pass, the sprint isn't done.

## Open questions that need answers in SPRINT.md

- One-process-per-agent vs multiplexed gateway? (CPU/RAM tradeoff)
- Per-bot vs per-conversation scoping? (one bot can serve many users)
- How does an agent know *its own slot id*? (env var? gateway header?)
- Where do agent definitions live? (`~/MAKAKOO/config/agents/<name>.toml`?
  or `plugins-core/agent-<name>/`?)
- Discovery: does `makakoo agent list` enumerate all configured bots
  even if their token is unset?
- Tool scoping: per-agent allowed-tools whitelist? Per-agent
  forbidden-paths blacklist?
- Telegram username vs slot id: does the `@OlibiaBot` username need
  to match `agent-olibia`? (probably yes for clarity)
