# Makakoo OS Documentation

> **Many bodies. One mind.**

Makakoo gives your AI work one local home: Brain, tools, rules, plugins,
agent slots, and maintenance tasks. Local AI CLIs and IDE agents can be
infected with Makakoo instructions. Chat systems such as Telegram,
Discord, Slack, email, voice, and web connect through agent slots. Same
Brain. Different doorway.

## Start here

| Goal | Read |
|---|---|
| Install from zero | [Getting started](getting-started.md) |
| See the main daily workflows | [Use cases](use-cases.md) |
| Understand every setup prompt | [Setup wizard](user-manual/setup-wizard.md) |
| Update Makakoo later | [Upgrade guide](upgrade.md), [`makakoo update`](user-manual/makakoo-update.md) |
| Look up a command | [User manual](user-manual/index.md) |
| Fix a failure | [Troubleshooting](troubleshooting/index.md) |

## What Makakoo does

| Area | What to read |
|---|---|
| Brain and search | [Brain guide](brain/index.md), [`makakoo query`](user-manual/makakoo-query.md), [`makakoo search`](user-manual/makakoo-search.md) |
| CLI and IDE infection | [`makakoo infect`](user-manual/makakoo-infect.md), [IDE integration](concepts/ide-integration.md) |
| Agent slots and transports | [`makakoo agent`](user-manual/agent.md), [multi-transport walkthrough](walkthroughs/multi-transport-subagents.md) |
| Durable child-agent work | [`makakoo agent-session`](user-manual/makakoo-agent-session.md), [`makakoo handle`](user-manual/makakoo-handle.md) |
| Plugins and skills | [Plugin guide](plugins/index.md), [Writing plugins](plugins/writing.md) |
| SANCHO background tasks | [SANCHO concept](concepts/sancho.md), [`makakoo sancho`](user-manual/makakoo-sancho.md) |
| Write-access grants | [`makakoo perms`](user-manual/makakoo-perms.md) |
| Model adapters | [Adapters](adapters.md), [`makakoo adapter`](user-manual/makakoo-adapter.md) |
| Brain Network federation | [`makakoo network`](user-manual/makakoo-network.md) |

## Setup surfaces worth knowing

- `makakoo setup updates` chooses auto or manual Makakoo OS updates. Fresh setup defaults to auto. Existing installs stay idle until this config exists.
- `makakoo update` is the primary update command. `makakoo upgrade` is only a legacy alias.
- `makakoo setup brain` registers Logseq, Obsidian, or plain markdown sources. If Obsidian is missing, the picker can offer to install it first when Homebrew, Flatpak, or winget is available.
- `makakoo setup lope` offers Lope, Makakoo's optional in-house companion for validator ensembles for reviews, votes, compares, negotiated plans, and validator-in-the-loop sprints.
- Headroom ships in the default core distro through `tool-headroom`. It compresses bulky tool and MCP output without making users manage another setup step.

## Architecture in one picture

```text
Local AI CLIs and IDE agents
  Claude Code, Codex, OpenCode, Gemini, Vibe, Cursor, Qwen, Kimi, pi
        │
        │ makakoo infect
        ▼
Makakoo OS
  Brain + Superbrain + MCP tools + plugins + SANCHO + Headroom + optional Lope integration
        ▲
        │ makakoo agent create <slot>
        │
Chat transports
  Telegram, Slack, Discord, WhatsApp, voice, email, web
```

## Resources

- [GitHub](https://github.com/makakoo/makakoo-os)
- [Issues](https://github.com/makakoo/makakoo-os/issues)
- [Discussions](https://github.com/makakoo/makakoo-os/discussions)
- [Discord](https://discord.gg/makakoo)

MIT licensed. Local-first. No telemetry by default.
